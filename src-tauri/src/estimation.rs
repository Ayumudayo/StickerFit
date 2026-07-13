#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use image::{ImageFormat, Rgba, RgbaImage};
    use serde_json::json;

    use super::{
        count_full_candidate_apng, estimate_candidate_size, estimate_candidate_size_with_workload,
        estimate_proxy_stratified_residual_interval, estimate_residual_interval,
        estimate_static_png, measure_sampled_apng_parts, parse_sample_seed, probe_candidate_size,
        probe_candidate_size_with_workload, sample_positions_without_replacement,
        sample_proxy_stratified_positions_without_replacement, select_extreme_transition_positions,
        select_extreme_transition_positions_from_proxies, transition_proxy_scores, ByteCounter,
        CountingWriter, EstimateConfidence, EstimateWorkloadCounters, OutputSizeEstimate,
        OutputSizeEstimateError,
    };
    use crate::frame_source::{
        build_candidate_output_sequence, PreparedFrame, PreparedSearchSource,
        TimelineTimingAuthority,
    };
    use crate::locale;
    use crate::media_error::PipelineError;
    use crate::media_limits::{MediaLimits, SourceIdentity};
    use crate::operation::{MediaOperationKind, OperationContext};
    use crate::{
        convert_static_image_to_png_internal, CandidatePreview, CropRegion, ResolvedTimelineFrame,
        UiLocale,
    };

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock")
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("stickerfit-{name}-{}-{nonce}", std::process::id()));
            fs::create_dir(&path).expect("test directory");
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct CandidateFixture {
        prepared: PreparedSearchSource,
        candidate: CandidatePreview,
    }

    impl CandidateFixture {
        fn new(frames: Vec<RgbaImage>) -> Self {
            let output_count = frames.len();
            let prepared_frames = frames
                .into_iter()
                .enumerate()
                .map(|(index, pixels)| PreparedFrame {
                    source_frame_id: u32::try_from(index + 1).expect("fixture frame id"),
                    duration_us: 10_000,
                    pixels: Arc::new(pixels),
                })
                .collect::<Vec<_>>();
            let base_sequence = (0..output_count)
                .map(|index| ResolvedTimelineFrame {
                    source_frame_index: u32::try_from(index).expect("fixture source index"),
                    duration_us: 10_000,
                })
                .collect::<Vec<_>>();
            Self {
                prepared: PreparedSearchSource {
                    frames: prepared_frames,
                    base_sequence,
                    timing_authority: TimelineTimingAuthority::Authored,
                    base_fps: 30,
                    decoded_bytes: output_count * 96 * 96 * 4,
                    tool_source: "fixture".into(),
                    tool_command: None,
                    tool_detail: None,
                },
                candidate: CandidatePreview {
                    id: "fixture-candidate".into(),
                    rank: 1,
                    duration_seconds: output_count as f64 / 30.0,
                    fps: 30,
                    content_scale: 1.0,
                    preset: "standard".into(),
                    fit_mode: "contain".into(),
                    score: 1.0,
                    source_similarity_score: 1.0,
                    relative_size_factor: 1.0,
                    summary: "fixture".into(),
                    frame_sample_step: 1,
                },
            }
        }

        fn sequence(
            &self,
            context: &OperationContext,
        ) -> crate::frame_source::PreparedCandidateSequence<'_, '_> {
            build_candidate_output_sequence(&self.prepared, &self.candidate, context)
                .expect("fixture candidate sequence")
        }
    }

    fn moving_square_frames(count: usize) -> Vec<RgbaImage> {
        (0..count)
            .map(|frame| {
                RgbaImage::from_fn(96, 96, |x, y| {
                    let offset = (frame * 3) as u32 % 80;
                    if (offset..offset + 16).contains(&x) && (32..48).contains(&y) {
                        Rgba([240, 70, 20, 255])
                    } else {
                        Rgba([8, 12, 18, 255])
                    }
                })
            })
            .collect()
    }

    fn seeded_pixel(seed: u32, frame: u32, x: u32, y: u32) -> Rgba<u8> {
        let mut value = seed
            .wrapping_add(frame.wrapping_mul(0x9e37_79b9))
            .wrapping_add(x.wrapping_mul(0x85eb_ca6b))
            .wrapping_add(y.wrapping_mul(0xc2b2_ae35));
        value ^= value >> 16;
        value = value.wrapping_mul(0x7feb_352d);
        value ^= value >> 15;
        Rgba([value as u8, (value >> 8) as u8, (value >> 16) as u8, 255])
    }

    fn generated_estimate_fixture_frames(kind: &str, seed: u32) -> Vec<RgbaImage> {
        (0..150_u32)
            .map(|frame| match kind {
                "solid" => RgbaImage::from_pixel(96, 96, Rgba([12, 34, 56, 255])),
                "moving-square" => RgbaImage::from_fn(96, 96, |x, y| {
                    let offset = frame * 3 % 80;
                    if (offset..offset + 16).contains(&x) && (32..48).contains(&y) {
                        Rgba([240, 70, 20, 255])
                    } else {
                        Rgba([8, 12, 18, 255])
                    }
                }),
                "alternating-colors" => RgbaImage::from_pixel(
                    96,
                    96,
                    if frame % 2 == 0 {
                        Rgba([220, 30, 60, 255])
                    } else {
                        Rgba([20, 80, 210, 255])
                    },
                ),
                "alpha-gradient-motion" => RgbaImage::from_fn(96, 96, |x, y| {
                    Rgba([
                        x as u8,
                        y as u8,
                        frame as u8,
                        ((x + y + frame) % 96 * 255 / 95) as u8,
                    ])
                }),
                "seeded-entropy" => {
                    RgbaImage::from_fn(96, 96, |x, y| seeded_pixel(seed, frame, x, y))
                }
                "mixed-entropy" => {
                    if frame % 12 < 3 {
                        RgbaImage::from_fn(96, 96, |x, y| seeded_pixel(seed, frame, x, y))
                    } else {
                        RgbaImage::from_pixel(96, 96, Rgba([frame as u8, 40, 90, 255]))
                    }
                }
                "periodic-bursts" => {
                    if frame % 20 == 0 {
                        RgbaImage::from_fn(96, 96, |x, y| seeded_pixel(seed, frame, x, y))
                    } else {
                        RgbaImage::from_pixel(96, 96, Rgba([17, 27, 37, 255]))
                    }
                }
                "single-scene-cut" => RgbaImage::from_pixel(
                    96,
                    96,
                    if frame < 75 {
                        Rgba([10, 20, 30, 255])
                    } else {
                        Rgba([210, 180, 140, 255])
                    },
                ),
                "long-still-short-motion" => {
                    if frame < 130 {
                        RgbaImage::from_pixel(96, 96, Rgba([25, 35, 45, 255]))
                    } else {
                        let offset = (frame - 130) * 3 % 80;
                        RgbaImage::from_fn(96, 96, |x, y| {
                            if (offset..offset + 16).contains(&x) && (32..48).contains(&y) {
                                Rgba([240, 70, 20, 255])
                            } else {
                                Rgba([25, 35, 45, 255])
                            }
                        })
                    }
                }
                "separate-seed" => {
                    RgbaImage::from_fn(96, 96, |x, y| seeded_pixel(seed ^ 0xa5a5_5a5a, frame, x, y))
                }
                _ => unreachable!("known generated fixture"),
            })
            .collect()
    }

    fn arithmetic_resample_intervals(
        sequence: &crate::frame_source::PreparedCandidateSequence<'_, '_>,
        context: &OperationContext,
    ) -> (u64, Vec<super::ByteInterval>) {
        let all_positions = (1..sequence.frames.len()).collect::<BTreeSet<_>>();
        let all = measure_sampled_apng_parts(sequence, &all_positions, context)
            .expect("one all-transition compression pass");
        let actual = all
            .transition_bytes
            .values()
            .try_fold(all.global_and_first_bytes, |sum, contribution| {
                sum.checked_add(*contribution)
            })
            .expect("actual generated fixture bytes");
        let transition_proxies =
            transition_proxy_scores(sequence, context).expect("generated fixture proxies");
        let extremes = select_extreme_transition_positions_from_proxies(&transition_proxies, 2);
        let extreme_set = extremes.iter().copied().collect::<BTreeSet<_>>();
        let residual_proxies = transition_proxies
            .into_iter()
            .filter(|(position, _)| !extreme_set.contains(position))
            .collect::<Vec<_>>();
        let fixed = extremes
            .iter()
            .try_fold(all.global_and_first_bytes, |sum, position| {
                sum.checked_add(all.transition_bytes[position])
            })
            .expect("generated fixture fixed bytes");
        let intervals = (0..200_u64)
            .map(|seed| {
                let sampled_positions = sample_proxy_stratified_positions_without_replacement(
                    &residual_proxies,
                    9,
                    seed,
                )
                .expect("proxy-stratified holdout resample");
                let sampled_contributions = sampled_positions
                    .into_iter()
                    .map(|position| (position, all.transition_bytes[&position]))
                    .collect::<BTreeMap<_, _>>();
                estimate_proxy_stratified_residual_interval(
                    &residual_proxies,
                    &sampled_contributions,
                    fixed,
                )
                .expect("holdout interval")
            })
            .collect();
        (actual, intervals)
    }

    fn write_fixture(path: &Path, pixels: RgbaImage) {
        pixels
            .save_with_format(path, ImageFormat::Png)
            .expect("fixture PNG");
    }

    fn alpha_gradient(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            Rgba([
                (x % 251) as u8,
                (y % 241) as u8,
                ((x + y) % 239) as u8,
                ((x * 255) / width.max(1)) as u8,
            ])
        })
    }

    fn deterministic_entropy(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            let mut value = x
                .wrapping_mul(0x9e37_79b9)
                .wrapping_add(y.wrapping_mul(0x85eb_ca6b));
            value ^= value >> 16;
            value = value.wrapping_mul(0x7feb_352d);
            value ^= value >> 15;
            Rgba([value as u8, (value >> 8) as u8, (value >> 16) as u8, 255])
        })
    }

    fn assert_estimate_matches_conversion(input: &Path, crop: Option<&CropRegion>) {
        let context = OperationContext::detached(MediaOperationKind::StaticEstimate.timeout());
        let estimate = estimate_static_png(input, crop, &context, MediaLimits::default())
            .expect("static estimate");
        let output_dir = input
            .parent()
            .expect("fixture parent")
            .join("output")
            .join(input.file_stem().expect("fixture stem"));
        fs::create_dir_all(&output_dir).expect("output directory");
        let input_path = input.to_string_lossy().into_owned();
        let output_directory = output_dir.to_string_lossy().into_owned();
        let conversion = convert_static_image_to_png_internal(
            &input_path,
            Some(output_directory.as_str()),
            crop,
            UiLocale::En,
            None,
        );
        assert!(conversion.ok, "conversion: {conversion:?}");
        let actual = fs::metadata(conversion.output_path.expect("published output"))
            .expect("output metadata")
            .len();
        assert!(matches!(
            estimate,
            OutputSizeEstimate::ExactStatic {
                bytes,
                output_frame_count: 1,
                ..
            } if bytes == actual
        ));
    }

    #[test]
    fn static_estimate_variants_serialize_with_closed_discriminants_and_candidate_nullability() {
        let exact_static = OutputSizeEstimate::exact_static(123);
        let exact_candidate = OutputSizeEstimate::exact_candidate("candidate-1".into(), 456, 7);
        let exact_probe = OutputSizeEstimate::exact_candidate_with_basis(
            super::ExactCandidateBasis::Probe,
            "candidate-probe".into(),
            789,
            9,
        );
        let range = OutputSizeEstimate::sampled_range(
            "candidate-2".into(),
            100,
            200,
            300,
            super::EstimateConfidence::High,
            12,
            40,
        );

        assert_eq!(
            serde_json::to_value(exact_static).expect("exact static JSON"),
            json!({
                "kind": "exact-static",
                "basis": "exact-static",
                "bytes": 123,
                "candidateId": null,
                "limitBytes": 524288,
                "outputFrameCount": 1
            })
        );
        assert_eq!(
            serde_json::to_value(exact_candidate).expect("exact candidate JSON"),
            json!({
                "kind": "exact-candidate",
                "basis": "exact-full-sequence",
                "bytes": 456,
                "candidateId": "candidate-1",
                "limitBytes": 524288,
                "outputFrameCount": 7
            })
        );
        assert_eq!(
            serde_json::to_value(exact_probe).expect("exact probe JSON"),
            json!({
                "kind": "exact-candidate",
                "basis": "probe",
                "bytes": 789,
                "candidateId": "candidate-probe",
                "limitBytes": 524288,
                "outputFrameCount": 9
            })
        );
        assert_eq!(
            serde_json::to_value(range).expect("range JSON"),
            json!({
                "kind": "range",
                "basis": "sampled",
                "lowerBytes": 100,
                "predictedBytes": 200,
                "upperBytes": 300,
                "confidence": "high",
                "candidateId": "candidate-2",
                "limitBytes": 524288,
                "measuredContributionCount": 12,
                "outputFrameCount": 40
            })
        );
    }

    #[test]
    fn static_estimate_counting_writer_counts_fragmented_writes_without_a_byte_buffer() {
        let mut writer = CountingWriter::new(io::sink());
        let counter = writer.counter();
        writer.write_all(b"abc").expect("first write");
        writer.write_all(b"defgh").expect("second write");
        writer.flush().expect("flush sink");

        assert_eq!(writer.bytes_written(), 8);
        assert_eq!(counter.bytes_written(), 8);
        assert_eq!(
            std::mem::size_of_val(&writer.inner),
            std::mem::size_of::<io::Sink>()
        );
    }

    #[test]
    fn sampled_positions_select_all_small_populations_and_are_sorted_unique() {
        for population in 1..=9 {
            assert_eq!(
                sample_positions_without_replacement(population, 9, 7)
                    .expect("small population sample"),
                (0..population).collect::<Vec<_>>()
            );
        }

        let positions = sample_positions_without_replacement(150, 9, 0x0123_4567_89ab_cdef)
            .expect("bounded sample");
        assert_eq!(positions.len(), 9);
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(positions.iter().all(|position| *position < 150));
    }

    #[test]
    fn sampled_positions_are_seeded_uniform_and_reject_zero_limit() {
        let first = sample_positions_without_replacement(150, 9, 17).expect("first sample");
        let repeated = sample_positions_without_replacement(150, 9, 17).expect("repeat sample");
        let different = sample_positions_without_replacement(150, 9, 18).expect("other sample");
        assert_eq!(first, repeated);
        assert_ne!(first, different);
        assert_eq!(
            sample_positions_without_replacement(0, 0, 1).expect("empty population"),
            Vec::<usize>::new()
        );
        assert!(sample_positions_without_replacement(1, 0, 1).is_err());

        let mut counts = [0_u32; 150];
        for seed in 0..1_000 {
            for position in
                sample_positions_without_replacement(150, 9, seed).expect("uniform sample")
            {
                counts[position] += 1;
            }
        }
        let expected = 60.0_f64;
        let chi_square = counts.iter().fold(0.0, |sum, observed| {
            let delta = f64::from(*observed) - expected;
            sum + delta * delta / expected
        });
        assert!(chi_square < 210.0, "chi square was {chi_square}");
    }

    #[test]
    fn sample_seed_parser_accepts_exact_lowercase_u64_hex_only() {
        assert_eq!(parse_sample_seed("0000000000000000"), Ok(0));
        assert_eq!(parse_sample_seed("ffffffffffffffff"), Ok(u64::MAX));
        for rejected in [
            "",
            "000000000000000",
            "00000000000000000",
            "0123456789ABCDEf",
            "0123456789abcdeg",
            " 123456789abcdef",
        ] {
            assert!(parse_sample_seed(rejected).is_err(), "{rejected:?}");
        }
    }

    #[test]
    fn residual_interval_is_exact_when_all_sampled_and_uses_checked_rounding() {
        let exact =
            estimate_residual_interval(&[11, 19, 31], 3, 100).expect("all-sampled residual");
        assert_eq!(exact.lower_bytes, 161);
        assert_eq!(exact.predicted_bytes, 161);
        assert_eq!(exact.upper_bytes, 161);
        assert_eq!(exact.confidence, EstimateConfidence::High);

        let interval =
            estimate_residual_interval(&[10, 20, 31], 9, 100).expect("sampled residual interval");
        assert!(interval.lower_bytes <= interval.predicted_bytes);
        assert!(interval.predicted_bytes <= interval.upper_bytes);
        assert!(interval.lower_bytes >= 100);
    }

    #[test]
    fn residual_interval_finite_population_correction_narrows_and_checks_failures() {
        let contributions = [
            1_000, 2_000, 4_000, 8_000, 16_000, 32_000, 64_000, 128_000, 256_000,
        ];
        let near_census =
            estimate_residual_interval(&contributions, 10, 500).expect("near-census interval");
        let large_population = estimate_residual_interval(&contributions, 100, 500)
            .expect("large-population interval");
        assert!(
            near_census.upper_bytes - near_census.lower_bytes
                < large_population.upper_bytes - large_population.lower_bytes
        );
        assert!(estimate_residual_interval(&[1], 2, 0).is_err());
        assert!(estimate_residual_interval(&[u64::MAX], 1, 1).is_err());
        assert!(estimate_residual_interval(&[u64::MAX, u64::MAX], usize::MAX, 0).is_err());
        assert_eq!(super::finite_u64(1.4, f64::floor), Ok(1));
        assert_eq!(super::finite_u64(1.4, f64::round), Ok(1));
        assert_eq!(super::finite_u64(1.4, f64::ceil), Ok(2));
        assert!(super::finite_u64(-1.0, f64::round).is_err());
        assert!(super::finite_u64(f64::NAN, f64::round).is_err());
        assert!(super::finite_u64(f64::INFINITY, f64::round).is_err());
        assert!(super::finite_u64(u64::MAX as f64, f64::round).is_err());
    }

    #[test]
    fn all_unchanged_incomplete_stratum_keeps_a_nonzero_interval_at_low_confidence() {
        let residual_proxies = (1..=10).map(|position| (position, 0)).collect::<Vec<_>>();
        let sampled_contributions = (1..=7)
            .map(|position| (position, 100))
            .collect::<BTreeMap<_, _>>();
        let interval = estimate_proxy_stratified_residual_interval(
            &residual_proxies,
            &sampled_contributions,
            1_000_000,
        )
        .expect("all-unchanged incomplete stratum");
        let missed_rare_large_actual = 1_000_000 + 7 * 100 + 2 * 100 + 500;

        assert_eq!(interval.predicted_bytes, 1_001_000);
        assert!(interval.lower_bytes < interval.predicted_bytes);
        assert!(interval.predicted_bytes < interval.upper_bytes);
        assert!(missed_rare_large_actual <= interval.upper_bytes);
        assert_eq!(interval.confidence, EstimateConfidence::Low);
    }

    #[test]
    fn non_stratified_interval_uses_sample_degrees_of_freedom() {
        let three_samples =
            estimate_residual_interval(&[100; 3], 10, 0).expect("three-sample interval");
        let seven_samples =
            estimate_residual_interval(&[100; 7], 10, 0).expect("seven-sample interval");

        assert_eq!(three_samples.upper_bytes, 3_191);
        assert_eq!(seven_samples.upper_bytes, 1_534);
        assert_eq!(three_samples.confidence, EstimateConfidence::Low);
        assert_eq!(seven_samples.confidence, EstimateConfidence::Low);
    }

    #[test]
    fn stratified_equal_changed_samples_cover_a_missed_rare_contribution_without_high_confidence() {
        let residual_proxies = (1..=12)
            .map(|position| {
                let proxy = match position {
                    1 | 2 => 0,
                    12 => 10_000,
                    _ => 100,
                };
                (position, proxy)
            })
            .collect::<Vec<_>>();
        let sampled_contributions = [(1, 40), (2, 40)]
            .into_iter()
            .chain((3..=9).map(|position| (position, 100)))
            .collect::<BTreeMap<_, _>>();
        let interval = estimate_proxy_stratified_residual_interval(
            &residual_proxies,
            &sampled_contributions,
            1_000_000,
        )
        .expect("stratified equal changed sample");
        let missed_rare_large_actual = 1_000_000 + 2 * 40 + 7 * 100 + 2 * 100 + 500;

        assert_eq!(interval.predicted_bytes, 1_001_080);
        assert!(interval.lower_bytes < interval.predicted_bytes);
        assert!(interval.predicted_bytes < interval.upper_bytes);
        assert!(missed_rare_large_actual <= interval.upper_bytes);
        assert_eq!(interval.confidence, EstimateConfidence::Low);
    }

    #[test]
    fn incomplete_unchanged_stratum_is_conservative_when_changed_stratum_is_exhaustive() {
        let residual_proxies = (1..=7)
            .map(|position| (position, if position >= 6 { 1 } else { 0 }))
            .collect::<Vec<_>>();
        let sampled_contributions = [(1, 100), (2, 100), (3, 100), (6, 1_000), (7, 1_000)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let interval = estimate_proxy_stratified_residual_interval(
            &residual_proxies,
            &sampled_contributions,
            0,
        )
        .expect("incomplete unchanged stratum");
        let missed_rare_large_actual = 3 * 100 + 100 + 500 + 2 * 1_000;

        assert_eq!(interval.predicted_bytes, 2_500);
        assert!(missed_rare_large_actual <= interval.upper_bytes);
        assert_eq!(interval.confidence, EstimateConfidence::Low);

        let exhaustive_contributions = [
            (1, 100),
            (2, 100),
            (3, 100),
            (4, 100),
            (5, 500),
            (6, 1_000),
            (7, 1_000),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let exact = estimate_proxy_stratified_residual_interval(
            &residual_proxies,
            &exhaustive_contributions,
            0,
        )
        .expect("exhaustive strata");
        assert_eq!(exact.lower_bytes, missed_rare_large_actual);
        assert_eq!(exact.predicted_bytes, missed_rare_large_actual);
        assert_eq!(exact.upper_bytes, missed_rare_large_actual);
        assert_eq!(exact.confidence, EstimateConfidence::High);
    }

    #[test]
    fn displayed_relative_half_width_sets_confidence_boundaries() {
        let high = super::confidence_for_interval(750, 1_000, 1_250);
        let medium = super::confidence_for_interval(250, 1_000, 1_750);
        let low = super::confidence_for_interval(0, 1_000, 2_000);
        assert_eq!(high, EstimateConfidence::High);
        assert_eq!(medium, EstimateConfidence::Medium);
        assert_eq!(low, EstimateConfidence::Low);
    }

    #[test]
    fn sampled_parts_preserve_actual_predecessors_and_sum_to_the_full_common_writer() {
        let fixture = CandidateFixture::new(moving_square_frames(18));
        let context = OperationContext::detached(Duration::from_secs(5));
        let sequence = fixture.sequence(&context);
        let all_positions = (1..sequence.frames.len()).collect::<BTreeSet<_>>();
        let all = measure_sampled_apng_parts(&sequence, &all_positions, &context)
            .expect("all transition parts");
        let full = count_full_candidate_apng(&sequence, &context).expect("full APNG count");
        let reconstructed = all
            .transition_bytes
            .values()
            .try_fold(all.global_and_first_bytes, |sum, contribution| {
                sum.checked_add(*contribution)
            })
            .expect("reconstructed APNG bytes");
        assert_eq!(reconstructed, full);

        let sparse_positions = [3, 11, 17].into_iter().collect::<BTreeSet<_>>();
        let sparse = measure_sampled_apng_parts(&sequence, &sparse_positions, &context)
            .expect("nonadjacent transition parts");
        assert_eq!(sparse.global_and_first_bytes, all.global_and_first_bytes);
        assert_eq!(
            sparse.transition_bytes.keys().copied().collect::<Vec<_>>(),
            vec![3, 11, 17]
        );
        for position in sparse_positions {
            assert_eq!(
                sparse.transition_bytes.get(&position),
                all.transition_bytes.get(&position),
                "position {position} must use its actual sequence predecessor"
            );
        }
    }

    #[test]
    fn shared_apng_session_rejects_finish_before_the_mandatory_first_idat() {
        let writer = CountingWriter::new(io::sink());
        let session =
            crate::NativeApngWriteSession::begin(writer, 1, 1, 1, &[10_000], "standard", false)
                .expect("partial APNG session header");
        assert!(session.finish().is_err());
    }

    #[test]
    fn proxy_extremes_are_deterministic_and_residual_sampling_is_disjoint() {
        let mut frames = vec![RgbaImage::from_pixel(96, 96, Rgba([0, 0, 0, 255])); 16];
        frames[4] = RgbaImage::from_pixel(96, 96, Rgba([255, 255, 255, 255]));
        frames[5] = RgbaImage::from_pixel(96, 96, Rgba([0, 0, 0, 255]));
        let fixture = CandidateFixture::new(frames);
        let context = OperationContext::detached(Duration::from_secs(5));
        let sequence = fixture.sequence(&context);
        let proxies = transition_proxy_scores(&sequence, &context).expect("transition proxies");
        assert_eq!(proxies.iter().filter(|(_, proxy)| *proxy == 0).count(), 13);
        assert_eq!(proxies.iter().filter(|(_, proxy)| *proxy > 0).count(), 2);
        let extremes =
            select_extreme_transition_positions(&sequence, &context, 2).expect("proxy extremes");
        assert_eq!(extremes, vec![4, 5]);

        let residual = (1..sequence.frames.len())
            .filter(|position| !extremes.contains(position))
            .collect::<Vec<_>>();
        let sampled = sample_positions_without_replacement(residual.len(), 9, 29)
            .expect("residual sample")
            .into_iter()
            .map(|rank| residual[rank])
            .collect::<BTreeSet<_>>();
        assert!(sampled.iter().all(|position| !extremes.contains(position)));
    }

    #[test]
    fn proxy_stratified_sampling_preserves_rare_changed_transitions_and_budget() {
        let residual_proxies = (1..=30)
            .map(|position| (position, if position > 20 { 1 } else { 0 }))
            .collect::<Vec<_>>();
        let first = sample_proxy_stratified_positions_without_replacement(
            &residual_proxies,
            9,
            0x0123_4567_89ab_cdef,
        )
        .expect("proxy-stratified sample");
        let repeated = sample_proxy_stratified_positions_without_replacement(
            &residual_proxies,
            9,
            0x0123_4567_89ab_cdef,
        )
        .expect("repeated proxy-stratified sample");
        assert_eq!(first, repeated);
        assert_eq!(first.len(), 9);
        assert_eq!(first.iter().filter(|position| **position > 20).count(), 7);
    }

    #[test]
    fn probe_candidate_exact_matches_full_writer_without_output_storage() {
        let fixture = CandidateFixture::new(moving_square_frames(12));
        let context = OperationContext::detached(Duration::from_secs(5));
        let sequence = fixture.sequence(&context);
        let full = count_full_candidate_apng(&sequence, &context).expect("full APNG count");
        let estimate = estimate_candidate_size(&sequence, 7, &context).expect("short estimate");
        let probe = probe_candidate_size(&sequence, &context).expect("candidate probe");
        assert!(matches!(
            estimate,
            OutputSizeEstimate::ExactCandidate {
                basis: super::ExactCandidateBasis::ExactFullSequence,
                bytes,
                output_frame_count: 12,
                ..
            } if bytes == full
        ));
        assert!(matches!(
            probe,
            OutputSizeEstimate::ExactCandidate {
                basis: super::ExactCandidateBasis::Probe,
                bytes,
                output_frame_count: 12,
                ..
            } if bytes == full
        ));

        let production = include_str!("estimation.rs")
            .rsplit_once("\nuse std::collections")
            .expect("production marker")
            .1;
        assert!(!production.contains("PendingOutput"));
        assert!(!production.contains("File::create"));
        assert!(!production.contains("Vec<u8>"));
    }

    #[test]
    fn thirteen_frames_take_the_first_sampled_branch_with_twelve_measured_parts() {
        let fixture = CandidateFixture::new(moving_square_frames(13));
        let context = OperationContext::detached(Duration::from_secs(5));
        let sequence = fixture.sequence(&context);
        let estimate = estimate_candidate_size(&sequence, 19, &context)
            .expect("thirteen-frame sampled estimate");
        assert!(matches!(
            estimate,
            OutputSizeEstimate::Range {
                measured_contribution_count: 12,
                output_frame_count: 13,
                ..
            }
        ));
    }

    #[test]
    fn estimate_workload_contract_is_bounded_to_five_candidates_and_twelve_parts_each() {
        let fixture = CandidateFixture::new(moving_square_frames(150));
        let context = OperationContext::detached(Duration::from_secs(30));
        let sequence = fixture.sequence(&context);
        let mut workload = EstimateWorkloadCounters::default();
        for sample_seed in 0..5 {
            let estimate = estimate_candidate_size_with_workload(
                &sequence,
                sample_seed,
                &context,
                &mut workload,
            )
            .expect("bounded candidate estimate workload");
            assert!(matches!(
                estimate,
                OutputSizeEstimate::Range {
                    measured_contribution_count: 12,
                    ..
                }
            ));
        }
        assert_eq!(workload.proxy_transforms, 5 * 150);
        assert_eq!(workload.measurement_transforms, 5 * 23);
        assert_eq!(workload.sampled_transform_count(), 865);
        assert_eq!(workload.compressed_contributions, 5 * 12);
        assert_eq!(workload.peak_scaled_rgba_frames, 2);
        assert_eq!(workload.full_sequence_pixel_vectors, 0);
        assert!(fixture.prepared.decoded_bytes <= crate::frame_source::MAX_PREPARED_SEARCH_BYTES);

        let mut probe_workload = EstimateWorkloadCounters::default();
        probe_candidate_size_with_workload(&sequence, &context, &mut probe_workload)
            .expect("streaming probe workload");
        assert_eq!(probe_workload.full_encode_transforms, 150);
        assert_eq!(probe_workload.peak_scaled_rgba_frames, 2);
        assert_eq!(probe_workload.full_sequence_pixel_vectors, 0);

        let estimator = include_str!("estimation.rs");
        let command = include_str!("lib.rs");
        let preparation = command
            .split_once("fn prepare_candidate_estimation_source(")
            .expect("candidate preparation helper")
            .1
            .split_once("fn estimate_optimizer_candidates_with_loader(")
            .expect("candidate estimate helper")
            .0;
        let candidate_loop = command
            .split_once("fn estimate_optimizer_candidates_with_loader(")
            .expect("candidate estimate helper")
            .1
            .split_once("fn probe_optimizer_candidate_with_loader(")
            .expect("candidate probe helper")
            .0;
        assert_eq!(preparation.matches("loader.prepare(").count(), 1);
        assert_eq!(candidate_loop.matches("loader.prepare(").count(), 0);
        let estimate_source_recheck = candidate_loop
            .find("checkpointed_source_check(context, preflight.identity()")
            .expect("estimate source recheck");
        let estimate_sync = candidate_loop
            .find("synchronize_candidate_estimation_plan(")
            .expect("estimate duration synchronization");
        let estimate_revalidation = candidate_loop
            .find("if request.candidate_ids.iter().any(")
            .expect("estimate candidate revalidation");
        assert!(estimate_source_recheck < estimate_sync);
        assert!(estimate_sync < estimate_revalidation);

        let probe_loop = command
            .split_once("fn probe_optimizer_candidate_with_loader(")
            .expect("candidate probe helper")
            .1
            .split_once("fn encode_prepared_candidate_with_checkpoint(")
            .expect("candidate encode helper")
            .0;
        let probe_source_recheck = probe_loop
            .find("checkpointed_source_check(context, preflight.identity()")
            .expect("probe source recheck");
        let probe_sync = probe_loop
            .find("synchronize_candidate_estimation_plan(")
            .expect("probe duration synchronization");
        let probe_revalidation = probe_loop
            .find("let candidate = plan")
            .expect("probe candidate revalidation");
        assert!(probe_source_recheck < probe_sync);
        assert!(probe_sync < probe_revalidation);

        let full_count = estimator
            .split_once("fn count_full_candidate_apng_with_workload(")
            .expect("full count helper")
            .1
            .split_once("pub(crate) fn estimate_candidate_size(")
            .expect("estimate helper")
            .0;
        let probe = estimator
            .split_once("pub(crate) fn probe_candidate_size_with_workload(")
            .expect("probe helper")
            .1
            .split_once("pub(crate) fn estimate_static_png(")
            .expect("static estimate helper")
            .0;
        for block in [full_count, probe] {
            assert!(!block.contains("Vec<RgbaImage>"));
            assert!(!block.contains("Vec<StickerFrame>"));
            assert!(!block.contains("Vec<u8>"));
            assert!(!block.contains("collect::<Vec"));
        }
        assert!(command.contains("candidate_ids.len() > 5"));
        assert!(estimator.contains("select_extreme_transition_positions(sequence, context, 2)"));
        assert!(estimator.contains(
            "sample_proxy_stratified_positions_without_replacement(&residual_proxies, 9"
        ));
        assert!(estimator.contains("1 + measured_positions.len()"));
        assert!(command.contains("let loader = DefaultFrameSourceLoader;"));
        assert!(command.contains("estimate_optimizer_candidates_with_loader("));
        assert!(command.contains("probe_optimizer_candidate_with_loader("));
        assert!(command.contains("estimate_optimizer_candidates,"));
        assert!(command.contains("probe_optimizer_candidate_size,"));
        let oversized_request = crate::OptimizerSizeEstimateRequest {
            input_path: "input.gif".into(),
            source_revision: "revision".into(),
            candidate_ids: ["a", "b", "c", "d", "e", "f"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            sample_seed: "0123456789abcdef".into(),
            plan: crate::OptimizerPlanRequest {
                locale: None,
                source_duration_seconds: None,
                input_width: None,
                input_height: None,
                avg_fps: None,
                fit_mode: None,
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: None,
                base_frame_count: None,
                timeline_frames: None,
            },
        };
        assert_eq!(
            crate::validate_candidate_ids(&oversized_request.candidate_ids),
            Err(PipelineError::InvalidRequestWithoutReason)
        );
    }

    #[test]
    fn generated_estimate_fixture_calibration_and_holdout_contracts_are_authored() {
        for calibration in [
            "solid",
            "moving-square",
            "alternating-colors",
            "alpha-gradient-motion",
            "seeded-entropy",
            "mixed-entropy",
        ] {
            let fixture = CandidateFixture::new(generated_estimate_fixture_frames(calibration, 11));
            let context = OperationContext::detached(Duration::from_secs(30));
            let sequence = fixture.sequence(&context);
            let all_positions = (1..sequence.frames.len()).collect::<BTreeSet<_>>();
            let all = measure_sampled_apng_parts(&sequence, &all_positions, &context)
                .expect("calibration all-transition parts");
            let reconstructed = all
                .transition_bytes
                .values()
                .try_fold(all.global_and_first_bytes, |sum, contribution| {
                    sum.checked_add(*contribution)
                })
                .expect("calibration reconstructed bytes");
            assert_eq!(
                reconstructed,
                count_full_candidate_apng(&sequence, &context)
                    .expect("calibration full common writer"),
                "{calibration}"
            );
        }

        let mut absolute_percentage_errors = Vec::new();
        for holdout in [
            "periodic-bursts",
            "single-scene-cut",
            "long-still-short-motion",
            "separate-seed",
        ] {
            let fixture = CandidateFixture::new(generated_estimate_fixture_frames(holdout, 97));
            let context = OperationContext::detached(Duration::from_secs(30));
            let sequence = fixture.sequence(&context);
            let (actual, intervals) = arithmetic_resample_intervals(&sequence, &context);
            let covered = intervals
                .iter()
                .filter(|interval| interval.lower_bytes <= actual && actual <= interval.upper_bytes)
                .count();
            assert!(covered >= 186, "{holdout} coverage was {covered}/200");
            for interval in intervals {
                assert!(interval.lower_bytes <= interval.predicted_bytes);
                assert!(interval.predicted_bytes <= interval.upper_bytes);
                absolute_percentage_errors
                    .push(interval.predicted_bytes.abs_diff(actual) as f64 / actual as f64);
            }
        }
        absolute_percentage_errors.sort_by(f64::total_cmp);
        let median = absolute_percentage_errors[absolute_percentage_errors.len() / 2];
        let p90 = absolute_percentage_errors
            [(absolute_percentage_errors.len() as f64 * 0.9).ceil() as usize - 1];
        assert!(median <= 0.15, "aggregate median APE was {median:.3}");
        assert!(p90 <= 0.25, "aggregate P90 APE was {p90:.3}");
    }

    #[test]
    fn static_estimate_errors_serialize_closed_codes_reasons_and_localized_diagnostics() {
        let source_changed =
            OutputSizeEstimateError::from_pipeline(&PipelineError::SourceChanged, UiLocale::Ko);
        let invalid_crop = OutputSizeEstimateError::from_pipeline(
            &PipelineError::InvalidRequest {
                reason: "invalid-crop",
            },
            UiLocale::En,
        );

        assert_eq!(
            serde_json::to_value(source_changed).expect("source-changed estimate error JSON"),
            json!({
                "errorCode": "source-changed",
                "reasonCode": null,
                "errorMessage": locale::media_pipeline_diagnostic(
                    UiLocale::Ko,
                    "source-changed"
                )
            })
        );
        assert_eq!(
            serde_json::to_value(invalid_crop).expect("invalid-crop estimate error JSON"),
            json!({
                "errorCode": "invalid-request",
                "reasonCode": "invalid-crop",
                "errorMessage": locale::media_pipeline_diagnostic(
                    UiLocale::En,
                    "invalid-request"
                )
            })
        );
    }

    #[test]
    fn static_estimate_counting_writer_rejects_counter_overflow() {
        let counter = ByteCounter::with_value_for_test(u64::MAX);
        let mut writer = CountingWriter::with_counter(io::sink(), counter.clone());

        let error = writer
            .write_all(b"x")
            .expect_err("byte count overflow must be rejected");

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(writer.bytes_written(), u64::MAX);
        assert_eq!(counter.bytes_written(), u64::MAX);
    }

    #[test]
    fn static_estimate_matches_solid_alpha_entropy_edge_crop_and_4k_downscale_outputs() {
        let test_dir = TestDir::new("static-estimate-fixtures");
        let fixtures = [
            (
                "solid.png",
                RgbaImage::from_pixel(640, 360, Rgba([12, 34, 56, 255])),
                None,
            ),
            ("alpha.png", alpha_gradient(640, 360), None),
            ("entropy.png", deterministic_entropy(640, 360), None),
            (
                "edge-crop.png",
                alpha_gradient(640, 360),
                Some(CropRegion {
                    x: 0.75,
                    y: 0.75,
                    width: 0.25,
                    height: 0.25,
                }),
            ),
            (
                "downscale-4k.png",
                RgbaImage::from_pixel(3840, 2160, Rgba([91, 27, 143, 255])),
                None,
            ),
        ];

        for (name, pixels, crop) in fixtures {
            let input = test_dir.path.join(name);
            write_fixture(&input, pixels);
            assert_estimate_matches_conversion(&input, crop.as_ref());
        }
    }

    #[test]
    fn static_estimate_stale_source_conversion_publishes_no_output() {
        let test_dir = TestDir::new("static-estimate-stale");
        let input = test_dir.path.join("stale.png");
        write_fixture(&input, alpha_gradient(96, 96));
        let identity = SourceIdentity::from_path(&input, MediaLimits::default())
            .expect("initial source identity");
        let context = OperationContext::detached(MediaOperationKind::StaticEstimate.timeout());
        estimate_static_png(&input, None, &context, MediaLimits::default())
            .expect("initial estimate");

        write_fixture(&input, deterministic_entropy(97, 96));
        let output_dir = test_dir.path.join("output");
        fs::create_dir(&output_dir).expect("output directory");
        let input_path = input.to_string_lossy().into_owned();
        let output_directory = output_dir.to_string_lossy().into_owned();
        let conversion = convert_static_image_to_png_internal(
            &input_path,
            Some(output_directory.as_str()),
            None,
            UiLocale::En,
            Some(&identity),
        );

        assert!(!conversion.ok);
        assert_eq!(conversion.error_code.as_deref(), Some("source-changed"));
        assert_eq!(fs::read_dir(output_dir).expect("output listing").count(), 0);
    }

    #[test]
    fn static_estimate_command_source_is_managed_and_never_creates_output_state() {
        let source = include_str!("lib.rs");
        let estimator_source = include_str!("estimation.rs");
        let production = estimator_source
            .rsplit_once("\nuse std::io::{self, Write};")
            .expect("estimator production marker")
            .1;
        let start = source
            .find("async fn estimate_static_output_size(")
            .expect("estimate command");
        let tail = &source[start..];
        let end = tail
            .find("\n}\n\n")
            .expect("estimate command closing brace");
        let command = &tail[..end];

        assert!(command.contains("MediaOperationKind::StaticEstimate"));
        assert!(command.contains("operation_id: String"));
        assert!(command.contains("on_progress: Channel<OperationProgress>"));
        assert!(command.contains("State<'_, PipelineState>"));
        assert!(command.contains(".reserve(&operation_id)"));
        assert!(command.contains(".promote(kind, &progress)"));
        assert!(command.contains("run_managed_blocking(managed"));
        assert!(!command.contains("PendingOutput"));
        assert!(!command.contains("resolve_output_directory"));
        assert!(source.contains("estimate_static_output_size,"));
        assert!(production.contains("CountingWriter::new(io::sink())"));
        assert!(production.contains("write_native_png(&mut writer, &output_pixels)"));
        assert!(!production.contains("PendingOutput"));
        assert!(!production.contains("Vec<u8>"));
        assert!(!production.contains("File::create"));
        assert!(!production.contains("fs::create_dir"));
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rand::rngs::StdRng;
use rand::seq::index;
use rand::SeedableRng;
use serde::{Deserialize, Serialize};

use crate::frame_source::PreparedCandidateSequence;
use crate::locale::UiLocale;
use crate::media_error::{MediaOperationErrorCode, MediaOperationReasonCode, PipelineError};
use crate::media_limits::{MediaLimits, SourceIdentity};
use crate::operation::{publish_progress, OperationContext, ProgressSink, ProgressStage};
use crate::{
    changed_frame_region, checkpointed_source_check, decode_still_rgba_image,
    pipeline_error_diagnostic, resolve_crop_region, scale_prepared_frame_for_candidate,
    transform_frame_for_static_png, write_native_png, CropRegion, NativeApngWriteSession,
    DISCORD_MAX_STICKER_BYTES,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StaticSizeEstimateRequest {
    pub(crate) input_path: String,
    pub(crate) source_revision: String,
    pub(crate) locale: String,
    pub(crate) crop_region: Option<CropRegion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ExactStaticBasis {
    ExactStatic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ExactCandidateBasis {
    ExactFullSequence,
    Probe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SampledBasis {
    Sampled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EstimateConfidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum OutputSizeEstimate {
    ExactStatic {
        basis: ExactStaticBasis,
        bytes: u64,
        candidate_id: (),
        limit_bytes: u64,
        output_frame_count: u32,
    },
    ExactCandidate {
        basis: ExactCandidateBasis,
        bytes: u64,
        candidate_id: String,
        limit_bytes: u64,
        output_frame_count: u32,
    },
    Range {
        basis: SampledBasis,
        lower_bytes: u64,
        predicted_bytes: u64,
        upper_bytes: u64,
        confidence: EstimateConfidence,
        candidate_id: String,
        limit_bytes: u64,
        measured_contribution_count: u32,
        output_frame_count: u32,
    },
}

impl OutputSizeEstimate {
    pub(crate) fn exact_static(bytes: u64) -> Self {
        Self::ExactStatic {
            basis: ExactStaticBasis::ExactStatic,
            bytes,
            candidate_id: (),
            limit_bytes: DISCORD_MAX_STICKER_BYTES,
            output_frame_count: 1,
        }
    }

    pub(crate) fn exact_candidate(
        candidate_id: String,
        bytes: u64,
        output_frame_count: u32,
    ) -> Self {
        Self::exact_candidate_with_basis(
            ExactCandidateBasis::ExactFullSequence,
            candidate_id,
            bytes,
            output_frame_count,
        )
    }

    pub(crate) fn exact_candidate_with_basis(
        basis: ExactCandidateBasis,
        candidate_id: String,
        bytes: u64,
        output_frame_count: u32,
    ) -> Self {
        Self::ExactCandidate {
            basis,
            bytes,
            candidate_id,
            limit_bytes: DISCORD_MAX_STICKER_BYTES,
            output_frame_count,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sampled_range(
        candidate_id: String,
        lower_bytes: u64,
        predicted_bytes: u64,
        upper_bytes: u64,
        confidence: EstimateConfidence,
        measured_contribution_count: u32,
        output_frame_count: u32,
    ) -> Self {
        Self::Range {
            basis: SampledBasis::Sampled,
            lower_bytes,
            predicted_bytes,
            upper_bytes,
            confidence,
            candidate_id,
            limit_bytes: DISCORD_MAX_STICKER_BYTES,
            measured_contribution_count,
            output_frame_count,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutputSizeEstimateError {
    pub(crate) error_code: MediaOperationErrorCode,
    pub(crate) reason_code: Option<MediaOperationReasonCode>,
    pub(crate) error_message: String,
}

impl OutputSizeEstimateError {
    pub(crate) fn from_pipeline(error: &PipelineError, locale: UiLocale) -> Self {
        Self {
            error_code: error.error_code(),
            reason_code: error.media_reason_code(),
            error_message: pipeline_error_diagnostic(error, locale),
        }
    }

    pub(crate) fn new(
        error_code: MediaOperationErrorCode,
        reason_code: Option<MediaOperationReasonCode>,
        error_message: String,
    ) -> Self {
        Self {
            error_code,
            reason_code,
            error_message,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct ByteCounter {
    bytes_written: Arc<AtomicU64>,
}

impl ByteCounter {
    pub(crate) fn bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::Relaxed)
    }

    fn checked_add(&self, additional: u64) -> io::Result<()> {
        self.bytes_written
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(additional)
            })
            .map(|_| ())
            .map_err(|_| io::Error::other("encoded byte count overflow"))
    }

    #[cfg(test)]
    fn with_value_for_test(value: u64) -> Self {
        Self {
            bytes_written: Arc::new(AtomicU64::new(value)),
        }
    }
}

pub(crate) struct CountingWriter<W> {
    pub(crate) inner: W,
    counter: ByteCounter,
}

impl<W> CountingWriter<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self::with_counter(inner, ByteCounter::default())
    }

    pub(crate) fn with_counter(inner: W, counter: ByteCounter) -> Self {
        Self { inner, counter }
    }

    pub(crate) fn bytes_written(&self) -> u64 {
        self.counter.bytes_written()
    }

    pub(crate) fn counter(&self) -> ByteCounter {
        self.counter.clone()
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.counter.checked_add(written as u64)?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ByteInterval {
    pub(crate) lower_bytes: u64,
    pub(crate) predicted_bytes: u64,
    pub(crate) upper_bytes: u64,
    pub(crate) confidence: EstimateConfidence,
}

fn invalid_estimate_request() -> PipelineError {
    PipelineError::InvalidRequestWithoutReason
}

pub(crate) fn parse_sample_seed(sample_seed: &str) -> Result<u64, PipelineError> {
    if sample_seed.len() != 16
        || !sample_seed
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(invalid_estimate_request());
    }
    u64::from_str_radix(sample_seed, 16).map_err(|_| invalid_estimate_request())
}

pub(crate) fn sample_positions_without_replacement(
    population: usize,
    sample_limit: usize,
    seed: u64,
) -> Result<Vec<usize>, PipelineError> {
    if population == 0 {
        return Ok(Vec::new());
    }
    if sample_limit == 0 {
        return Err(invalid_estimate_request());
    }
    if sample_limit >= population {
        return Ok((0..population).collect());
    }

    let mut random = StdRng::seed_from_u64(seed);
    let mut positions = index::sample(&mut random, population, sample_limit).into_vec();
    positions.sort_unstable();
    Ok(positions)
}

fn unchanged_proxy_stratum_sample_count(
    unchanged_population: usize,
    changed_population: usize,
    sample_limit: usize,
) -> Result<usize, PipelineError> {
    let population = unchanged_population
        .checked_add(changed_population)
        .ok_or_else(invalid_estimate_request)?;
    if unchanged_population == 0
        || changed_population == 0
        || sample_limit < 4
        || sample_limit >= population
    {
        return Err(invalid_estimate_request());
    }

    // Identical-frame transitions are the low-variance stratum, so retain the
    // two observations needed to estimate its variance and spend the remaining
    // budget on changed-frame tail risk. If that tail is smaller than its
    // allocation, measure it exhaustively and return the spare slots here.
    let unchanged_minimum = unchanged_population.min(2);
    let changed_minimum = changed_population.min(2);
    let lower = unchanged_minimum.max(sample_limit.saturating_sub(changed_population));
    let upper = unchanged_population.min(
        sample_limit
            .checked_sub(changed_minimum)
            .ok_or_else(invalid_estimate_request)?,
    );
    if lower > upper {
        return Err(invalid_estimate_request());
    }
    Ok(lower)
}

fn sample_proxy_stratified_positions_without_replacement(
    residual_proxies: &[(usize, u64)],
    sample_limit: usize,
    seed: u64,
) -> Result<BTreeSet<usize>, PipelineError> {
    if residual_proxies.is_empty() {
        return Ok(BTreeSet::new());
    }
    if sample_limit == 0 {
        return Err(invalid_estimate_request());
    }
    if sample_limit >= residual_proxies.len() {
        return Ok(residual_proxies
            .iter()
            .map(|(position, _)| *position)
            .collect());
    }

    let unchanged = residual_proxies
        .iter()
        .filter(|(_, proxy)| *proxy == 0)
        .map(|(position, _)| *position)
        .collect::<Vec<_>>();
    let changed = residual_proxies
        .iter()
        .filter(|(_, proxy)| *proxy != 0)
        .map(|(position, _)| *position)
        .collect::<Vec<_>>();
    if unchanged.is_empty() || changed.is_empty() || sample_limit < 4 {
        return sample_positions_without_replacement(residual_proxies.len(), sample_limit, seed)
            .map(|ranks| {
                ranks
                    .into_iter()
                    .map(|rank| residual_proxies[rank].0)
                    .collect()
            });
    }

    let unchanged_sample_count =
        unchanged_proxy_stratum_sample_count(unchanged.len(), changed.len(), sample_limit)?;
    let changed_sample_count = sample_limit
        .checked_sub(unchanged_sample_count)
        .ok_or_else(invalid_estimate_request)?;
    let mut selected = BTreeSet::new();
    for rank in sample_positions_without_replacement(
        unchanged.len(),
        unchanged_sample_count,
        seed ^ 0x6a09_e667_f3bc_c909,
    )? {
        selected.insert(unchanged[rank]);
    }
    for rank in sample_positions_without_replacement(
        changed.len(),
        changed_sample_count,
        seed.rotate_left(29) ^ 0xbb67_ae85_84ca_a73b,
    )? {
        selected.insert(changed[rank]);
    }
    if selected.len() != sample_limit {
        return Err(invalid_estimate_request());
    }
    Ok(selected)
}

pub(crate) fn confidence_for_interval(
    lower_bytes: u64,
    predicted_bytes: u64,
    upper_bytes: u64,
) -> EstimateConfidence {
    if predicted_bytes == 0 {
        return if lower_bytes == 0 && upper_bytes == 0 {
            EstimateConfidence::High
        } else {
            EstimateConfidence::Low
        };
    }
    let width = upper_bytes.saturating_sub(lower_bytes) as f64;
    let relative_half_width = width / (2.0 * predicted_bytes as f64);
    if relative_half_width <= 0.25 {
        EstimateConfidence::High
    } else if relative_half_width <= 0.75 {
        EstimateConfidence::Medium
    } else {
        EstimateConfidence::Low
    }
}

fn sampled_variance_with_scale_floor(
    squared_deviation_sum: f64,
    sample_count: f64,
    mean: f64,
) -> Result<(f64, bool), PipelineError> {
    let observed_variance = squared_deviation_sum / (sample_count - 1.0);
    if !observed_variance.is_finite() {
        return Err(invalid_estimate_request());
    }

    // Low observed spread in any incomplete stratum does not prove that the
    // unsampled encoded contributions have no tail. Keep the finite-population
    // estimator conservative with a scale-relative (CV = 1) variance floor.
    // This is a policy guard rather than a distribution-free bound, so callers
    // also report Low confidence whenever the floor is needed.
    let scale = mean.max(1.0);
    let variance_floor = scale * scale;
    if !variance_floor.is_finite() {
        return Err(invalid_estimate_request());
    }
    Ok((
        observed_variance.max(variance_floor),
        observed_variance < variance_floor,
    ))
}

fn sampled_interval_confidence(
    lower_bytes: u64,
    predicted_bytes: u64,
    upper_bytes: u64,
    used_scale_floor: bool,
) -> EstimateConfidence {
    let confidence = confidence_for_interval(lower_bytes, predicted_bytes, upper_bytes);
    if used_scale_floor {
        EstimateConfidence::Low
    } else {
        confidence
    }
}

fn conservative_t_critical_value(degrees_of_freedom: usize) -> Result<f64, PipelineError> {
    match degrees_of_freedom {
        0 => Err(invalid_estimate_request()),
        1 => Ok(12.706),
        2 => Ok(4.303),
        3 => Ok(3.182),
        4 => Ok(2.776),
        5 => Ok(2.571),
        6 => Ok(2.447),
        7 => Ok(2.365),
        _ => Ok(2.306),
    }
}

fn finite_u64(value: f64, rounding: fn(f64) -> f64) -> Result<u64, PipelineError> {
    if !value.is_finite() || value < 0.0 || value >= u64::MAX as f64 {
        return Err(invalid_estimate_request());
    }
    let rounded = rounding(value);
    if !rounded.is_finite() || rounded < 0.0 || rounded >= u64::MAX as f64 {
        return Err(invalid_estimate_request());
    }
    Ok(rounded as u64)
}

pub(crate) fn estimate_residual_interval(
    frame_contributions: &[u64],
    population: usize,
    fixed_overhead: u64,
) -> Result<ByteInterval, PipelineError> {
    estimate_residual_interval_with_scale_floor(frame_contributions, population, fixed_overhead)
}

fn estimate_residual_interval_with_scale_floor(
    frame_contributions: &[u64],
    population: usize,
    fixed_overhead: u64,
) -> Result<ByteInterval, PipelineError> {
    let sampled = frame_contributions.len();
    if sampled > population {
        return Err(invalid_estimate_request());
    }
    if population == 0 {
        if sampled != 0 {
            return Err(invalid_estimate_request());
        }
        return Ok(ByteInterval {
            lower_bytes: fixed_overhead,
            predicted_bytes: fixed_overhead,
            upper_bytes: fixed_overhead,
            confidence: EstimateConfidence::High,
        });
    }
    if sampled == population {
        let total = frame_contributions
            .iter()
            .try_fold(fixed_overhead, |sum, contribution| {
                sum.checked_add(*contribution)
                    .ok_or_else(invalid_estimate_request)
            })?;
        return Ok(ByteInterval {
            lower_bytes: total,
            predicted_bytes: total,
            upper_bytes: total,
            confidence: EstimateConfidence::High,
        });
    }
    if sampled < 2 {
        return Err(invalid_estimate_request());
    }

    let sample_count = sampled as f64;
    let population_count = population as f64;
    if !sample_count.is_finite() || !population_count.is_finite() {
        return Err(invalid_estimate_request());
    }
    let sum = frame_contributions
        .iter()
        .try_fold(0_u64, |sum, contribution| {
            sum.checked_add(*contribution)
                .ok_or_else(invalid_estimate_request)
        })?;
    let mean = sum as f64 / sample_count;
    let squared_deviation_sum = frame_contributions.iter().try_fold(0.0, |total, value| {
        let deviation = *value as f64 - mean;
        let next = total + deviation * deviation;
        next.is_finite()
            .then_some(next)
            .ok_or_else(invalid_estimate_request)
    })?;
    let (variance, used_scale_floor) =
        sampled_variance_with_scale_floor(squared_deviation_sum, sample_count, mean)?;
    let correction = ((population_count - sample_count) / (population_count - 1.0)).sqrt();
    let standard_error = population_count * (variance / sample_count).sqrt() * correction;
    let residual_prediction = population_count * mean;
    let half_width = conservative_t_critical_value(sampled - 1)? * standard_error;
    if !variance.is_finite()
        || !correction.is_finite()
        || !standard_error.is_finite()
        || !residual_prediction.is_finite()
        || !half_width.is_finite()
    {
        return Err(invalid_estimate_request());
    }

    let predicted_residual = finite_u64(residual_prediction, f64::round)?;
    let lower_residual = finite_u64((residual_prediction - half_width).max(0.0), f64::floor)?;
    let upper_residual = finite_u64(residual_prediction + half_width, f64::ceil)?;
    let predicted_bytes = fixed_overhead
        .checked_add(predicted_residual)
        .ok_or_else(invalid_estimate_request)?;
    let lower_bytes = fixed_overhead
        .checked_add(lower_residual)
        .ok_or_else(invalid_estimate_request)?;
    let upper_bytes = fixed_overhead
        .checked_add(upper_residual)
        .ok_or_else(invalid_estimate_request)?;
    if lower_bytes > predicted_bytes || predicted_bytes > upper_bytes {
        return Err(invalid_estimate_request());
    }
    Ok(ByteInterval {
        lower_bytes,
        predicted_bytes,
        upper_bytes,
        confidence: sampled_interval_confidence(
            lower_bytes,
            predicted_bytes,
            upper_bytes,
            used_scale_floor,
        ),
    })
}

fn estimate_proxy_stratified_residual_interval(
    residual_proxies: &[(usize, u64)],
    sampled_contributions: &BTreeMap<usize, u64>,
    fixed_overhead: u64,
) -> Result<ByteInterval, PipelineError> {
    if residual_proxies.is_empty() {
        return estimate_residual_interval(&[], 0, fixed_overhead);
    }

    let mut unchanged_population = 0_usize;
    let mut changed_population = 0_usize;
    let mut unchanged_samples = Vec::new();
    let mut changed_samples = Vec::new();
    for (position, proxy) in residual_proxies {
        let (population, samples) = if *proxy == 0 {
            (&mut unchanged_population, &mut unchanged_samples)
        } else {
            (&mut changed_population, &mut changed_samples)
        };
        *population = population
            .checked_add(1)
            .ok_or_else(invalid_estimate_request)?;
        if let Some(contribution) = sampled_contributions.get(position) {
            samples.push(*contribution);
        }
    }
    if unchanged_samples.len() + changed_samples.len() != sampled_contributions.len() {
        return Err(invalid_estimate_request());
    }
    if unchanged_population == 0 || changed_population == 0 {
        let samples = if unchanged_population == 0 {
            &changed_samples
        } else {
            &unchanged_samples
        };
        return estimate_residual_interval_with_scale_floor(
            samples,
            residual_proxies.len(),
            fixed_overhead,
        );
    }

    let strata = [
        (unchanged_population, unchanged_samples.as_slice()),
        (changed_population, changed_samples.as_slice()),
    ];
    let mut residual_prediction = 0.0_f64;
    let mut residual_variance = 0.0_f64;
    let mut minimum_degrees_of_freedom = usize::MAX;
    let mut used_scale_floor = false;
    for (population, samples) in strata {
        let sampled = samples.len();
        if sampled == 0 || sampled > population || (sampled < population && sampled < 2) {
            return Err(invalid_estimate_request());
        }
        let sample_count = sampled as f64;
        let population_count = population as f64;
        let sum = samples.iter().try_fold(0_u64, |total, contribution| {
            total
                .checked_add(*contribution)
                .ok_or_else(invalid_estimate_request)
        })?;
        let mean = sum as f64 / sample_count;
        let stratum_prediction = population_count * mean;
        if !mean.is_finite() || !stratum_prediction.is_finite() {
            return Err(invalid_estimate_request());
        }
        residual_prediction += stratum_prediction;

        if sampled < population {
            let squared_deviation_sum = samples.iter().try_fold(0.0, |total, value| {
                let deviation = *value as f64 - mean;
                let next = total + deviation * deviation;
                next.is_finite()
                    .then_some(next)
                    .ok_or_else(invalid_estimate_request)
            })?;
            let (sample_variance, stratum_used_scale_floor) =
                sampled_variance_with_scale_floor(squared_deviation_sum, sample_count, mean)?;
            let finite_population_correction =
                (population_count - sample_count) / (population_count - 1.0);
            let stratum_variance = population_count * population_count * sample_variance
                / sample_count
                * finite_population_correction;
            if !sample_variance.is_finite()
                || !finite_population_correction.is_finite()
                || !stratum_variance.is_finite()
            {
                return Err(invalid_estimate_request());
            }
            residual_variance += stratum_variance;
            used_scale_floor |= stratum_used_scale_floor;
            minimum_degrees_of_freedom = minimum_degrees_of_freedom.min(sampled - 1);
        }
    }
    if !residual_prediction.is_finite() || !residual_variance.is_finite() {
        return Err(invalid_estimate_request());
    }

    // A lower-bound Welch degree of freedom keeps the combined interval
    // conservative without requiring another pass over the encoded frames.
    let critical_value = conservative_t_critical_value(minimum_degrees_of_freedom)?;
    let half_width = critical_value * residual_variance.sqrt();
    if !half_width.is_finite() {
        return Err(invalid_estimate_request());
    }

    let predicted_residual = finite_u64(residual_prediction, f64::round)?;
    let lower_residual = finite_u64((residual_prediction - half_width).max(0.0), f64::floor)?;
    let upper_residual = finite_u64(residual_prediction + half_width, f64::ceil)?;
    let predicted_bytes = fixed_overhead
        .checked_add(predicted_residual)
        .ok_or_else(invalid_estimate_request)?;
    let lower_bytes = fixed_overhead
        .checked_add(lower_residual)
        .ok_or_else(invalid_estimate_request)?;
    let upper_bytes = fixed_overhead
        .checked_add(upper_residual)
        .ok_or_else(invalid_estimate_request)?;
    if lower_bytes > predicted_bytes || predicted_bytes > upper_bytes {
        return Err(invalid_estimate_request());
    }
    Ok(ByteInterval {
        lower_bytes,
        predicted_bytes,
        upper_bytes,
        confidence: sampled_interval_confidence(
            lower_bytes,
            predicted_bytes,
            upper_bytes,
            used_scale_floor,
        ),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeasuredApngParts {
    pub(crate) global_and_first_bytes: u64,
    pub(crate) transition_bytes: BTreeMap<usize, u64>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EstimateWorkloadCounters {
    pub(crate) proxy_transforms: usize,
    pub(crate) measurement_transforms: usize,
    pub(crate) full_encode_transforms: usize,
    pub(crate) compressed_contributions: usize,
    pub(crate) peak_scaled_rgba_frames: usize,
    pub(crate) full_sequence_pixel_vectors: usize,
}

impl EstimateWorkloadCounters {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn sampled_transform_count(self) -> usize {
        self.proxy_transforms + self.measurement_transforms
    }
}

fn sequence_durations(
    sequence: &PreparedCandidateSequence<'_, '_>,
) -> Result<Vec<u64>, PipelineError> {
    if sequence.frames.is_empty() {
        return Err(PipelineError::InvalidRequest {
            reason: "no-frames-selected",
        });
    }
    Ok(sequence
        .frames
        .iter()
        .map(|frame| frame.duration_us)
        .collect())
}

fn scaled_sequence_frame(
    sequence: &PreparedCandidateSequence<'_, '_>,
    position: usize,
) -> Result<image::RgbaImage, PipelineError> {
    let frame = sequence
        .frames
        .get(position)
        .ok_or_else(invalid_estimate_request)?;
    let source = sequence
        .source
        .pixels_for_source_index(frame.source_frame_index)?;
    Ok(scale_prepared_frame_for_candidate(
        source.as_ref(),
        sequence.candidate.content_scale,
    ))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn select_extreme_transition_positions(
    sequence: &PreparedCandidateSequence<'_, '_>,
    context: &OperationContext,
    limit: usize,
) -> Result<Vec<usize>, PipelineError> {
    let proxies = transition_proxy_scores_with_workload(
        sequence,
        context,
        &mut EstimateWorkloadCounters::default(),
    )?;
    Ok(select_extreme_transition_positions_from_proxies(
        &proxies, limit,
    ))
}

#[cfg_attr(not(test), allow(dead_code))]
fn transition_proxy_scores(
    sequence: &PreparedCandidateSequence<'_, '_>,
    context: &OperationContext,
) -> Result<Vec<(usize, u64)>, PipelineError> {
    transition_proxy_scores_with_workload(
        sequence,
        context,
        &mut EstimateWorkloadCounters::default(),
    )
}

fn transition_proxy_scores_with_workload(
    sequence: &PreparedCandidateSequence<'_, '_>,
    context: &OperationContext,
    workload: &mut EstimateWorkloadCounters,
) -> Result<Vec<(usize, u64)>, PipelineError> {
    context.checkpoint()?;
    let frame_count = sequence.frames.len();
    if frame_count <= 1 {
        return Ok(Vec::new());
    }
    let mut previous = scaled_sequence_frame(sequence, 0)?;
    workload.proxy_transforms += 1;
    workload.peak_scaled_rgba_frames = workload.peak_scaled_rgba_frames.max(1);
    context.checkpoint()?;
    let mut proxies = Vec::with_capacity(frame_count - 1);
    for position in 1..frame_count {
        context.checkpoint()?;
        let current = scaled_sequence_frame(sequence, position)?;
        workload.proxy_transforms += 1;
        workload.peak_scaled_rgba_frames = workload.peak_scaled_rgba_frames.max(2);
        let region = changed_frame_region(&previous, &current);
        let region_area = u64::from(region.width)
            .checked_mul(u64::from(region.height))
            .ok_or_else(invalid_estimate_request)?;
        let changed_bytes = previous
            .as_raw()
            .iter()
            .zip(current.as_raw())
            .filter(|(left, right)| left != right)
            .try_fold(0_u64, |count, _| {
                count.checked_add(1).ok_or_else(invalid_estimate_request)
            })?;
        let proxy = if changed_bytes == 0 {
            0
        } else {
            region_area
                .checked_add(changed_bytes)
                .ok_or_else(invalid_estimate_request)?
        };
        proxies.push((position, proxy));
        previous = current;
    }
    Ok(proxies)
}

fn select_extreme_transition_positions_from_proxies(
    proxies: &[(usize, u64)],
    limit: usize,
) -> Vec<usize> {
    let mut ranked = proxies.to_vec();
    ranked.sort_by(
        |(left_position, left_proxy), (right_position, right_proxy)| {
            right_proxy
                .cmp(left_proxy)
                .then_with(|| left_position.cmp(right_position))
        },
    );
    let mut selected = ranked
        .into_iter()
        .take(limit.min(proxies.len()))
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    selected.sort_unstable();
    selected
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn measure_sampled_apng_parts(
    sequence: &PreparedCandidateSequence<'_, '_>,
    measured_transition_positions: &BTreeSet<usize>,
    context: &OperationContext,
) -> Result<MeasuredApngParts, PipelineError> {
    measure_sampled_apng_parts_with_workload(
        sequence,
        measured_transition_positions,
        context,
        &mut EstimateWorkloadCounters::default(),
    )
}

fn measure_sampled_apng_parts_with_workload(
    sequence: &PreparedCandidateSequence<'_, '_>,
    measured_transition_positions: &BTreeSet<usize>,
    context: &OperationContext,
    workload: &mut EstimateWorkloadCounters,
) -> Result<MeasuredApngParts, PipelineError> {
    context.checkpoint()?;
    let frame_count = sequence.frames.len();
    if measured_transition_positions
        .iter()
        .any(|position| *position == 0 || *position >= frame_count)
    {
        return Err(invalid_estimate_request());
    }
    let durations = sequence_durations(sequence)?;
    let first = scaled_sequence_frame(sequence, 0)?;
    workload.measurement_transforms += 1;
    workload.peak_scaled_rgba_frames = workload.peak_scaled_rgba_frames.max(1);
    context.checkpoint()?;
    let writer = CountingWriter::new(io::sink());
    let counter = writer.counter();
    let mut session = NativeApngWriteSession::begin(
        writer,
        first.width(),
        first.height(),
        frame_count,
        &durations,
        &sequence.candidate.preset,
        false,
    )?;
    context.checkpoint()?;
    session.write_first(&first)?;
    workload.compressed_contributions += 1;
    context.checkpoint()?;
    drop(first);
    let mut transition_bytes = BTreeMap::new();
    for position in measured_transition_positions {
        context.checkpoint()?;
        let previous = scaled_sequence_frame(sequence, position - 1)?;
        let current = scaled_sequence_frame(sequence, *position)?;
        workload.measurement_transforms += 2;
        workload.peak_scaled_rgba_frames = workload.peak_scaled_rgba_frames.max(2);
        let before = counter.bytes_written();
        session.write_transition(*position, &previous, &current)?;
        workload.compressed_contributions += 1;
        let contribution = counter
            .bytes_written()
            .checked_sub(before)
            .ok_or_else(invalid_estimate_request)?;
        transition_bytes.insert(*position, contribution);
    }
    context.checkpoint()?;
    session.finish()?;
    context.checkpoint()?;
    let selected_sum = transition_bytes.values().try_fold(0_u64, |sum, value| {
        sum.checked_add(*value).ok_or_else(invalid_estimate_request)
    })?;
    let global_and_first_bytes = counter
        .bytes_written()
        .checked_sub(selected_sum)
        .ok_or_else(invalid_estimate_request)?;
    Ok(MeasuredApngParts {
        global_and_first_bytes,
        transition_bytes,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn count_full_candidate_apng(
    sequence: &PreparedCandidateSequence<'_, '_>,
    context: &OperationContext,
) -> Result<u64, PipelineError> {
    count_full_candidate_apng_with_workload(
        sequence,
        context,
        &mut EstimateWorkloadCounters::default(),
    )
}

fn count_full_candidate_apng_with_workload(
    sequence: &PreparedCandidateSequence<'_, '_>,
    context: &OperationContext,
    workload: &mut EstimateWorkloadCounters,
) -> Result<u64, PipelineError> {
    context.checkpoint()?;
    let frame_count = sequence.frames.len();
    let durations = sequence_durations(sequence)?;
    let first = scaled_sequence_frame(sequence, 0)?;
    workload.full_encode_transforms += 1;
    workload.peak_scaled_rgba_frames = workload.peak_scaled_rgba_frames.max(1);
    context.checkpoint()?;
    let writer = CountingWriter::new(io::sink());
    let counter = writer.counter();
    let mut session = NativeApngWriteSession::begin(
        writer,
        first.width(),
        first.height(),
        frame_count,
        &durations,
        &sequence.candidate.preset,
        true,
    )?;
    context.checkpoint()?;
    session.write_first(&first)?;
    workload.compressed_contributions += 1;
    context.checkpoint()?;
    let mut previous = first;
    for position in 1..frame_count {
        context.checkpoint()?;
        let current = scaled_sequence_frame(sequence, position)?;
        workload.full_encode_transforms += 1;
        workload.peak_scaled_rgba_frames = workload.peak_scaled_rgba_frames.max(2);
        session.write_transition(position, &previous, &current)?;
        workload.compressed_contributions += 1;
        previous = current;
    }
    context.checkpoint()?;
    session.finish()?;
    context.checkpoint()?;
    Ok(counter.bytes_written())
}

pub(crate) fn estimate_candidate_size(
    sequence: &PreparedCandidateSequence<'_, '_>,
    sample_seed: u64,
    context: &OperationContext,
) -> Result<OutputSizeEstimate, PipelineError> {
    estimate_candidate_size_with_workload(
        sequence,
        sample_seed,
        context,
        &mut EstimateWorkloadCounters::default(),
    )
}

pub(crate) fn estimate_candidate_size_with_workload(
    sequence: &PreparedCandidateSequence<'_, '_>,
    sample_seed: u64,
    context: &OperationContext,
    workload: &mut EstimateWorkloadCounters,
) -> Result<OutputSizeEstimate, PipelineError> {
    let frame_count = sequence.frames.len();
    let output_frame_count = u32::try_from(frame_count).map_err(|_| invalid_estimate_request())?;
    let transition_count = frame_count.saturating_sub(1);
    if transition_count <= 11 {
        return Ok(OutputSizeEstimate::exact_candidate(
            sequence.candidate.id.clone(),
            count_full_candidate_apng_with_workload(sequence, context, workload)?,
            output_frame_count,
        ));
    }

    let transition_proxies = transition_proxy_scores_with_workload(sequence, context, workload)?;
    let extremes = select_extreme_transition_positions_from_proxies(&transition_proxies, 2);
    if extremes.len() != 2 {
        return Err(invalid_estimate_request());
    }
    let extreme_set = extremes.iter().copied().collect::<BTreeSet<_>>();
    let residual_proxies = transition_proxies
        .into_iter()
        .filter(|(position, _)| !extreme_set.contains(position))
        .collect::<Vec<_>>();
    let sampled_positions =
        sample_proxy_stratified_positions_without_replacement(&residual_proxies, 9, sample_seed)?;
    if sampled_positions
        .iter()
        .any(|position| extreme_set.contains(position))
    {
        return Err(invalid_estimate_request());
    }
    let measured_positions = extreme_set
        .union(&sampled_positions)
        .copied()
        .collect::<BTreeSet<_>>();
    let measured =
        measure_sampled_apng_parts_with_workload(sequence, &measured_positions, context, workload)?;
    let fixed_bytes =
        extremes
            .iter()
            .try_fold(measured.global_and_first_bytes, |sum, position| {
                sum.checked_add(
                    *measured
                        .transition_bytes
                        .get(position)
                        .ok_or_else(invalid_estimate_request)?,
                )
                .ok_or_else(invalid_estimate_request)
            })?;
    let sampled_contributions = sampled_positions
        .iter()
        .map(|position| {
            measured
                .transition_bytes
                .get(position)
                .copied()
                .map(|contribution| (*position, contribution))
                .ok_or_else(invalid_estimate_request)
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let interval = estimate_proxy_stratified_residual_interval(
        &residual_proxies,
        &sampled_contributions,
        fixed_bytes,
    )?;
    let measured_contribution_count =
        u32::try_from(1 + measured_positions.len()).map_err(|_| invalid_estimate_request())?;
    Ok(OutputSizeEstimate::sampled_range(
        sequence.candidate.id.clone(),
        interval.lower_bytes,
        interval.predicted_bytes,
        interval.upper_bytes,
        interval.confidence,
        measured_contribution_count,
        output_frame_count,
    ))
}

pub(crate) fn probe_candidate_size(
    sequence: &PreparedCandidateSequence<'_, '_>,
    context: &OperationContext,
) -> Result<OutputSizeEstimate, PipelineError> {
    probe_candidate_size_with_workload(sequence, context, &mut EstimateWorkloadCounters::default())
}

pub(crate) fn probe_candidate_size_with_workload(
    sequence: &PreparedCandidateSequence<'_, '_>,
    context: &OperationContext,
    workload: &mut EstimateWorkloadCounters,
) -> Result<OutputSizeEstimate, PipelineError> {
    let output_frame_count =
        u32::try_from(sequence.frames.len()).map_err(|_| invalid_estimate_request())?;
    Ok(OutputSizeEstimate::exact_candidate_with_basis(
        ExactCandidateBasis::Probe,
        sequence.candidate.id.clone(),
        count_full_candidate_apng_with_workload(sequence, context, workload)?,
        output_frame_count,
    ))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn estimate_static_png(
    input_path: &Path,
    crop: Option<&CropRegion>,
    context: &OperationContext,
    limits: MediaLimits,
) -> Result<OutputSizeEstimate, PipelineError> {
    estimate_static_png_with_callbacks(
        input_path,
        crop,
        context,
        limits,
        || context.checkpoint(),
        |_, _, _| {},
    )
}

pub(crate) fn estimate_static_png_with_operation(
    input_path: &Path,
    crop: Option<&CropRegion>,
    expected_source: &SourceIdentity,
    context: &OperationContext,
    limits: MediaLimits,
    progress: &impl ProgressSink,
) -> Result<OutputSizeEstimate, PipelineError> {
    estimate_static_png_with_callbacks(
        input_path,
        crop,
        context,
        limits,
        || checkpointed_source_check(context, expected_source, limits),
        |stage, completed, total| publish_progress(progress, context, stage, completed, total),
    )
}

fn estimate_static_png_with_callbacks(
    input_path: &Path,
    crop: Option<&CropRegion>,
    context: &OperationContext,
    limits: MediaLimits,
    mut source_checkpoint: impl FnMut() -> Result<(), PipelineError>,
    mut progress: impl FnMut(ProgressStage, u32, Option<u32>),
) -> Result<OutputSizeEstimate, PipelineError> {
    source_checkpoint()?;
    progress(ProgressStage::Decoding, 0, Some(1));
    let input_path = input_path.to_string_lossy();
    let source_pixels =
        decode_still_rgba_image(input_path.as_ref(), limits, || context.checkpoint())?;
    source_checkpoint()?;
    progress(ProgressStage::Decoding, 1, Some(1));

    let resolved_crop = resolve_crop_region(
        crop,
        Some(source_pixels.width()),
        Some(source_pixels.height()),
        UiLocale::En,
    )
    .map_err(|_| PipelineError::InvalidRequest {
        reason: "invalid-crop",
    })?;
    let output_pixels = transform_frame_for_static_png(&source_pixels, resolved_crop);
    source_checkpoint()?;

    progress(ProgressStage::Encoding, 0, Some(1));
    let mut writer = CountingWriter::new(io::sink());
    write_native_png(&mut writer, &output_pixels)?;
    context.checkpoint()?;
    source_checkpoint()?;
    progress(ProgressStage::Encoding, 1, Some(1));

    Ok(OutputSizeEstimate::exact_static(writer.bytes_written()))
}
