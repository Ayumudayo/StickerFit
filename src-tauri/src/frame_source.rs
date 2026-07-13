use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use image::RgbaImage;

use crate::locale::UiLocale;
use crate::media_error::PipelineError;
use crate::media_limits::MediaLimits;
use crate::operation::OperationContext;
use crate::{
    CandidatePreview, CropRegion, EditedTimelineFrame, OptimizerPlanResponse, ResolvedTimelineFrame,
};

pub(crate) const MAX_PREPARED_SEARCH_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const MAX_SEARCH_OUTPUT_FRAMES: usize = 150;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimelineTimingAuthority {
    Authored,
    Native,
    Inspected,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedFrame {
    pub(crate) source_frame_id: u32,
    pub(crate) pixels: Arc<RgbaImage>,
    pub(crate) duration_us: u64,
}

#[derive(Debug)]
pub(crate) struct PreparedSearchSource {
    pub(crate) frames: Vec<PreparedFrame>,
    pub(crate) base_sequence: Vec<ResolvedTimelineFrame>,
    pub(crate) timing_authority: TimelineTimingAuthority,
    pub(crate) base_fps: u32,
    pub(crate) decoded_bytes: usize,
    pub(crate) tool_source: String,
    pub(crate) tool_command: Option<String>,
    pub(crate) tool_detail: Option<String>,
}

impl PreparedSearchSource {
    pub(crate) fn pixels_for_source_index(
        &self,
        source_frame_index: u32,
    ) -> Result<&Arc<RgbaImage>, PipelineError> {
        let source_frame_id =
            source_frame_index
                .checked_add(1)
                .ok_or(PipelineError::InvalidRequest {
                    reason: "invalid-frame-selection",
                })?;
        self.frames
            .iter()
            .find(|frame| frame.source_frame_id == source_frame_id)
            .map(|frame| &frame.pixels)
            .ok_or(PipelineError::InvalidRequest {
                reason: "invalid-frame-selection",
            })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FramePreparationRequest<'a> {
    pub(crate) input_path: &'a Path,
    pub(crate) source_revision: &'a str,
    pub(crate) crop_region: Option<&'a CropRegion>,
    pub(crate) input_width: Option<u32>,
    pub(crate) input_height: Option<u32>,
    pub(crate) base_frame_count: Option<u32>,
    pub(crate) timeline_frames: Option<&'a [EditedTimelineFrame]>,
    pub(crate) resolved_timeline_frames: Option<&'a [ResolvedTimelineFrame]>,
    pub(crate) selected_frame_indexes: Option<&'a [u32]>,
    pub(crate) source_duration_seconds: Option<f64>,
    pub(crate) avg_fps: Option<f64>,
    pub(crate) locale: UiLocale,
    pub(crate) optimizer_goal: &'a str,
}

pub(crate) trait FrameSourceLoader: Send + Sync {
    fn prepare(
        &self,
        request: FramePreparationRequest<'_>,
        plan: &OptimizerPlanResponse,
        context: &OperationContext,
        limits: MediaLimits,
    ) -> Result<PreparedSearchSource, PipelineError>;
}

pub(crate) struct PreparedCandidateSequence<'source, 'candidate> {
    pub(crate) source: &'source PreparedSearchSource,
    pub(crate) candidate: &'candidate CandidatePreview,
    pub(crate) frames: Cow<'source, [ResolvedTimelineFrame]>,
    pub(crate) duration_us: u64,
}

fn invalid_frame_selection() -> PipelineError {
    PipelineError::InvalidRequest {
        reason: "invalid-frame-selection",
    }
}

pub(crate) fn checked_timeline_duration(
    frames: &[ResolvedTimelineFrame],
) -> Result<u64, PipelineError> {
    frames.iter().try_fold(0_u64, |total, frame| {
        total
            .checked_add(frame.duration_us)
            .ok_or_else(invalid_frame_selection)
    })
}

pub(crate) fn checked_prepared_bytes(
    current: usize,
    additional: usize,
) -> Result<usize, PipelineError> {
    let actual = current.checked_add(additional).unwrap_or(usize::MAX);
    if actual > MAX_PREPARED_SEARCH_BYTES {
        return Err(PipelineError::LimitExceeded {
            resource: "decoded-bytes",
            limit: MAX_PREPARED_SEARCH_BYTES as u64,
            actual: u64::try_from(actual).unwrap_or(u64::MAX),
        });
    }
    Ok(actual)
}

fn boundary_distance(
    prefix_us: u64,
    bucket_number: usize,
    total_us: u64,
    bucket_count: usize,
) -> u128 {
    let scaled_prefix = u128::from(prefix_us) * bucket_count as u128;
    let scaled_target = u128::from(total_us) * bucket_number as u128;
    scaled_prefix.abs_diff(scaled_target)
}

fn representative_index(
    frames: &[ResolvedTimelineFrame],
    prefix: &[u64],
    start: usize,
    end: usize,
) -> usize {
    let bucket_start = prefix[start];
    let bucket_duration = prefix[end] - bucket_start;
    let bucket_midpoint_twice = u128::from(bucket_start) * 2 + u128::from(bucket_duration);

    (start..end)
        .min_by_key(|index| {
            let frame_start = prefix[*index];
            let frame_midpoint_twice =
                u128::from(frame_start) * 2 + u128::from(frames[*index].duration_us);
            frame_midpoint_twice.abs_diff(bucket_midpoint_twice)
        })
        .unwrap_or(start)
}

pub(crate) fn resample_timeline_to_limit<'a>(
    frames: &'a [ResolvedTimelineFrame],
    max_frames: usize,
) -> Result<Cow<'a, [ResolvedTimelineFrame]>, PipelineError> {
    if frames.is_empty() || max_frames == 0 || (frames.len() > 1 && max_frames == 1) {
        return Err(invalid_frame_selection());
    }

    let total_duration_us = checked_timeline_duration(frames)?;
    if frames.len() <= max_frames {
        return Ok(Cow::Borrowed(frames));
    }

    let mut prefix = Vec::with_capacity(frames.len() + 1);
    prefix.push(0_u64);
    for frame in frames {
        let next = prefix
            .last()
            .copied()
            .unwrap_or_default()
            .checked_add(frame.duration_us)
            .ok_or_else(invalid_frame_selection)?;
        prefix.push(next);
    }

    let mut output = Vec::with_capacity(max_frames);
    let mut start = 0_usize;
    for bucket in 0..max_frames {
        let remaining_buckets = max_frames - bucket - 1;
        let min_end = start + 1;
        let max_end = frames.len() - remaining_buckets;
        let end = if bucket + 1 == max_frames {
            frames.len()
        } else {
            (min_end..=max_end)
                .min_by_key(|candidate| {
                    boundary_distance(
                        prefix[*candidate],
                        bucket + 1,
                        total_duration_us,
                        max_frames,
                    )
                })
                .unwrap_or(min_end)
        };
        let mut representative = representative_index(frames, &prefix, start, end);
        if bucket == 0 {
            representative = 0;
        } else if bucket + 1 == max_frames {
            representative = frames.len() - 1;
        }
        output.push(ResolvedTimelineFrame {
            source_frame_index: frames[representative].source_frame_index,
            duration_us: prefix[end] - prefix[start],
        });
        start = end;
    }

    if output.len() != max_frames || checked_timeline_duration(&output)? != total_duration_us {
        return Err(invalid_frame_selection());
    }
    Ok(Cow::Owned(output))
}

