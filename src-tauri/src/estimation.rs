#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use image::{ImageFormat, Rgba, RgbaImage};
    use serde_json::json;

    use super::{estimate_static_png, CountingWriter, OutputSizeEstimate, OutputSizeEstimateError};
    use crate::locale;
    use crate::media_error::PipelineError;
    use crate::media_limits::{MediaLimits, SourceIdentity};
    use crate::operation::{MediaOperationKind, OperationContext};
    use crate::{convert_static_image_to_png_internal, CropRegion, UiLocale};

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
        writer.write_all(b"abc").expect("first write");
        writer.write_all(b"defgh").expect("second write");
        writer.flush().expect("flush sink");

        assert_eq!(writer.bytes_written(), 8);
        assert_eq!(
            std::mem::size_of_val(&writer),
            std::mem::size_of::<io::Sink>() + std::mem::size_of::<u64>()
        );
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
        let mut writer = CountingWriter {
            inner: io::sink(),
            bytes_written: u64::MAX,
        };

        let error = writer
            .write_all(b"x")
            .expect_err("byte count overflow must be rejected");

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(writer.bytes_written(), u64::MAX);
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

use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::locale::UiLocale;
use crate::media_error::{MediaOperationErrorCode, MediaOperationReasonCode, PipelineError};
use crate::media_limits::{MediaLimits, SourceIdentity};
use crate::operation::{publish_progress, OperationContext, ProgressSink, ProgressStage};
use crate::{
    checkpointed_source_check, decode_still_rgba_image, pipeline_error_diagnostic,
    resolve_crop_region, transform_frame_for_static_png, write_native_png, CropRegion,
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

pub(crate) struct CountingWriter<W> {
    inner: W,
    bytes_written: u64,
}

impl<W> CountingWriter<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self {
            inner,
            bytes_written: 0,
        }
    }

    pub(crate) fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.bytes_written = self
            .bytes_written
            .checked_add(written as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "encoded byte count overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

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