fn interval_for_candidate(
    source: &PreparedSearchSource,
    candidate: &CandidatePreview,
) -> Result<Option<u64>, PipelineError> {
    if candidate.frame_sample_step <= 1 && candidate.fps >= source.base_fps {
        return Ok(None);
    }
    let base_fps = u64::from(source.base_fps.max(1));
    let target_fps = u64::from(candidate.fps.max(1));
    let base_interval_us = (1_000_000_u64 + base_fps / 2) / base_fps;
    let target_interval_us = (1_000_000_u64 + target_fps / 2) / target_fps;
    let step_interval_us = base_interval_us
        .checked_mul(u64::from(candidate.frame_sample_step.max(1)))
        .ok_or_else(invalid_frame_selection)?;
    Ok(Some(target_interval_us.max(step_interval_us)))
}

fn sample_timeline_by_interval(
    frames: &[ResolvedTimelineFrame],
    interval_us: u64,
) -> Result<Vec<ResolvedTimelineFrame>, PipelineError> {
    if frames.len() <= 2 {
        return Ok(frames.to_vec());
    }

    let total_duration_us = checked_timeline_duration(frames)?;
    let mut output = Vec::with_capacity(frames.len());
    output.push(frames[0].clone());
    let mut elapsed_us = frames[0].duration_us;
    let mut next_retain_at = interval_us.max(1);

    for frame in &frames[1..frames.len() - 1] {
        if elapsed_us >= next_retain_at {
            output.push(frame.clone());
            let interval_us = interval_us.max(1);
            let increments = u128::from(elapsed_us - next_retain_at) / u128::from(interval_us) + 1;
            let advanced =
                u128::from(next_retain_at) + increments.saturating_mul(u128::from(interval_us));
            next_retain_at = u64::try_from(advanced).unwrap_or(u64::MAX);
        } else {
            let previous = output.last_mut().ok_or_else(invalid_frame_selection)?;
            previous.duration_us = previous
                .duration_us
                .checked_add(frame.duration_us)
                .ok_or_else(invalid_frame_selection)?;
        }
        elapsed_us = elapsed_us
            .checked_add(frame.duration_us)
            .ok_or_else(invalid_frame_selection)?;
    }
    output.push(frames.last().cloned().ok_or_else(invalid_frame_selection)?);

    if checked_timeline_duration(&output)? != total_duration_us {
        return Err(invalid_frame_selection());
    }
    Ok(output)
}

pub(crate) fn build_candidate_output_sequence<'source, 'candidate>(
    prepared: &'source PreparedSearchSource,
    candidate: &'candidate CandidatePreview,
    context: &OperationContext,
) -> Result<PreparedCandidateSequence<'source, 'candidate>, PipelineError> {
    context.checkpoint()?;
    let bounded = resample_timeline_to_limit(&prepared.base_sequence, MAX_SEARCH_OUTPUT_FRAMES)?;
    let duration_us = checked_timeline_duration(bounded.as_ref())?;
    let interval_us = interval_for_candidate(prepared, candidate)?;
    context.checkpoint()?;

    let frames = match interval_us {
        None => bounded,
        Some(interval_us) => {
            let sampled = sample_timeline_by_interval(bounded.as_ref(), interval_us)?;
            if sampled.as_slice() == bounded.as_ref() {
                bounded
            } else {
                Cow::Owned(sampled)
            }
        }
    };
    context.checkpoint()?;
    if checked_timeline_duration(frames.as_ref())? != duration_us {
        return Err(invalid_frame_selection());
    }

    Ok(PreparedCandidateSequence {
        source: prepared,
        candidate,
        frames,
        duration_us,
    })
}

pub(crate) fn build_inspected_timing_grid(
    base_frame_count: u32,
    source_duration_seconds: Option<f64>,
    avg_fps: Option<f64>,
) -> Result<Vec<ResolvedTimelineFrame>, PipelineError> {
    if base_frame_count == 0 {
        return Err(invalid_frame_selection());
    }

    let durations = if let Some(duration_seconds) =
        source_duration_seconds.filter(|duration| duration.is_finite() && *duration > 0.0)
    {
        let duration_us = duration_seconds * 1_000_000.0;
        if !duration_us.is_finite() || duration_us > u64::MAX as f64 {
            return Err(invalid_frame_selection());
        }
        let total_duration_us = duration_us.round() as u64;
        let count = u128::from(base_frame_count);
        (0..base_frame_count)
            .map(|index| {
                let rounding = count / 2;
                let start = (u128::from(index) * u128::from(total_duration_us) + rounding) / count;
                let end =
                    (u128::from(index + 1) * u128::from(total_duration_us) + rounding) / count;
                let start = u64::try_from(start).unwrap_or(u64::MAX);
                let end = u64::try_from(end).unwrap_or(u64::MAX);
                end - start
            })
            .collect::<Vec<_>>()
    } else {
        let fps = avg_fps
            .filter(|fps| fps.is_finite() && *fps > 0.0)
            .unwrap_or(30.0);
        let duration_us = (1_000_000.0 / fps).round().clamp(1.0, u64::MAX as f64) as u64;
        vec![duration_us; base_frame_count as usize]
    };

    let grid = durations
        .into_iter()
        .enumerate()
        .map(|(index, duration_us)| {
            Ok(ResolvedTimelineFrame {
                source_frame_index: u32::try_from(index).map_err(|_| invalid_frame_selection())?,
                duration_us,
            })
        })
        .collect::<Result<Vec<_>, PipelineError>>()?;
    checked_timeline_duration(&grid)?;
    Ok(grid)
}

pub(crate) fn project_timing_grid(
    timing_grid: &[ResolvedTimelineFrame],
    selected_frame_indexes: Option<&[u32]>,
) -> Result<Vec<ResolvedTimelineFrame>, PipelineError> {
    let Some(selected_frame_indexes) = selected_frame_indexes else {
        return Ok(timing_grid.to_vec());
    };
    if selected_frame_indexes.is_empty() {
        return Err(PipelineError::InvalidRequest {
            reason: "no-frames-selected",
        });
    }

    selected_frame_indexes
        .iter()
        .map(|index| {
            timing_grid
                .get(*index as usize)
                .cloned()
                .ok_or_else(invalid_frame_selection)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::Arc;
    use std::time::Duration;

    use image::RgbaImage;

    use super::{
        build_candidate_output_sequence, build_inspected_timing_grid, checked_prepared_bytes,
        project_timing_grid, resample_timeline_to_limit, PreparedFrame, PreparedSearchSource,
        TimelineTimingAuthority, MAX_PREPARED_SEARCH_BYTES, MAX_SEARCH_OUTPUT_FRAMES,
    };
    use crate::operation::OperationContext;
    use crate::{CandidatePreview, ResolvedTimelineFrame};

    fn timeline(count: usize, duration_us: u64) -> Vec<ResolvedTimelineFrame> {
        (0..count)
            .map(|index| ResolvedTimelineFrame {
                source_frame_index: (index % 17) as u32,
                duration_us,
            })
            .collect()
    }

    fn candidate(fps: u32, frame_sample_step: u32) -> CandidatePreview {
        CandidatePreview {
            id: format!("test-{fps}-{frame_sample_step}"),
            rank: 1,
            duration_seconds: 1.0,
            fps,
            content_scale: 1.0,
            preset: "standard".into(),
            fit_mode: "contain".into(),
            score: 1.0,
            source_similarity_score: 1.0,
            relative_size_factor: 1.0,
            summary: "test".into(),
            frame_sample_step,
        }
    }

    fn prepared(base_sequence: Vec<ResolvedTimelineFrame>, base_fps: u32) -> PreparedSearchSource {
        let pixels = Arc::new(RgbaImage::new(1, 1));
        let frames = (0..17)
            .map(|index| PreparedFrame {
                source_frame_id: index + 1,
                pixels: Arc::clone(&pixels),
                duration_us: 1,
            })
            .collect();
        PreparedSearchSource {
            frames,
            base_sequence,
            timing_authority: TimelineTimingAuthority::Inspected,
            base_fps,
            decoded_bytes: 4,
            tool_source: "test".into(),
            tool_command: None,
            tool_detail: None,
        }
    }

    fn total_duration(frames: &[ResolvedTimelineFrame]) -> u64 {
        frames.iter().map(|frame| frame.duration_us).sum()
    }

    #[test]
    fn prepared_byte_limit_rejects_before_crossing_128_mib() {
        assert_eq!(
            checked_prepared_bytes(MAX_PREPARED_SEARCH_BYTES - 1, 1),
            Ok(MAX_PREPARED_SEARCH_BYTES)
        );
        assert!(checked_prepared_bytes(MAX_PREPARED_SEARCH_BYTES, 1).is_err());
    }

    #[test]
    fn resample_at_or_below_limit_borrows_original_storage() {
        let frames = timeline(MAX_SEARCH_OUTPUT_FRAMES, 10_000);
        let sampled = resample_timeline_to_limit(&frames, MAX_SEARCH_OUTPUT_FRAMES)
            .expect("bounded timeline must be accepted");

        assert!(matches!(&sampled, Cow::Borrowed(_)));
        assert!(std::ptr::eq(sampled.as_ptr(), frames.as_ptr()));
        assert_eq!(sampled.as_ref(), frames.as_slice());
    }

    #[test]
    fn resample_151_frames_uses_150_buckets_and_preserves_time_and_edges() {
        let frames = timeline(151, 10_000);
        let sampled = resample_timeline_to_limit(&frames, MAX_SEARCH_OUTPUT_FRAMES)
            .expect("151 frames must be resampled");

        assert!(matches!(&sampled, Cow::Owned(_)));
        assert_eq!(sampled.len(), MAX_SEARCH_OUTPUT_FRAMES);
        assert_eq!(sampled[0].source_frame_index, frames[0].source_frame_index);
        assert_eq!(
            sampled.last().map(|frame| frame.source_frame_index),
            frames.last().map(|frame| frame.source_frame_index)
        );
        assert_eq!(total_duration(sampled.as_ref()), total_duration(&frames));
    }

    #[test]
    fn duration_bucket_midpoint_tie_chooses_the_earlier_frame() {
        let frames = (0..4)
            .map(|index| ResolvedTimelineFrame {
                source_frame_index: index,
                duration_us: 10,
            })
            .collect::<Vec<_>>();

        let sampled = resample_timeline_to_limit(&frames, 3)
            .expect("four frames must form three duration buckets");

        assert_eq!(
            sampled
                .iter()
                .map(|frame| (frame.source_frame_index, frame.duration_us))
                .collect::<Vec<_>>(),
            vec![(0, 10), (1, 20), (3, 10)]
        );
    }

    #[test]
    fn resample_300_variable_frames_preserves_order_duplicates_and_exact_duration() {
        let frames = (0..300)
            .map(|index| ResolvedTimelineFrame {
                source_frame_index: (index / 4) as u32,
                duration_us: 100 + (index % 9) as u64,
            })
            .collect::<Vec<_>>();
        let sampled = resample_timeline_to_limit(&frames, MAX_SEARCH_OUTPUT_FRAMES)
            .expect("300 frames must be resampled");

        assert_eq!(sampled.len(), MAX_SEARCH_OUTPUT_FRAMES);
        assert_eq!(total_duration(sampled.as_ref()), total_duration(&frames));
        assert!(sampled
            .windows(2)
            .all(|pair| pair[0].source_frame_index <= pair[1].source_frame_index));
        assert!(sampled
            .windows(2)
            .any(|pair| pair[0].source_frame_index == pair[1].source_frame_index));
    }

    #[test]
    fn resample_rejects_duration_overflow() {
        let frames = vec![
            ResolvedTimelineFrame {
                source_frame_index: 0,
                duration_us: u64::MAX,
            },
            ResolvedTimelineFrame {
                source_frame_index: 1,
                duration_us: 1,
            },
        ];

        assert!(resample_timeline_to_limit(&frames, 2).is_err());
    }

    #[test]
    fn candidate_fps_and_sample_step_preserve_first_last_and_total_duration() {
        let frames = (0..120)
            .map(|index| ResolvedTimelineFrame {
                source_frame_index: index as u32,
                duration_us: 20_000 + (index % 5) as u64 * 7_000,
            })
            .collect::<Vec<_>>();
        let prepared = prepared(frames.clone(), 30);

        for (fps, step) in [(30, 1), (15, 2), (10, 3)] {
            let candidate = candidate(fps, step);
            let context = OperationContext::detached(Duration::from_secs(1));
            let sequence = build_candidate_output_sequence(&prepared, &candidate, &context)
                .expect("candidate sequence must be built");
            assert_eq!(
                sequence.frames[0].source_frame_index,
                frames[0].source_frame_index
            );
            assert_eq!(
                sequence.frames.last().map(|frame| frame.source_frame_index),
                frames.last().map(|frame| frame.source_frame_index)
            );
            assert_eq!(sequence.duration_us, total_duration(&frames));
            assert_eq!(
                total_duration(sequence.frames.as_ref()),
                total_duration(&frames)
            );
        }
    }

    #[test]
    fn candidate_target_fps_caps_a_high_rate_bounded_sequence() {
        let frames = (0..150)
            .map(|index| ResolvedTimelineFrame {
                source_frame_index: index as u32,
                duration_us: 20_000,
            })
            .collect::<Vec<_>>();
        let prepared = prepared(frames.clone(), 50);
        let candidate = candidate(30, 1);
        let context = OperationContext::detached(Duration::from_secs(1));

        let sequence = build_candidate_output_sequence(&prepared, &candidate, &context)
            .expect("50 FPS source must be capped by a 30 FPS candidate");

        assert!(sequence.frames.len() <= 91);
        assert_eq!(sequence.frames[0].source_frame_index, 0);
        assert_eq!(
            sequence.frames.last().map(|frame| frame.source_frame_index),
            Some(149)
        );
        assert_eq!(sequence.duration_us, 3_000_000);
    }

    #[test]
    fn candidate_sampling_advances_huge_duration_boundary_in_constant_time() {
        let frames = vec![
            ResolvedTimelineFrame {
                source_frame_index: 0,
                duration_us: u64::MAX - 2,
            },
            ResolvedTimelineFrame {
                source_frame_index: 1,
                duration_us: 1,
            },
            ResolvedTimelineFrame {
                source_frame_index: 2,
                duration_us: 1,
            },
        ];
        let prepared = prepared(frames, 30);
        let candidate = candidate(30, 1);
        let context = OperationContext::detached(Duration::from_secs(1));

        let sequence = build_candidate_output_sequence(&prepared, &candidate, &context)
            .expect("large valid duration must not loop on every interval");

        assert_eq!(sequence.duration_us, u64::MAX);
        assert_eq!(total_duration(sequence.frames.as_ref()), u64::MAX);
    }

    #[test]
    fn sparse_selection_projects_full_inspected_timing_grid() {
        let duration_grid = build_inspected_timing_grid(7, Some(1.0), Some(7.0))
            .expect("duration timing grid must be built");
        let fps_grid =
            build_inspected_timing_grid(7, None, Some(7.0)).expect("FPS timing grid must be built");
        let selected = [0, 2, 6];

        let duration_projection = project_timing_grid(&duration_grid, Some(&selected))
            .expect("duration grid selection must project");
        let fps_projection = project_timing_grid(&fps_grid, Some(&selected))
            .expect("FPS grid selection must project");

        assert_eq!(total_duration(&duration_projection), 428_571);
        assert_eq!(total_duration(&fps_projection), 428_571);
        assert_eq!(
            duration_projection
                .iter()
                .map(|frame| frame.source_frame_index)
                .collect::<Vec<_>>(),
            selected
        );
    }

    #[test]
    fn duplicate_sequence_reuses_one_prepared_arc() {
        let pixels = Arc::new(RgbaImage::new(2, 2));
        let source = PreparedSearchSource {
            frames: vec![PreparedFrame {
                source_frame_id: 1,
                pixels: Arc::clone(&pixels),
                duration_us: 10_000,
            }],
            base_sequence: vec![
                ResolvedTimelineFrame {
                    source_frame_index: 0,
                    duration_us: 10_000,
                },
                ResolvedTimelineFrame {
                    source_frame_index: 0,
                    duration_us: 20_000,
                },
            ],
            timing_authority: TimelineTimingAuthority::Authored,
            base_fps: 30,
            decoded_bytes: 16,
            tool_source: "native".into(),
            tool_command: None,
            tool_detail: None,
        };

        let first = source
            .pixels_for_source_index(0)
            .expect("first duplicate must resolve");
        let second = source
            .pixels_for_source_index(0)
            .expect("second duplicate must resolve");
        assert!(Arc::ptr_eq(first, second));
        assert_eq!(source.decoded_bytes, pixels.as_raw().len());
    }
}
