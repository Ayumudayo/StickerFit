mod estimation;
mod frame_source;
mod locale;
mod media_error;
mod media_limits;
mod operation;
mod output_file;
mod preview_cache;
mod process_runner;

use base64::{engine::general_purpose, Engine as _};
use image::codecs::gif::GifDecoder as ImageGifDecoder;
use image::codecs::png::PngDecoder as ImagePngDecoder;
use image::error::LimitErrorKind as ImageLimitErrorKind;
use image::imageops::{self, FilterType};
use image::{AnimationDecoder, ImageDecoder, ImageReader, RgbaImage};
#[cfg(test)]
use image::{DynamicImage, ImageFormat, Rgba};
use png::{
    BitDepth as PngBitDepth, BlendOp as PngBlendOp, ColorType as PngColorType,
    DeflateCompression as PngDeflateCompression, DisposeOp as PngDisposeOp,
    Encoder as NativePngEncoder, Filter as PngFilter,
};
use regex::Regex;
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tauri::{ipc::Channel, State};
#[cfg(target_os = "windows")]
use windows::core::PCWSTR;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
#[cfg(target_os = "windows")]
use windows::Win32::Media::MediaFoundation::{
    MFCreateSourceReaderFromURL, MFShutdown, MFStartup, MFSTARTUP_FULL, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_PD_DURATION, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READER_MEDIASOURCE, MF_VERSION,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use crate::estimation::{
    estimate_candidate_size, estimate_static_png_with_operation, parse_sample_seed,
    probe_candidate_size, OutputSizeEstimate, OutputSizeEstimateError, StaticSizeEstimateRequest,
};
use crate::frame_source::{
    build_candidate_output_sequence, build_inspected_timing_grid, checked_prepared_bytes,
    checked_timeline_duration, project_timing_grid, FramePreparationRequest, FrameSourceLoader,
    PreparedCandidateSequence, PreparedFrame, PreparedSearchSource, TimelineTimingAuthority,
    MAX_PREPARED_SEARCH_BYTES, MAX_SEARCH_OUTPUT_FRAMES,
};
use crate::locale::{parse_ui_locale, UiLocale};
use crate::media_error::{MediaOperationErrorCode, MediaOperationReasonCode, PipelineError};
use crate::media_limits::{
    checked_rgba_bytes, image_decode_limits, validate_file_metadata, MediaLimits, SourceIdentity,
};
use crate::operation::{
    publish_progress, run_managed_blocking, ChannelProgressSink, MediaOperationKind,
    OperationContext, OperationProgress, PipelineState, ProgressSink, ProgressStage,
    ValidatedProgressSink,
};
use crate::output_file::PendingOutput;
use crate::preview_cache::{
    CachedPreview, PreviewCache, PreviewCachePublication, PreviewSourceKey, PreviewVariant,
    MAX_PREVIEW_CACHE_BYTES,
};
use crate::process_runner::{
    run_captured, stream_fixed_rgba_frames, CapturedProcess, ProcessLimits,
};

const CANONICAL_FIT_MODE: &str = "contain";
const MAX_MEDIA_FRAME_COUNT: usize = 300;
const MAX_PREVIEW_BATCH_IDS: usize = 24;
const MAX_PREVIEW_EDGE: u32 = 128;
const MEDIA_FOUNDATION_FAILED_REASON_CODE: &str = "media-foundation-failed";

struct BoundedVecVisitor<T>(PhantomData<T>);

impl<'de, T> Visitor<'de> for BoundedVecVisitor<T>
where
    T: Deserialize<'de>,
{
    type Value = Vec<T>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "a sequence containing at most {MAX_MEDIA_FRAME_COUNT} frame items"
        )
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        if sequence
            .size_hint()
            .is_some_and(|size| size > MAX_MEDIA_FRAME_COUNT)
        {
            return Err(de::Error::custom("media frame count exceeds 300"));
        }

        let mut values = Vec::with_capacity(
            sequence
                .size_hint()
                .unwrap_or_default()
                .min(MAX_MEDIA_FRAME_COUNT),
        );
        while values.len() < MAX_MEDIA_FRAME_COUNT {
            let Some(value) = sequence.next_element()? else {
                return Ok(values);
            };
            values.push(value);
        }
        if sequence.next_element::<de::IgnoredAny>()?.is_some() {
            return Err(de::Error::custom("media frame count exceeds 300"));
        }
        Ok(values)
    }
}

fn deserialize_bounded_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    deserializer.deserialize_seq(BoundedVecVisitor(PhantomData))
}

struct OptionalBoundedVecVisitor<T>(PhantomData<T>);

impl<'de, T> Visitor<'de> for OptionalBoundedVecVisitor<T>
where
    T: Deserialize<'de>,
{
    type Value = Option<Vec<T>>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("null or a frame sequence containing at most 300 items")
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(None)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_bounded_vec(deserializer).map(Some)
    }
}

fn deserialize_optional_bounded_vec<'de, D, T>(deserializer: D) -> Result<Option<Vec<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    deserializer.deserialize_option(OptionalBoundedVecVisitor(PhantomData))
}

fn deserialize_optional_frame_count<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<u32>::deserialize(deserializer)?;
    if value.is_some_and(|count| count as usize > MAX_MEDIA_FRAME_COUNT) {
        return Err(de::Error::custom("media frame count exceeds 300"));
    }
    Ok(value)
}

#[derive(Debug)]
struct BoundedFrameIds(Vec<u32>);

impl<'de> Deserialize<'de> for BoundedFrameIds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_bounded_vec(deserializer).map(Self)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolCheck {
    tool: String,
    available: bool,
    source: String,
    resolved_command: Option<String>,
    fallback_reason: Option<String>,
    version_line: Option<String>,
    detail: String,
    expected_sidecar_name: String,
    attempted_sidecar_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolHealthReport {
    ready: bool,
    checks: Vec<ToolCheck>,
    summary: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaInspection {
    ok: bool,
    input_path: String,
    source_revision: Option<String>,
    tool_source: Option<String>,
    tool_command: Option<String>,
    tool_detail: Option<String>,
    fallback_reason_code: Option<String>,
    format_name: Option<String>,
    duration_seconds: Option<f64>,
    size_bytes: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    codec_name: Option<String>,
    pixel_format: Option<String>,
    avg_fps: Option<f64>,
    frame_rate_label: Option<String>,
    estimated_frames: Option<u64>,
    frame_durations_seconds: Option<Vec<f64>>,
    warnings: Vec<String>,
    is_static_image: bool,
    can_convert_to_png: bool,
    error_code: Option<MediaOperationErrorCode>,
    reason_code: Option<MediaOperationReasonCode>,
    error_message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FramePreviewResponse {
    ok: bool,
    data_url: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    error_code: Option<MediaOperationErrorCode>,
    reason_code: Option<MediaOperationReasonCode>,
    error_message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FramePreviewItem {
    source_frame_id: u32,
    data_url: String,
    width: u32,
    height: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FramePreviewsResponse {
    ok: bool,
    previews: Vec<FramePreviewItem>,
    error_code: Option<MediaOperationErrorCode>,
    reason_code: Option<MediaOperationReasonCode>,
    error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CropRegion {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OptimizerPlanRequest {
    locale: Option<String>,
    source_duration_seconds: Option<f64>,
    input_width: Option<u32>,
    input_height: Option<u32>,
    avg_fps: Option<f64>,
    fit_mode: Option<String>,
    preset_strategy: Option<String>,
    optimizer_goal: Option<String>,
    quality_frame_drop_interval: Option<u32>,
    search_depth: Option<String>,
    crop_region: Option<CropRegion>,
    #[serde(default, deserialize_with = "deserialize_optional_bounded_vec")]
    selected_frames: Option<Vec<u32>>,
    #[serde(default, deserialize_with = "deserialize_optional_frame_count")]
    base_frame_count: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_optional_bounded_vec")]
    timeline_frames: Option<Vec<EditedTimelineFrame>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OptimizerSizeEstimateRequest {
    input_path: String,
    source_revision: String,
    candidate_ids: Vec<String>,
    sample_seed: String,
    #[serde(flatten)]
    plan: OptimizerPlanRequest,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CandidateSizeProbeRequest {
    input_path: String,
    source_revision: String,
    candidate_id: String,
    #[serde(flatten)]
    plan: OptimizerPlanRequest,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditedTimelineFrame {
    source_frame_id: u32,
    duration_us: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CandidatePreview {
    id: String,
    rank: usize,
    duration_seconds: f64,
    fps: u32,
    content_scale: f64,
    preset: String,
    fit_mode: String,
    score: f64,
    source_similarity_score: f64,
    relative_size_factor: f64,
    summary: String,
    #[serde(skip_serializing)]
    frame_sample_step: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OptimizerPlanResponse {
    ok: bool,
    fit_mode: String,
    selected_duration_seconds: Option<f64>,
    recommended_max_duration_seconds: f64,
    search_budget: usize,
    warnings: Vec<String>,
    candidates: Vec<CandidatePreview>,
    error_code: Option<MediaOperationErrorCode>,
    reason_code: Option<MediaOperationReasonCode>,
    error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StaticImageConversionRequest {
    input_path: String,
    #[serde(default)]
    source_revision: Option<String>,
    output_directory: Option<String>,
    locale: Option<String>,
    crop_region: Option<CropRegion>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StaticImageConversionResult {
    ok: bool,
    output_path: Option<String>,
    size_bytes: Option<u64>,
    elapsed_ms: Option<u64>,
    tool_source: Option<String>,
    tool_command: Option<String>,
    tool_detail: Option<String>,
    warnings: Vec<String>,
    error_code: Option<MediaOperationErrorCode>,
    reason_code: Option<MediaOperationReasonCode>,
    error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OptimizerSearchRequest {
    input_path: String,
    #[serde(default)]
    source_revision: Option<String>,
    output_directory: Option<String>,
    locale: Option<String>,
    source_duration_seconds: Option<f64>,
    input_width: Option<u32>,
    input_height: Option<u32>,
    avg_fps: Option<f64>,
    fit_mode: Option<String>,
    preset_strategy: Option<String>,
    optimizer_goal: Option<String>,
    quality_frame_drop_interval: Option<u32>,
    search_depth: Option<String>,
    crop_region: Option<CropRegion>,
    #[serde(default, deserialize_with = "deserialize_optional_bounded_vec")]
    selected_frames: Option<Vec<u32>>,
    #[serde(default, deserialize_with = "deserialize_optional_frame_count")]
    base_frame_count: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_optional_bounded_vec")]
    timeline_frames: Option<Vec<EditedTimelineFrame>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchAttemptResult {
    candidate_id: String,
    canonical_candidate_id: String,
    equivalent_to_candidate_id: Option<String>,
    rank: usize,
    duration_seconds: f64,
    fps: u32,
    content_scale: f64,
    preset: String,
    fit_mode: String,
    score: f64,
    source_similarity_score: f64,
    summary: String,
    skipped: bool,
    within_limit: bool,
    output_path: Option<String>,
    size_bytes: Option<u64>,
    elapsed_ms: Option<u64>,
    tool_source: Option<String>,
    tool_command: Option<String>,
    tool_detail: Option<String>,
    warnings: Vec<String>,
    error_code: Option<MediaOperationErrorCode>,
    reason_code: Option<MediaOperationReasonCode>,
    error_message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OptimizerSearchResponse {
    ok: bool,
    fit_mode: String,
    selected_duration_seconds: Option<f64>,
    limit_bytes: u64,
    search_budget: usize,
    real_attempt_count: usize,
    stop_reason: Option<String>,
    selection_reason: String,
    summary: String,
    warnings: Vec<String>,
    attempts: Vec<SearchAttemptResult>,
    winning_candidate_id: Option<String>,
    closest_candidate_id: Option<String>,
    best_output_path: Option<String>,
    best_size_bytes: Option<u64>,
    best_within_limit: bool,
    error_code: Option<MediaOperationErrorCode>,
    reason_code: Option<MediaOperationReasonCode>,
    error_message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResolvedCropRegion {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone)]
struct ResolvedFrameSelection {
    selected_frames: Option<Vec<u32>>,
    selected_frame_count: usize,
    base_frame_count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedTimelineFrame {
    source_frame_index: u32,
    duration_us: u64,
}

#[derive(Clone)]
struct ToolResolution {
    source: &'static str,
    command: OsString,
    command_display: String,
    attempted_sidecar_paths: Vec<String>,
    fallback_reason: Option<String>,
}

struct CommandOutput {
    resolution: ToolResolution,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

struct ToolRunError {
    resolution: ToolResolution,
    error: PipelineError,
}

struct EncodeResult {
    pending_output: PendingOutput,
    size_bytes: u64,
    elapsed_ms: u64,
    tool_source: String,
    tool_command: Option<String>,
    tool_detail: Option<String>,
}

#[derive(Debug, Clone)]
struct SelectedEncodeOutput {
    candidate_id: String,
    rank: usize,
    duration_seconds: f64,
    size_bytes: u64,
    source_similarity_score: f64,
}

struct PendingSelectedEncodeOutput {
    selected: SelectedEncodeOutput,
    pending_output: PendingOutput,
}

#[derive(Debug)]
struct PublishedSelectedEncodeOutput {
    selected: SelectedEncodeOutput,
    output_path: String,
}

#[derive(Debug, Clone)]
struct StickerFrame {
    pixels: RgbaImage,
    duration_us: u64,
}

#[derive(Clone, Copy)]
struct FrameRegion {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Debug)]
struct PngAnimationMetadata {
    width: u32,
    height: u32,
    frame_count: Option<u64>,
    frame_durations: Vec<f64>,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct PngFileMetadata {
    animation: Option<PngAnimationMetadata>,
    warnings: Vec<String>,
}

const RECOMMENDED_MAX_DURATION_US: u64 = 3_000_000;
const DISCORD_MAX_DURATION_US: u64 = 5_000_000;
const DISCORD_MAX_STICKER_BYTES: u64 = 512 * 1024;
const MAX_SEARCH_BUDGET: usize = 20;
const INTERNAL_TASK_ERROR_CODE: &str = "internal-task-failed";
const PROCESS_CAPTURE_LIMIT_BYTES: usize = 1024 * 1024;
const TOOL_PROCESS_TIMEOUT: Duration = Duration::from_secs(15);
const RAW_DECODE_PROCESS_TIMEOUT: Duration = Duration::from_secs(120);

fn duration_us_to_seconds(duration_us: u64) -> f64 {
    duration_us as f64 / 1_000_000.0
}

fn frame_duration_us_for_fps(fps: u32) -> u64 {
    let fps = u64::from(fps.max(1));
    (1_000_000 + fps / 2) / fps
}

async fn run_blocking_task<T, F>(job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| error.to_string())
}

fn current_target_triple() -> &'static str {
    if cfg!(all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc"
    )) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(
        target_os = "windows",
        target_arch = "aarch64",
        target_env = "msvc"
    )) {
        "aarch64-pc-windows-msvc"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "gnu"
    )) {
        "x86_64-unknown-linux-gnu"
    } else {
        "unknown-target"
    }
}

fn expected_sidecar_name(tool: &str) -> String {
    let extension = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };
    format!("{}-{}{}", tool, current_target_triple(), extension)
}

fn packaged_sidecar_name(tool: &str) -> String {
    let extension = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };
    format!("{tool}{extension}")
}

fn first_output_line(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    let stdout_line = String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned);

    stdout_line.or_else(|| {
        String::from_utf8_lossy(stderr)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn preset_similarity_score(preset: &str) -> f64 {
    match preset {
        "standard" => 1.0,
        "compact" => 0.9,
        "compactPlus" => 0.8,
        _ => 0.75,
    }
}

#[cfg(test)]
fn source_similarity_score(
    source_fps: f64,
    candidate_fps: u32,
    content_scale: f64,
    preset: &str,
    source_duration_seconds: f64,
    candidate_duration_seconds: f64,
) -> f64 {
    let normalized_source_fps = source_fps.clamp(1.0, 30.0);
    let fps_score = ((candidate_fps as f64) / normalized_source_fps).clamp(0.0, 1.0);
    let scale_score = content_scale.clamp(0.0, 1.0);
    let duration_score = if source_duration_seconds.is_finite() && source_duration_seconds > 0.0 {
        (1.0 - ((candidate_duration_seconds - source_duration_seconds).abs()
            / source_duration_seconds))
            .clamp(0.0, 1.0)
    } else {
        1.0
    };

    (fps_score * 0.45)
        + (scale_score * 0.35)
        + (preset_similarity_score(preset) * 0.15)
        + (duration_score * 0.05)
}

fn optimizer_goal_score(
    optimizer_goal: &str,
    source_fps: f64,
    candidate_fps: u32,
    content_scale: f64,
    preset: &str,
    source_duration_seconds: f64,
    candidate_duration_seconds: f64,
    frame_retention_score: f64,
) -> f64 {
    let normalized_source_fps = source_fps.clamp(1.0, 30.0);
    let fps_score = ((candidate_fps as f64) / normalized_source_fps).clamp(0.0, 1.0);
    let scale_score = content_scale.clamp(0.0, 1.0);
    let preset_score = preset_similarity_score(preset);
    let duration_score = if source_duration_seconds.is_finite() && source_duration_seconds > 0.0 {
        (1.0 - ((candidate_duration_seconds - source_duration_seconds).abs()
            / source_duration_seconds))
            .clamp(0.0, 1.0)
    } else {
        1.0
    };
    let frame_score = frame_retention_score.clamp(0.0, 1.0);

    match optimizer_goal {
        "motion" => {
            (frame_score * 0.45)
                + (fps_score * 0.20)
                + (duration_score * 0.15)
                + (scale_score * 0.15)
                + (preset_score * 0.05)
        }
        "quality" => {
            (scale_score * 0.45)
                + (preset_score * 0.20)
                + (frame_score * 0.15)
                + (fps_score * 0.10)
                + (duration_score * 0.10)
        }
        _ => {
            (frame_score * 0.30)
                + (scale_score * 0.30)
                + (preset_score * 0.15)
                + (fps_score * 0.15)
                + (duration_score * 0.10)
        }
    }
}

fn is_better_within_limit_candidate(
    current: &SelectedEncodeOutput,
    contender: &SelectedEncodeOutput,
) -> bool {
    if contender.source_similarity_score > current.source_similarity_score {
        return true;
    }

    if (contender.source_similarity_score - current.source_similarity_score).abs() <= 0.000_001 {
        if contender.size_bytes < current.size_bytes {
            return true;
        }

        if contender.size_bytes == current.size_bytes {
            return contender.rank < current.rank;
        }
    }

    false
}

fn is_better_oversize_candidate(
    current: &SelectedEncodeOutput,
    contender: &SelectedEncodeOutput,
) -> bool {
    if contender.size_bytes < current.size_bytes {
        return true;
    }

    if contender.size_bytes == current.size_bytes {
        if contender.source_similarity_score > current.source_similarity_score {
            return true;
        }

        if (contender.source_similarity_score - current.source_similarity_score).abs() <= 0.000_001
        {
            return contender.rank < current.rank;
        }
    }

    false
}

fn remaining_candidate_cannot_beat_within_limit(
    best_within_limit_output: Option<&SelectedEncodeOutput>,
    candidate: &CandidatePreview,
) -> bool {
    best_within_limit_output
        .map(|best| candidate.source_similarity_score + 0.000_001 < best.source_similarity_score)
        .unwrap_or(false)
}

#[cfg(test)]
fn apng_compression_level_for_preset(preset: &str) -> &'static str {
    match preset {
        "compactPlus" => "9",
        "compact" => "7",
        _ => "4",
    }
}

#[cfg(test)]
fn apng_prediction_for_preset(preset: &str) -> &'static str {
    match preset {
        "compactPlus" => "mixed",
        "compact" => "mixed",
        _ => "paeth",
    }
}

fn normalized_preset_strategy(raw: Option<&str>) -> &'static str {
    match raw {
        Some("quality") => "quality",
        Some("size") => "size",
        _ => "auto",
    }
}

fn normalized_optimizer_goal(
    raw: Option<&str>,
    legacy_preset_strategy: Option<&str>,
) -> &'static str {
    match raw {
        Some("motion") => "motion",
        Some("quality") => "quality",
        Some("balanced") => "balanced",
        _ => match legacy_preset_strategy {
            Some("quality") => "quality",
            _ => "balanced",
        },
    }
}

fn normalized_quality_frame_drop_interval(raw: Option<u32>) -> u32 {
    match raw.unwrap_or(3) {
        0 | 1 => 0,
        value => value.min(12),
    }
}

fn normalized_search_depth(raw: Option<&str>) -> &'static str {
    match raw {
        Some("thorough") => "thorough",
        _ => "standard",
    }
}

fn search_budget_for_depth(search_depth: &str) -> usize {
    match search_depth {
        "thorough" => 36,
        _ => MAX_SEARCH_BUDGET,
    }
}

fn preset_ladder_for_strategy(duration_seconds: f64, preset_strategy: &str) -> Vec<&'static str> {
    match preset_strategy {
        "quality" => vec!["standard"],
        "size" => vec!["compact", "compactPlus"],
        _ => {
            if duration_seconds > duration_us_to_seconds(RECOMMENDED_MAX_DURATION_US) {
                vec!["standard", "compact", "compactPlus"]
            } else {
                vec!["standard", "compact"]
            }
        }
    }
}

fn preset_size_factor(preset: &str) -> f64 {
    match preset {
        "compactPlus" => 0.82,
        "compact" => 0.9,
        _ => 1.0,
    }
}

fn relative_size_factor_for(frame_sample_step: u32, content_scale: f64, preset: &str) -> f64 {
    let frame_factor = 1.0 / frame_sample_step.max(1) as f64;
    let scale_factor = content_scale.clamp(0.01, 1.0).powi(2);
    frame_factor * scale_factor * preset_size_factor(preset)
}

fn candidate_estimated_size_factor(candidate: &CandidatePreview) -> f64 {
    candidate.relative_size_factor
}

fn push_unique_candidate(
    selected: &mut Vec<CandidatePreview>,
    candidate: &CandidatePreview,
    limit: usize,
) {
    if selected.len() >= limit {
        return;
    }

    if selected.iter().any(|existing| existing.id == candidate.id) {
        return;
    }

    selected.push(candidate.clone());
}

fn select_ranked_candidate_subset(
    mut candidates: Vec<CandidatePreview>,
    search_budget: usize,
) -> Vec<CandidatePreview> {
    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.fps.cmp(&left.fps))
            .then_with(|| {
                right
                    .content_scale
                    .partial_cmp(&left.content_scale)
                    .unwrap_or(Ordering::Equal)
            })
    });

    let quality_budget = (search_budget / 2).max(1);
    let mut selected = Vec::with_capacity(search_budget.min(candidates.len()));
    for candidate in candidates.iter().take(quality_budget) {
        push_unique_candidate(&mut selected, candidate, search_budget);
    }

    let mut smallest_candidates = candidates.clone();
    smallest_candidates.sort_by(|left, right| {
        candidate_estimated_size_factor(left)
            .partial_cmp(&candidate_estimated_size_factor(right))
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(Ordering::Equal)
            })
    });
    for candidate in &smallest_candidates {
        push_unique_candidate(&mut selected, candidate, search_budget);
    }

    for candidate in &candidates {
        push_unique_candidate(&mut selected, candidate, search_budget);
    }

    selected.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.fps.cmp(&left.fps))
            .then_with(|| {
                right
                    .content_scale
                    .partial_cmp(&left.content_scale)
                    .unwrap_or(Ordering::Equal)
            })
    });

    for (index, candidate) in selected.iter_mut().enumerate() {
        candidate.rank = index + 1;
    }

    selected
}

fn source_output_directory(input_path: &str, locale: UiLocale) -> Result<PathBuf, String> {
    Path::new(input_path)
        .parent()
        .map(Path::to_path_buf)
        .filter(|directory| !directory.as_os_str().is_empty())
        .ok_or_else(|| locale::source_output_directory_error(locale))
}

fn resolve_output_directory(
    selected_directory: Option<&str>,
    input_path: &str,
    locale: UiLocale,
) -> Result<PathBuf, String> {
    let Some(selected_directory) = selected_directory
        .map(str::trim)
        .filter(|directory| !directory.is_empty())
    else {
        return source_output_directory(input_path, locale);
    };

    let directory = PathBuf::from(selected_directory);

    if directory.exists() {
        if directory.is_dir() {
            Ok(directory)
        } else {
            Err(locale::output_path_not_directory_error(locale))
        }
    } else {
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        Ok(directory)
    }
}

fn sidecar_candidate_paths_for_exe(tool: &str, current_exe: Option<&Path>) -> Vec<PathBuf> {
    let expected = expected_sidecar_name(tool);
    let packaged = packaged_sidecar_name(tool);
    let mut paths = Vec::new();

    paths.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(&expected),
    );

    if let Some(current_exe) = current_exe {
        if let Some(exe_dir) = current_exe.parent() {
            paths.push(exe_dir.join(&packaged));
            paths.push(exe_dir.join(&expected));
            paths.push(exe_dir.join("binaries").join(&packaged));
            paths.push(exe_dir.join("binaries").join(&expected));
        }
    }

    paths
}

fn sidecar_candidate_paths(tool: &str) -> Vec<PathBuf> {
    sidecar_candidate_paths_for_exe(tool, std::env::current_exe().ok().as_deref())
}

fn safe_tool_path_label(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

fn safe_tool_version_line(tool: &str, stdout: &[u8], stderr: &[u8]) -> Option<String> {
    let line = first_output_line(stdout, stderr)?;
    let mut words = line.split_whitespace();
    while let Some(word) = words.next() {
        if word.eq_ignore_ascii_case("version") {
            let version = words.next().unwrap_or("available");
            let safe_version = version
                .chars()
                .filter(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '+')
                })
                .take(48)
                .collect::<String>();
            return Some(format!(
                "{tool} {}",
                if safe_version.is_empty() {
                    "available"
                } else {
                    &safe_version
                }
            ));
        }
    }
    Some(format!("{tool} available"))
}

fn resolve_tool(tool: &str, locale: UiLocale) -> Result<ToolResolution, String> {
    let expected = expected_sidecar_name(tool);
    let candidate_paths = sidecar_candidate_paths(tool);
    let attempted_paths: Vec<String> = candidate_paths
        .iter()
        .map(|path| safe_tool_path_label(path, &expected))
        .collect();

    if let Some(existing_path) = candidate_paths.iter().find(|path| path.is_file()).cloned() {
        return Ok(ToolResolution {
            source: "sidecar",
            command: existing_path.as_os_str().to_os_string(),
            command_display: safe_tool_path_label(&existing_path, tool),
            attempted_sidecar_paths: attempted_paths,
            fallback_reason: None,
        });
    }

    Err(locale::missing_sidecar_reason(
        locale,
        tool,
        &expected,
        &attempted_paths,
    ))
}

fn run_resolved_command(
    resolution: &ToolResolution,
    args: &[OsString],
    limits: ProcessLimits,
    context: &OperationContext,
) -> Result<CommandOutput, PipelineError> {
    let CapturedProcess { stdout, stderr } =
        run_captured(&resolution.command, args, limits, context)?;
    Ok(CommandOutput {
        resolution: resolution.clone(),
        stdout,
        stderr,
    })
}

fn run_sidecar_tool(
    tool: &'static str,
    args: &[OsString],
    locale: UiLocale,
    limits: ProcessLimits,
    context: &OperationContext,
) -> Result<CommandOutput, ToolRunError> {
    let resolved = match resolve_tool(tool, locale) {
        Ok(resolved) => resolved,
        Err(_) => {
            return Err(ToolRunError {
                resolution: ToolResolution {
                    source: "missing",
                    command: OsString::new(),
                    command_display: tool.into(),
                    attempted_sidecar_paths: sidecar_candidate_paths(tool)
                        .iter()
                        .map(|path| safe_tool_path_label(path, tool))
                        .collect(),
                    fallback_reason: None,
                },
                error: PipelineError::ToolMissing { tool },
            })
        }
    };

    match run_resolved_command(&resolved, args, limits, context) {
        Ok(output) => Ok(output),
        Err(error) => Err(ToolRunError {
            resolution: resolved,
            error,
        }),
    }
}

fn check_tool(tool: &'static str, locale: UiLocale) -> ToolCheck {
    let expected = expected_sidecar_name(tool);
    let context = OperationContext::detached(TOOL_PROCESS_TIMEOUT);
    let args = [OsString::from("-version")];

    match run_sidecar_tool(
        tool,
        &args,
        locale,
        ProcessLimits {
            timeout: TOOL_PROCESS_TIMEOUT,
            max_stdout_bytes: PROCESS_CAPTURE_LIMIT_BYTES,
            max_stderr_bytes: PROCESS_CAPTURE_LIMIT_BYTES,
        },
        &context,
    ) {
        Ok(output) => ToolCheck {
            tool: tool.to_string(),
            available: true,
            source: output.resolution.source.to_string(),
            resolved_command: Some(output.resolution.command_display.clone()),
            fallback_reason: output.resolution.fallback_reason.clone(),
            version_line: safe_tool_version_line(tool, &output.stdout, &output.stderr),
            detail: locale::tool_check_sidecar_ok_detail(
                locale,
                tool,
                &output.resolution.command_display,
            ),
            expected_sidecar_name: expected,
            attempted_sidecar_paths: output.resolution.attempted_sidecar_paths.clone(),
        },
        Err(error) => {
            let source = error.resolution.source.to_string();
            let resolved_command = (error.resolution.source != "missing")
                .then(|| error.resolution.command_display.clone());
            ToolCheck {
                tool: tool.to_string(),
                available: false,
                source,
                resolved_command,
                fallback_reason: error.resolution.fallback_reason.clone(),
                version_line: None,
                detail: pipeline_error_diagnostic(&error.error, locale),
                expected_sidecar_name: expected,
                attempted_sidecar_paths: error.resolution.attempted_sidecar_paths,
            }
        }
    }
}

fn lowercase_source_extension(input_path: &str) -> Option<String> {
    Path::new(input_path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
}

fn is_supported_static_image_extension(extension: &str) -> bool {
    matches!(extension, "png" | "jpg" | "jpeg" | "bmp")
}

fn is_supported_video_extension(extension: &str) -> bool {
    matches!(extension, "mp4" | "m4v" | "mov" | "webm")
}

fn crop_region_is_full_frame(crop_region: &CropRegion) -> bool {
    (crop_region.x - 0.0).abs() < 0.000_1
        && (crop_region.y - 0.0).abs() < 0.000_1
        && (crop_region.width - 1.0).abs() < 0.000_1
        && (crop_region.height - 1.0).abs() < 0.000_1
}

fn resolve_crop_region(
    crop_region: Option<&CropRegion>,
    input_width: Option<u32>,
    input_height: Option<u32>,
    locale: UiLocale,
) -> Result<Option<ResolvedCropRegion>, String> {
    let Some(crop_region) = crop_region else {
        return Ok(None);
    };

    if crop_region_is_full_frame(crop_region) {
        return Ok(None);
    }

    let values = [
        crop_region.x,
        crop_region.y,
        crop_region.width,
        crop_region.height,
    ];
    if values.iter().any(|value| !value.is_finite()) {
        return Err(locale::invalid_crop_number_error(locale));
    }

    let source_width = input_width
        .filter(|value| *value > 0)
        .ok_or_else(|| locale::crop_needs_source_width_error(locale))?;
    let source_height = input_height
        .filter(|value| *value > 0)
        .ok_or_else(|| locale::crop_needs_source_height_error(locale))?;

    let width = ((source_width as f64 * crop_region.width).round() as u32).clamp(1, source_width);
    let height =
        ((source_height as f64 * crop_region.height).round() as u32).clamp(1, source_height);
    let max_x = source_width.saturating_sub(width);
    let max_y = source_height.saturating_sub(height);
    let x = ((source_width as f64 * crop_region.x).round() as u32).min(max_x);
    let y = ((source_height as f64 * crop_region.y).round() as u32).min(max_y);

    if x == 0 && y == 0 && width == source_width && height == source_height {
        Ok(None)
    } else {
        Ok(Some(ResolvedCropRegion {
            x,
            y,
            width,
            height,
        }))
    }
}

fn unpack_media_foundation_pair(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, (value & 0xffff_ffff) as u32)
}

fn normalize_selected_frame_indexes(selected_frames: Option<&Vec<u32>>) -> Option<Vec<u32>> {
    selected_frames.map(|frames| {
        frames
            .iter()
            .copied()
            .filter(|frame| *frame > 0)
            .map(|frame| frame - 1)
            .collect()
    })
}

fn resolve_frame_selection(
    selected_frames: Option<&Vec<u32>>,
    base_frame_count: Option<u32>,
) -> Result<ResolvedFrameSelection, &'static str> {
    if selected_frames.is_some_and(|frames| frames.len() > MAX_MEDIA_FRAME_COUNT)
        || base_frame_count.is_some_and(|count| count as usize > MAX_MEDIA_FRAME_COUNT)
    {
        return Err("invalid-frame-selection");
    }
    let normalized_selected_frames = normalize_selected_frame_indexes(selected_frames);

    match normalized_selected_frames {
        Some(frames) if frames.is_empty() => Err("no-frames-selected"),
        Some(frames) => {
            if !selected_frame_indexes_match_base_count(&frames, base_frame_count) {
                return Err("invalid-frame-selection");
            }

            let selected_frame_count = frames.len();
            let is_unedited_full_sequence = base_frame_count
                .filter(|count| *count > 0)
                .is_some_and(|base_count| {
                    frames.len() == base_count as usize && frames.iter().copied().eq(0..base_count)
                });
            let selected_frames = (!is_unedited_full_sequence).then_some(frames);

            Ok(ResolvedFrameSelection {
                selected_frames,
                selected_frame_count,
                base_frame_count,
            })
        }
        None => {
            let base_frame_count = match base_frame_count.filter(|count| *count > 0) {
                Some(base_frame_count) => base_frame_count,
                None => return Err("no-frames-selected"),
            };

            Ok(ResolvedFrameSelection {
                selected_frames: None,
                selected_frame_count: base_frame_count as usize,
                base_frame_count: Some(base_frame_count),
            })
        }
    }
}

fn resolve_timeline_frames(
    timeline_frames: Option<&Vec<EditedTimelineFrame>>,
    base_frame_count: Option<u32>,
) -> Result<Option<Vec<ResolvedTimelineFrame>>, &'static str> {
    let Some(timeline_frames) = timeline_frames else {
        return Ok(None);
    };

    if timeline_frames.is_empty() {
        return Err("no-frames-selected");
    }

    if timeline_frames.len() > MAX_MEDIA_FRAME_COUNT
        || base_frame_count.is_some_and(|count| count as usize > MAX_MEDIA_FRAME_COUNT)
    {
        return Err("invalid-frame-selection");
    }

    let Some(base_frame_count) = base_frame_count.filter(|count| *count > 0) else {
        return Err("invalid-frame-selection");
    };

    let mut resolved = Vec::with_capacity(timeline_frames.len());

    for frame in timeline_frames {
        if frame.source_frame_id == 0 || frame.source_frame_id > base_frame_count {
            return Err("invalid-frame-selection");
        }

        if frame.duration_us < 100 {
            return Err("invalid-frame-duration");
        }

        resolved.push(ResolvedTimelineFrame {
            source_frame_index: frame.source_frame_id - 1,
            duration_us: frame.duration_us,
        });
    }

    Ok(Some(resolved))
}

fn selected_frame_indexes_match_base_count(
    selected_frames: &[u32],
    base_frame_count: Option<u32>,
) -> bool {
    let Some(base_frame_count) = base_frame_count.filter(|count| *count > 0) else {
        return true;
    };

    selected_frames
        .iter()
        .all(|frame_index| *frame_index < base_frame_count)
}

fn derive_source_fps(
    avg_fps: Option<f64>,
    source_duration_seconds: Option<f64>,
    base_frame_count: Option<u32>,
) -> f64 {
    avg_fps
        .filter(|fps| fps.is_finite() && *fps > 0.0)
        .or_else(|| {
            match (
                source_duration_seconds.filter(|duration| duration.is_finite() && *duration > 0.0),
                base_frame_count.filter(|count| *count > 0),
            ) {
                (Some(duration), Some(frame_count)) => Some(frame_count as f64 / duration),
                _ => None,
            }
        })
        .unwrap_or(30.0)
}

fn candidate_duration_seconds(selected_frame_count: usize, fps: u32) -> f64 {
    selected_frame_count as f64 / fps.max(1) as f64
}

fn sampled_frame_count(frame_count: usize, sample_step: u32) -> usize {
    let step = sample_step.max(1) as usize;
    frame_count.div_ceil(step)
}

fn frame_sample_steps_for_selection(selected_frame_count: usize) -> Vec<u32> {
    let mut steps = vec![1];
    if selected_frame_count > 40 {
        steps.push(2);
    }
    if selected_frame_count > 120 {
        steps.push(3);
    }
    if selected_frame_count > 180 {
        steps.push(4);
    }

    let required_step = (selected_frame_count as f64
        / (duration_us_to_seconds(DISCORD_MAX_DURATION_US) * 30.0))
        .ceil() as u32;
    if required_step > 1 {
        steps.push(required_step);
    }

    steps.sort_unstable();
    steps.dedup();
    steps
}

fn frame_sample_steps_for_goal(selected_frame_count: usize, optimizer_goal: &str) -> Vec<u32> {
    match optimizer_goal {
        "quality" => vec![1],
        "motion" => {
            if candidate_duration_seconds(selected_frame_count, 30)
                <= duration_us_to_seconds(DISCORD_MAX_DURATION_US)
            {
                vec![1]
            } else {
                frame_sample_steps_for_selection(selected_frame_count)
            }
        }
        _ => frame_sample_steps_for_selection(selected_frame_count),
    }
}

fn remove_every_nth_frame_indexes(frame_indexes: Vec<u32>, interval: u32) -> Vec<u32> {
    if interval < 2 {
        return frame_indexes;
    }

    frame_indexes
        .into_iter()
        .enumerate()
        .filter_map(|(index, frame)| {
            if (index + 1) % interval as usize == 0 {
                None
            } else {
                Some(frame)
            }
        })
        .collect()
}

fn apply_quality_frame_drop_to_selection(
    selection: ResolvedFrameSelection,
    interval: u32,
) -> Result<ResolvedFrameSelection, &'static str> {
    if interval < 2 {
        return Ok(selection);
    }

    let frame_indexes = selection.selected_frames.clone().unwrap_or_else(|| {
        selection
            .base_frame_count
            .map(|count| (0..count).collect())
            .unwrap_or_default()
    });
    let filtered = remove_every_nth_frame_indexes(frame_indexes, interval);

    if filtered.is_empty() {
        return Err("no-frames-selected");
    }

    Ok(ResolvedFrameSelection {
        selected_frame_count: filtered.len(),
        selected_frames: Some(filtered),
        base_frame_count: selection.base_frame_count,
    })
}

fn apply_quality_frame_drop_to_timeline_frames(
    timeline_frames: Vec<ResolvedTimelineFrame>,
    interval: u32,
) -> Result<Vec<ResolvedTimelineFrame>, &'static str> {
    if interval < 2 {
        return Ok(timeline_frames);
    }

    let filtered = timeline_frames
        .into_iter()
        .enumerate()
        .filter_map(|(index, frame)| {
            if (index + 1) % interval as usize == 0 {
                None
            } else {
                Some(frame)
            }
        })
        .collect::<Vec<_>>();

    if filtered.is_empty() {
        Err("no-frames-selected")
    } else {
        Ok(filtered)
    }
}

fn natural_selection_duration_seconds(selected_frame_count: usize, source_fps: f64) -> f64 {
    selected_frame_count as f64 / source_fps.max(1.0)
}

fn projected_selection_duration_seconds(
    selected_frame_count: usize,
    base_frame_count: Option<u32>,
    source_duration_seconds: Option<f64>,
    source_fps: f64,
) -> f64 {
    match (
        source_duration_seconds.filter(|duration| duration.is_finite() && *duration > 0.0),
        base_frame_count.filter(|count| *count > 0),
    ) {
        (Some(duration), Some(base_frame_count)) => {
            duration * selected_frame_count as f64 / f64::from(base_frame_count)
        }
        _ => natural_selection_duration_seconds(selected_frame_count, source_fps),
    }
}

fn checked_duration_us(durations_us: impl IntoIterator<Item = u64>) -> Result<u64, &'static str> {
    durations_us
        .into_iter()
        .try_fold(0_u64, |total, duration_us| {
            total
                .checked_add(duration_us)
                .ok_or("invalid-frame-selection")
        })
}

fn timeline_duration_us(timeline_frames: &[ResolvedTimelineFrame]) -> Result<u64, &'static str> {
    checked_duration_us(timeline_frames.iter().map(|frame| frame.duration_us))
}

fn sticker_frame_duration_us(frames: &[StickerFrame]) -> Result<u64, &'static str> {
    checked_duration_us(frames.iter().map(|frame| frame.duration_us))
}

fn timeline_average_fps(timeline_frames: &[ResolvedTimelineFrame], duration_us: u64) -> f64 {
    if duration_us == 0 {
        1.0
    } else {
        timeline_frames.len() as f64 / duration_us_to_seconds(duration_us)
    }
}

fn build_candidate_universe_fixed_duration(
    duration_seconds: f64,
    fps: u32,
    input_width: Option<u32>,
    input_height: Option<u32>,
    preset_strategy: &str,
    optimizer_goal: &str,
    locale: UiLocale,
) -> Vec<CandidatePreview> {
    let scale_ladder: Vec<f64> = match (input_width.unwrap_or(0), input_height.unwrap_or(0)) {
        (w, h) if optimizer_goal == "motion" && (w >= 640 || h >= 640) => {
            vec![1.0, 0.92, 0.84, 0.76, 0.68, 0.60, 0.52, 0.44]
        }
        (w, h) if optimizer_goal == "motion" && (w >= 400 || h >= 400) => {
            vec![1.0, 0.92, 0.84, 0.76, 0.68, 0.60]
        }
        _ if optimizer_goal == "motion" => vec![1.0, 0.92, 0.84, 0.76, 0.68, 0.60, 0.52, 0.44],
        _ if optimizer_goal == "quality" => vec![1.0, 0.96, 0.92, 0.88, 0.84, 0.80, 0.76],
        (w, h) if w >= 640 || h >= 640 => vec![1.0, 0.92, 0.84, 0.76, 0.68, 0.60, 0.52, 0.44],
        (w, h) if w >= 400 || h >= 400 => vec![1.0, 0.92, 0.84, 0.76, 0.68, 0.60],
        _ => vec![1.0, 0.92, 0.84, 0.76, 0.68],
    };

    let preset_ladder = preset_ladder_for_strategy(duration_seconds, preset_strategy);

    let mut candidates = Vec::new();

    for scale in &scale_ladder {
        for preset in &preset_ladder {
            let summary = locale::candidate_summary(locale, fps, *scale, preset, duration_seconds);
            let score = optimizer_goal_score(
                optimizer_goal,
                fps as f64,
                fps,
                *scale,
                preset,
                duration_seconds,
                duration_seconds,
                1.0,
            );

            candidates.push(CandidatePreview {
                id: format!(
                    "{}-{}-{}fps-{}scale-{}ms",
                    CANONICAL_FIT_MODE,
                    preset,
                    fps,
                    (scale * 100.0).round() as u32,
                    (duration_seconds * 1000.0).round() as u64
                ),
                rank: 0,
                duration_seconds,
                fps,
                content_scale: *scale,
                preset: (*preset).into(),
                fit_mode: CANONICAL_FIT_MODE.into(),
                score,
                source_similarity_score: score,
                relative_size_factor: relative_size_factor_for(1, *scale, preset),
                summary,
                frame_sample_step: 1,
            });
        }
    }

    candidates
}

fn build_candidate_ladder_fixed_duration(
    duration_seconds: f64,
    fps: u32,
    input_width: Option<u32>,
    input_height: Option<u32>,
    preset_strategy: &str,
    optimizer_goal: &str,
    search_budget: usize,
    locale: UiLocale,
) -> Vec<CandidatePreview> {
    select_ranked_candidate_subset(
        build_candidate_universe_fixed_duration(
            duration_seconds,
            fps,
            input_width,
            input_height,
            preset_strategy,
            optimizer_goal,
            locale,
        ),
        search_budget,
    )
}

fn build_filter_graph(
    fps: u32,
    content_scale: f64,
    input_width: Option<u32>,
    input_height: Option<u32>,
    crop_region: Option<ResolvedCropRegion>,
    selected_frames: Option<&[u32]>,
) -> String {
    let effective_width = crop_region.map(|crop| crop.width).or(input_width);
    let effective_height = crop_region.map(|crop| crop.height).or(input_height);
    let (target_width, target_height) =
        scaled_output_dimensions(effective_width, effective_height, content_scale);
    let crop_prefix = crop_region
        .map(|crop| format!("crop={}:{}:{}:{},", crop.width, crop.height, crop.x, crop.y))
        .unwrap_or_default();

    let selection_prefix = selected_frames
        .filter(|frames| !frames.is_empty())
        .map(|frames| {
            let eq_exprs: Vec<String> = frames
                .iter()
                .map(|frame| format!("eq(n,{frame})"))
                .collect();
            format!("select='{}',setpts=N/({fps}*TB),", eq_exprs.join("+"))
        })
        .unwrap_or_else(|| "setpts=PTS-STARTPTS,".into());

    format!(
        "{crop_prefix}{selection_prefix}fps={fps},scale={target_width}:{target_height},format=rgba,setsar=1"
    )
}

fn build_source_frame_select_filter(frame_indexes: &BTreeSet<u32>) -> String {
    let eq_exprs = frame_indexes
        .iter()
        .map(|frame| format!("eq(n,{frame})"))
        .collect::<Vec<_>>();
    format!("select='{}',format=rgba", eq_exprs.join("+"))
}

fn scaled_output_dimensions(
    input_width: Option<u32>,
    input_height: Option<u32>,
    content_scale: f64,
) -> (u32, u32) {
    let source_width = input_width.filter(|value| *value > 0).unwrap_or(320) as f64;
    let source_height = input_height.filter(|value| *value > 0).unwrap_or(320) as f64;
    let base_scale = (320.0 / source_width.max(source_height)).min(1.0);
    let effective_scale = (base_scale * content_scale).max(0.01);

    let target_width = (source_width * effective_scale).round().clamp(1.0, 320.0) as u32;
    let target_height = (source_height * effective_scale).round().clamp(1.0, 320.0) as u32;

    (target_width, target_height)
}

#[cfg(test)]
fn build_static_image_filter_graph(
    input_width: Option<u32>,
    input_height: Option<u32>,
    crop_region: Option<ResolvedCropRegion>,
) -> String {
    let effective_width = crop_region.map(|crop| crop.width).or(input_width);
    let effective_height = crop_region.map(|crop| crop.height).or(input_height);
    let (target_width, target_height) =
        scaled_output_dimensions(effective_width, effective_height, 1.0);
    let crop_prefix = crop_region
        .map(|crop| format!("crop={}:{}:{}:{},", crop.width, crop.height, crop.x, crop.y))
        .unwrap_or_default();

    format!("{crop_prefix}scale={target_width}:{target_height},format=rgba,setsar=1")
}

fn native_png_deflate_for_preset(preset: &str) -> PngDeflateCompression {
    match preset {
        "compact" => PngDeflateCompression::Level(7),
        "compactPlus" => PngDeflateCompression::Level(9),
        _ => PngDeflateCompression::Level(4),
    }
}

fn native_png_filter_for_preset(preset: &str) -> PngFilter {
    match preset {
        "compact" | "compactPlus" => PngFilter::Adaptive,
        _ => PngFilter::Paeth,
    }
}

fn frame_delay_microseconds(delay_ms: u32, delay_den_ms: u32) -> u64 {
    if delay_den_ms == 0 {
        u64::from(delay_ms) * 10
    } else {
        let numerator_us = u64::from(delay_ms) * 1_000;
        let denominator = u64::from(delay_den_ms);
        (numerator_us + denominator / 2) / denominator
    }
}

fn quantize_apng_delays(durations_us: &[u64]) -> Result<Vec<(u16, u16)>, &'static str> {
    let mut cumulative_us = 0_u64;
    let mut cumulative_ticks = Vec::with_capacity(durations_us.len());

    for duration_us in durations_us {
        if *duration_us < 100 {
            return Err("invalid-frame-duration");
        }

        cumulative_us = cumulative_us
            .checked_add(*duration_us)
            .ok_or("invalid-frame-selection")?;
        let rounded_ticks = cumulative_us / 100 + if cumulative_us % 100 >= 50 { 1 } else { 0 };
        cumulative_ticks.push(rounded_ticks);
    }

    let mut previous_ticks = 0_u64;
    cumulative_ticks
        .into_iter()
        .map(|end_ticks| {
            let frame_ticks = end_ticks
                .checked_sub(previous_ticks)
                .ok_or("invalid-frame-selection")?;
            let numerator = u16::try_from(frame_ticks).map_err(|_| "invalid-frame-duration")?;
            if numerator == 0 {
                return Err("invalid-frame-duration");
            }

            previous_ticks = end_ticks;
            Ok((numerator, 10_000))
        })
        .collect()
}

fn crop_rgba_image(source: &RgbaImage, crop_region: Option<ResolvedCropRegion>) -> RgbaImage {
    match crop_region {
        Some(crop) => {
            imageops::crop_imm(source, crop.x, crop.y, crop.width, crop.height).to_image()
        }
        None => source.clone(),
    }
}

fn transform_frame_for_candidate(
    source: &RgbaImage,
    content_scale: f64,
    crop_region: Option<ResolvedCropRegion>,
) -> RgbaImage {
    let cropped = crop_rgba_image(source, crop_region);
    let (target_width, target_height) =
        scaled_output_dimensions(Some(cropped.width()), Some(cropped.height()), content_scale);

    imageops::resize(&cropped, target_width, target_height, FilterType::Lanczos3)
}

fn transform_frame_for_static_png(
    source: &RgbaImage,
    crop_region: Option<ResolvedCropRegion>,
) -> RgbaImage {
    let cropped = crop_rgba_image(source, crop_region);
    let (target_width, target_height) =
        scaled_output_dimensions(Some(cropped.width()), Some(cropped.height()), 1.0);
    imageops::resize(&cropped, target_width, target_height, FilterType::Lanczos3)
}

fn prepare_frame_for_search(
    source: RgbaImage,
    crop_region: Option<ResolvedCropRegion>,
) -> RgbaImage {
    let effective_width = crop_region.map(|crop| crop.width).unwrap_or(source.width());
    let effective_height = crop_region
        .map(|crop| crop.height)
        .unwrap_or(source.height());
    let (target_width, target_height) =
        scaled_output_dimensions(Some(effective_width), Some(effective_height), 1.0);

    match crop_region {
        None if source.dimensions() == (target_width, target_height) => source,
        None => imageops::resize(&source, target_width, target_height, FilterType::Lanczos3),
        Some(crop) => {
            let cropped = imageops::crop_imm(&source, crop.x, crop.y, crop.width, crop.height);
            if (crop.width, crop.height) == (target_width, target_height) {
                cropped.to_image()
            } else {
                imageops::resize(&cropped, target_width, target_height, FilterType::Lanczos3)
            }
        }
    }
}

fn scale_prepared_frame_for_candidate(source: &RgbaImage, content_scale: f64) -> RgbaImage {
    let (target_width, target_height) =
        scaled_output_dimensions(Some(source.width()), Some(source.height()), content_scale);
    if source.dimensions() == (target_width, target_height) {
        source.clone()
    } else {
        imageops::resize(source, target_width, target_height, FilterType::Lanczos3)
    }
}

fn malformed_decoder_error(format: &'static str, error: impl std::fmt::Display) -> PipelineError {
    PipelineError::MalformedInput {
        format,
        reason: error.to_string(),
    }
}

fn image_decoder_error(
    format: &'static str,
    error: image::ImageError,
    limits: MediaLimits,
) -> PipelineError {
    match error {
        image::ImageError::Limits(limit_error) => match limit_error.kind() {
            ImageLimitErrorKind::DimensionError => PipelineError::LimitExceeded {
                resource: "image-dimensions",
                limit: u64::from(limits.max_dimension),
                actual: u64::from(limits.max_dimension).saturating_add(1),
            },
            ImageLimitErrorKind::InsufficientMemory => PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: limits.max_single_rgba_bytes,
                actual: limits.max_single_rgba_bytes.saturating_add(1),
            },
            _ => malformed_decoder_error(format, "decoder does not support the requested limits"),
        },
        error => malformed_decoder_error(format, error),
    }
}

fn run_decoder_boundary<T>(
    format: &'static str,
    operation: impl FnOnce() -> Result<T, PipelineError>,
) -> Result<T, PipelineError> {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(result) => result,
        Err(_) => Err(PipelineError::MalformedInput {
            format,
            reason: "decoder panicked".into(),
        }),
    }
}

fn checked_total_decoded_bytes(
    current: u64,
    frame_bytes: usize,
    limits: MediaLimits,
) -> Result<u64, PipelineError> {
    let actual = current
        .checked_add(u64::try_from(frame_bytes).unwrap_or(u64::MAX))
        .unwrap_or(u64::MAX);
    if actual > limits.max_total_decoded_bytes {
        return Err(PipelineError::LimitExceeded {
            resource: "decoded-bytes",
            limit: limits.max_total_decoded_bytes,
            actual,
        });
    }
    Ok(actual)
}

fn preflight_animation_decoded_bytes(
    frame_bytes: usize,
    frame_count: u64,
    limits: MediaLimits,
) -> Result<u64, PipelineError> {
    if frame_count > u64::from(limits.max_frame_count) {
        return Err(PipelineError::LimitExceeded {
            resource: "frame-count",
            limit: u64::from(limits.max_frame_count),
            actual: frame_count,
        });
    }
    let actual = u64::try_from(frame_bytes)
        .unwrap_or(u64::MAX)
        .checked_mul(frame_count)
        .unwrap_or(u64::MAX);
    if actual > limits.max_total_decoded_bytes {
        return Err(PipelineError::LimitExceeded {
            resource: "decoded-bytes",
            limit: limits.max_total_decoded_bytes,
            actual,
        });
    }
    Ok(actual)
}

fn collect_decoded_animation_frames<I>(
    format: &'static str,
    mut frames: I,
    preflight_frame_bytes: usize,
    preflight_frame_count: u64,
    limits: MediaLimits,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<Vec<StickerFrame>, PipelineError>
where
    I: Iterator<Item = image::ImageResult<image::Frame>>,
{
    preflight_animation_decoded_bytes(preflight_frame_bytes, preflight_frame_count, limits)?;
    let mut decoded = Vec::new();
    let mut total_decoded_bytes = 0u64;
    loop {
        checkpoint()?;
        let Some(frame) = frames.next() else {
            if u64::try_from(decoded.len()).unwrap_or(u64::MAX) != preflight_frame_count {
                return Err(PipelineError::MalformedInput {
                    format,
                    reason: "decoded frame count did not match metadata".into(),
                });
            }
            return Ok(decoded);
        };
        let next_count = u64::try_from(decoded.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if next_count > u64::from(limits.max_frame_count) {
            return Err(PipelineError::LimitExceeded {
                resource: "frame-count",
                limit: u64::from(limits.max_frame_count),
                actual: next_count,
            });
        }
        if next_count > preflight_frame_count {
            return Err(PipelineError::MalformedInput {
                format,
                reason: "decoder produced more frames than metadata declared".into(),
            });
        }

        let frame = frame.map_err(|error| image_decoder_error(format, error, limits))?;
        let frame = image_frame_to_sticker_frame(frame);
        let frame_bytes = checked_rgba_bytes(frame.pixels.width(), frame.pixels.height(), limits)?;
        total_decoded_bytes =
            checked_total_decoded_bytes(total_decoded_bytes, frame_bytes, limits)?;
        decoded.push(frame);
    }
}

fn select_decoded_animation_frame<I>(
    format: &'static str,
    mut frames: I,
    frame_index: usize,
    preflight_frame_bytes: usize,
    limits: MediaLimits,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<StickerFrame, PipelineError>
where
    I: Iterator<Item = image::ImageResult<image::Frame>>,
{
    let requested_count = u64::try_from(frame_index)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    if requested_count > u64::from(limits.max_frame_count) {
        return Err(PipelineError::LimitExceeded {
            resource: "frame-count",
            limit: u64::from(limits.max_frame_count),
            actual: requested_count,
        });
    }
    preflight_animation_decoded_bytes(preflight_frame_bytes, requested_count, limits)?;

    let mut total_decoded_bytes = 0u64;
    for current_index in 0..=frame_index {
        checkpoint()?;
        let frame = frames
            .next()
            .ok_or(PipelineError::InvalidRequest {
                reason: "invalid-frame-selection",
            })?
            .map_err(|error| image_decoder_error(format, error, limits))?;
        let frame = image_frame_to_sticker_frame(frame);
        let frame_bytes = checked_rgba_bytes(frame.pixels.width(), frame.pixels.height(), limits)?;
        total_decoded_bytes =
            checked_total_decoded_bytes(total_decoded_bytes, frame_bytes, limits)?;
        if current_index == frame_index {
            checkpoint()?;
            return Ok(frame);
        }
    }

    Err(PipelineError::InvalidRequest {
        reason: "invalid-frame-selection",
    })
}

fn visit_decoded_animation_frame_iterator<I, F>(
    format: &'static str,
    mut frames: I,
    expected_frame_count: Option<u64>,
    limits: MediaLimits,
    context: &OperationContext,
    mut visitor: F,
) -> Result<u32, PipelineError>
where
    I: Iterator<Item = image::ImageResult<image::Frame>>,
    F: FnMut(u32, image::Frame) -> Result<(), PipelineError>,
{
    let mut count = 0_u32;
    loop {
        context.checkpoint()?;
        let Some(decoded) = frames.next() else {
            if expected_frame_count.is_some_and(|expected| expected != u64::from(count)) {
                return Err(PipelineError::MalformedInput {
                    format,
                    reason: "decoded frame count did not match metadata".into(),
                });
            }
            return Ok(count);
        };
        let next_count = count.checked_add(1).ok_or(PipelineError::LimitExceeded {
            resource: "frame-count",
            limit: u64::from(limits.max_frame_count),
            actual: u64::MAX,
        })?;
        if next_count > limits.max_frame_count {
            return Err(PipelineError::LimitExceeded {
                resource: "frame-count",
                limit: u64::from(limits.max_frame_count),
                actual: u64::from(next_count),
            });
        }
        if expected_frame_count.is_some_and(|expected| u64::from(next_count) > expected) {
            return Err(PipelineError::MalformedInput {
                format,
                reason: "decoder produced more frames than metadata declared".into(),
            });
        }

        let frame = decoded.map_err(|error| image_decoder_error(format, error, limits))?;
        context.checkpoint()?;
        visitor(next_count, frame)?;
        context.checkpoint()?;
        count = next_count;
    }
}

fn open_limited_image_reader(
    input_path: &str,
    limits: MediaLimits,
) -> Result<(ImageReader<BufReader<File>>, fs::Metadata), PipelineError> {
    let file = File::open(input_path).map_err(|error| PipelineError::Io {
        operation: "open image input",
        message: error.to_string(),
    })?;
    let metadata = file.metadata().map_err(|error| PipelineError::Io {
        operation: "read image input metadata",
        message: error.to_string(),
    })?;
    let metadata = validate_file_metadata(metadata, limits)?;
    let mut reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|error| PipelineError::Io {
            operation: "detect image format",
            message: error.to_string(),
        })?;
    reader.limits(image_decode_limits(limits));
    Ok((reader, metadata))
}

fn validate_still_decoder_allocation(
    width: u32,
    height: u32,
    color_type: image::ColorType,
    native_total_bytes: u64,
    limits: MediaLimits,
) -> Result<(), PipelineError> {
    let rgba_bytes = u64::try_from(checked_rgba_bytes(width, height, limits)?).unwrap_or(u64::MAX);
    if native_total_bytes > limits.max_total_decoded_bytes {
        return Err(PipelineError::LimitExceeded {
            resource: "decoded-bytes",
            limit: limits.max_total_decoded_bytes,
            actual: native_total_bytes,
        });
    }

    let peak_bytes = if color_type == image::ColorType::Rgba8 {
        native_total_bytes
    } else {
        native_total_bytes
            .checked_add(rgba_bytes)
            .unwrap_or(u64::MAX)
    };
    if peak_bytes > limits.max_total_decoded_bytes {
        return Err(PipelineError::LimitExceeded {
            resource: "decoded-bytes",
            limit: limits.max_total_decoded_bytes,
            actual: peak_bytes,
        });
    }

    Ok(())
}

fn decode_still_rgba_image(
    input_path: &str,
    limits: MediaLimits,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<RgbaImage, PipelineError> {
    run_decoder_boundary("image", || {
        checkpoint()?;
        let (reader, _) = open_limited_image_reader(input_path, limits)?;
        let decoder = reader
            .into_decoder()
            .map_err(|error| image_decoder_error("image", error, limits))?;
        let (width, height) = decoder.dimensions();
        validate_still_decoder_allocation(
            width,
            height,
            decoder.color_type(),
            decoder.total_bytes(),
            limits,
        )?;
        checkpoint()?;
        let image = image::DynamicImage::from_decoder(decoder)
            .map_err(|error| image_decoder_error("image", error, limits))?;
        checked_rgba_bytes(image.width(), image.height(), limits)?;
        let pixels = image.into_rgba8();
        checkpoint()?;
        Ok(pixels)
    })
}

fn decode_gif_animation_frames(
    input_path: &str,
    limits: MediaLimits,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<Vec<StickerFrame>, PipelineError> {
    run_decoder_boundary("gif", || {
        checkpoint()?;
        let mut file = File::open(input_path).map_err(|error| PipelineError::Io {
            operation: "open GIF input",
            message: error.to_string(),
        })?;
        let metadata = file.metadata().map_err(|error| PipelineError::Io {
            operation: "read GIF input metadata",
            message: error.to_string(),
        })?;
        validate_file_metadata(metadata, limits)?;
        let (metadata_width, metadata_height, frame_count) = {
            let mut metadata_reader = BufReader::new(&mut file);
            read_gif_frame_metadata(&mut metadata_reader, limits, &mut checkpoint)?
        };
        file.seek(SeekFrom::Start(0))
            .map_err(|error| pipeline_io_error("rewind GIF input", error))?;
        let mut decoder = ImageGifDecoder::new(BufReader::new(file))
            .map_err(|error| image_decoder_error("gif", error, limits))?;
        decoder
            .set_limits(image_decode_limits(limits))
            .map_err(|error| image_decoder_error("gif", error, limits))?;
        let (width, height) = decoder.dimensions();
        if (width, height) != (metadata_width, metadata_height) {
            return Err(malformed_gif("GIF dimensions changed during decode setup"));
        }
        let frame_bytes = checked_rgba_bytes(width, height, limits)?;
        preflight_animation_decoded_bytes(frame_bytes, u64::from(frame_count), limits)?;
        collect_decoded_animation_frames(
            "gif",
            decoder.into_frames(),
            frame_bytes,
            u64::from(frame_count),
            limits,
            || checkpoint(),
        )
    })
}

fn image_frame_to_sticker_frame(frame: image::Frame) -> StickerFrame {
    let (delay_ms, delay_den_ms) = frame.delay().numer_denom_ms();
    StickerFrame {
        pixels: frame.into_buffer(),
        duration_us: frame_delay_microseconds(delay_ms, delay_den_ms),
    }
}

fn malformed_gif(reason: impl Into<String>) -> PipelineError {
    PipelineError::MalformedInput {
        format: "gif",
        reason: reason.into(),
    }
}

fn read_gif_exact<R: Read>(
    reader: &mut R,
    buffer: &mut [u8],
    context: &'static str,
) -> Result<(), PipelineError> {
    reader
        .read_exact(buffer)
        .map_err(|_| malformed_gif(format!("truncated {context}")))
}

fn skip_gif_bytes<R: Read>(
    reader: &mut R,
    mut remaining: usize,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> Result<(), PipelineError> {
    let mut scratch = [0u8; 64 * 1024];
    while remaining > 0 {
        checkpoint()?;
        let count = remaining.min(scratch.len());
        read_gif_exact(reader, &mut scratch[..count], "GIF block")?;
        remaining -= count;
    }
    Ok(())
}

fn skip_gif_sub_blocks<R: Read>(
    reader: &mut R,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> Result<(), PipelineError> {
    loop {
        checkpoint()?;
        let mut length = [0u8; 1];
        read_gif_exact(reader, &mut length, "GIF sub-block length")?;
        if length[0] == 0 {
            return Ok(());
        }
        skip_gif_bytes(reader, usize::from(length[0]), checkpoint)?;
    }
}

fn read_gif_frame_metadata<R: Read>(
    reader: &mut R,
    limits: MediaLimits,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> Result<(u32, u32, u32), PipelineError> {
    let mut signature = [0u8; 6];
    read_gif_exact(reader, &mut signature, "GIF signature")?;
    if !matches!(&signature, b"GIF87a" | b"GIF89a") {
        return Err(malformed_gif("invalid GIF signature"));
    }

    let mut logical_screen = [0u8; 7];
    read_gif_exact(reader, &mut logical_screen, "GIF logical screen descriptor")?;
    let width = u32::from(u16::from_le_bytes([logical_screen[0], logical_screen[1]]));
    let height = u32::from(u16::from_le_bytes([logical_screen[2], logical_screen[3]]));
    if width == 0 || height == 0 {
        return Err(malformed_gif("GIF dimensions must be non-zero"));
    }
    checked_rgba_bytes(width, height, limits)?;

    let packed = logical_screen[4];
    if packed & 0x80 != 0 {
        let entries = 1usize << (usize::from(packed & 0x07) + 1);
        skip_gif_bytes(reader, entries * 3, checkpoint)?;
    }

    let mut frame_count = 0u32;
    loop {
        checkpoint()?;
        let mut introducer = [0u8; 1];
        read_gif_exact(reader, &mut introducer, "GIF block introducer")?;
        match introducer[0] {
            0x3b => {
                if frame_count == 0 {
                    return Err(malformed_gif("GIF did not contain an image frame"));
                }
                return Ok((width, height, frame_count));
            }
            0x21 => {
                let mut extension_label = [0u8; 1];
                read_gif_exact(reader, &mut extension_label, "GIF extension label")?;
                skip_gif_sub_blocks(reader, checkpoint)?;
            }
            0x2c => {
                frame_count =
                    frame_count
                        .checked_add(1)
                        .ok_or_else(|| PipelineError::LimitExceeded {
                            resource: "frame-count",
                            limit: u64::from(limits.max_frame_count),
                            actual: u64::MAX,
                        })?;
                if frame_count > limits.max_frame_count {
                    return Err(PipelineError::LimitExceeded {
                        resource: "frame-count",
                        limit: u64::from(limits.max_frame_count),
                        actual: u64::from(frame_count),
                    });
                }
                let mut descriptor = [0u8; 9];
                read_gif_exact(reader, &mut descriptor, "GIF image descriptor")?;
                let frame_width = u32::from(u16::from_le_bytes([descriptor[4], descriptor[5]]));
                let frame_height = u32::from(u16::from_le_bytes([descriptor[6], descriptor[7]]));
                if frame_width == 0 || frame_height == 0 {
                    return Err(malformed_gif("GIF frame dimensions must be non-zero"));
                }
                if descriptor[8] & 0x80 != 0 {
                    let entries = 1usize << (usize::from(descriptor[8] & 0x07) + 1);
                    skip_gif_bytes(reader, entries * 3, checkpoint)?;
                }
                let mut lzw_minimum_code_size = [0u8; 1];
                read_gif_exact(
                    reader,
                    &mut lzw_minimum_code_size,
                    "GIF LZW minimum code size",
                )?;
                skip_gif_sub_blocks(reader, checkpoint)?;
            }
            _ => return Err(malformed_gif("invalid GIF block introducer")),
        }
    }
}

fn decode_gif_animation_frame(
    input_path: &str,
    frame_index: usize,
    limits: MediaLimits,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<StickerFrame, PipelineError> {
    run_decoder_boundary("gif", || {
        checkpoint()?;
        let mut file = File::open(input_path).map_err(|error| PipelineError::Io {
            operation: "open GIF input",
            message: error.to_string(),
        })?;
        let metadata = file.metadata().map_err(|error| PipelineError::Io {
            operation: "read GIF input metadata",
            message: error.to_string(),
        })?;
        validate_file_metadata(metadata, limits)?;
        let (metadata_width, metadata_height, frame_count) = {
            let mut metadata_reader = BufReader::new(&mut file);
            read_gif_frame_metadata(&mut metadata_reader, limits, &mut checkpoint)?
        };
        if u64::try_from(frame_index).unwrap_or(u64::MAX) >= u64::from(frame_count) {
            return Err(PipelineError::InvalidRequest {
                reason: "invalid-frame-selection",
            });
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|error| pipeline_io_error("rewind GIF input", error))?;
        let mut decoder = ImageGifDecoder::new(BufReader::new(file))
            .map_err(|error| image_decoder_error("gif", error, limits))?;
        decoder
            .set_limits(image_decode_limits(limits))
            .map_err(|error| image_decoder_error("gif", error, limits))?;
        let (width, height) = decoder.dimensions();
        if (width, height) != (metadata_width, metadata_height) {
            return Err(malformed_gif("GIF dimensions changed during decode setup"));
        }
        let frame_bytes = checked_rgba_bytes(width, height, limits)?;
        select_decoded_animation_frame(
            "gif",
            decoder.into_frames(),
            frame_index,
            frame_bytes,
            limits,
            || checkpoint(),
        )
    })
}

fn decode_apng_animation_frames(
    input_path: &str,
    limits: MediaLimits,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<Vec<StickerFrame>, PipelineError> {
    run_decoder_boundary("png", || {
        checkpoint()?;
        let mut file = File::open(input_path).map_err(|error| PipelineError::Io {
            operation: "open PNG input",
            message: error.to_string(),
        })?;
        let metadata = file.metadata().map_err(|error| PipelineError::Io {
            operation: "read PNG input metadata",
            message: error.to_string(),
        })?;
        let file_length = validate_file_metadata(metadata, limits)?.len();
        let animation_metadata = {
            let mut metadata_reader = BufReader::new(&mut file);
            read_png_metadata_from_reader(&mut metadata_reader, file_length, limits, || {
                checkpoint()
            })?
            .animation
        };
        let frame_count = animation_metadata
            .and_then(|metadata| metadata.frame_count)
            .ok_or_else(|| malformed_decoder_error("png", "missing APNG animation control"))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| pipeline_io_error("rewind PNG input", error))?;
        let decoder =
            ImagePngDecoder::with_limits(BufReader::new(file), image_decode_limits(limits))
                .map_err(|error| image_decoder_error("png", error, limits))?;
        let (width, height) = decoder.dimensions();
        let frame_bytes = checked_rgba_bytes(width, height, limits)?;
        preflight_animation_decoded_bytes(frame_bytes, frame_count, limits)?;
        let apng_decoder = decoder
            .apng()
            .map_err(|error| image_decoder_error("png", error, limits))?;
        collect_decoded_animation_frames(
            "png",
            apng_decoder.into_frames(),
            frame_bytes,
            frame_count,
            limits,
            || checkpoint(),
        )
    })
}

fn decode_apng_animation_frame(
    input_path: &str,
    frame_index: usize,
    limits: MediaLimits,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<StickerFrame, PipelineError> {
    run_decoder_boundary("png", || {
        checkpoint()?;
        let mut file = File::open(input_path).map_err(|error| PipelineError::Io {
            operation: "open PNG input",
            message: error.to_string(),
        })?;
        let metadata = file.metadata().map_err(|error| PipelineError::Io {
            operation: "read PNG input metadata",
            message: error.to_string(),
        })?;
        let file_length = validate_file_metadata(metadata, limits)?.len();
        let animation_metadata = {
            let mut metadata_reader = BufReader::new(&mut file);
            read_png_metadata_from_reader(&mut metadata_reader, file_length, limits, || {
                checkpoint()
            })?
            .animation
        };
        let frame_count = animation_metadata
            .and_then(|metadata| metadata.frame_count)
            .ok_or_else(|| malformed_decoder_error("png", "missing APNG animation control"))?;
        let requested_count = u64::try_from(frame_index)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if requested_count > frame_count {
            return Err(PipelineError::InvalidRequest {
                reason: "invalid-frame-selection",
            });
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|error| pipeline_io_error("rewind PNG input", error))?;
        let decoder =
            ImagePngDecoder::with_limits(BufReader::new(file), image_decode_limits(limits))
                .map_err(|error| image_decoder_error("png", error, limits))?;
        let (width, height) = decoder.dimensions();
        let frame_bytes = checked_rgba_bytes(width, height, limits)?;
        let apng_decoder = decoder
            .apng()
            .map_err(|error| image_decoder_error("png", error, limits))?;
        select_decoded_animation_frame(
            "png",
            apng_decoder.into_frames(),
            frame_index,
            frame_bytes,
            limits,
            || checkpoint(),
        )
    })
}

pub(crate) fn visit_native_animation_frames<F>(
    input_path: &Path,
    limits: MediaLimits,
    context: &OperationContext,
    mut visitor: F,
) -> Result<u32, PipelineError>
where
    F: FnMut(u32, image::Frame) -> Result<(), PipelineError>,
{
    let extension = input_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    match extension.as_str() {
        "gif" => run_decoder_boundary("gif", || {
            context.checkpoint()?;
            let mut file = File::open(input_path).map_err(|error| PipelineError::Io {
                operation: "open GIF input",
                message: error.to_string(),
            })?;
            let metadata = file.metadata().map_err(|error| PipelineError::Io {
                operation: "read GIF input metadata",
                message: error.to_string(),
            })?;
            validate_file_metadata(metadata, limits)?;
            let (metadata_width, metadata_height, frame_count) = {
                let mut metadata_reader = BufReader::new(&mut file);
                read_gif_frame_metadata(&mut metadata_reader, limits, &mut || context.checkpoint())?
            };
            file.seek(SeekFrom::Start(0))
                .map_err(|error| pipeline_io_error("rewind GIF input", error))?;
            let mut decoder = ImageGifDecoder::new(BufReader::new(file))
                .map_err(|error| image_decoder_error("gif", error, limits))?;
            decoder
                .set_limits(image_decode_limits(limits))
                .map_err(|error| image_decoder_error("gif", error, limits))?;
            if decoder.dimensions() != (metadata_width, metadata_height) {
                return Err(malformed_gif("GIF dimensions changed during decode setup"));
            }
            visit_decoded_animation_frame_iterator(
                "gif",
                decoder.into_frames(),
                Some(u64::from(frame_count)),
                limits,
                context,
                |source_frame_id, frame| visitor(source_frame_id, frame),
            )
        }),
        "apng" | "png" => run_decoder_boundary("png", || {
            context.checkpoint()?;
            let mut file = File::open(input_path).map_err(|error| PipelineError::Io {
                operation: "open PNG input",
                message: error.to_string(),
            })?;
            let metadata = file.metadata().map_err(|error| PipelineError::Io {
                operation: "read PNG input metadata",
                message: error.to_string(),
            })?;
            let file_length = validate_file_metadata(metadata, limits)?.len();
            let frame_count = {
                let mut metadata_reader = BufReader::new(&mut file);
                read_png_metadata_from_reader(&mut metadata_reader, file_length, limits, || {
                    context.checkpoint()
                })?
                .animation
                .and_then(|metadata| metadata.frame_count)
                .ok_or_else(|| malformed_decoder_error("png", "missing APNG animation control"))?
            };
            file.seek(SeekFrom::Start(0))
                .map_err(|error| pipeline_io_error("rewind PNG input", error))?;
            let decoder =
                ImagePngDecoder::with_limits(BufReader::new(file), image_decode_limits(limits))
                    .map_err(|error| image_decoder_error("png", error, limits))?;
            let apng_decoder = decoder
                .apng()
                .map_err(|error| image_decoder_error("png", error, limits))?;
            visit_decoded_animation_frame_iterator(
                "png",
                apng_decoder.into_frames(),
                Some(frame_count),
                limits,
                context,
                |source_frame_id, frame| visitor(source_frame_id, frame),
            )
        }),
        _ => Err(PipelineError::InvalidRequest {
            reason: "unsupported-source-format",
        }),
    }
}

fn resolved_authored_timeline(
    request: FramePreparationRequest<'_>,
) -> Result<Option<Vec<ResolvedTimelineFrame>>, PipelineError> {
    if let Some(frames) = request.resolved_timeline_frames {
        return Ok(Some(frames.to_vec()));
    }
    request
        .timeline_frames
        .map(|frames| {
            frames
                .iter()
                .map(|frame| {
                    if frame.source_frame_id == 0 || frame.duration_us < 100 {
                        return Err(PipelineError::InvalidRequest {
                            reason: "invalid-frame-selection",
                        });
                    }
                    Ok(ResolvedTimelineFrame {
                        source_frame_index: frame.source_frame_id - 1,
                        duration_us: frame.duration_us,
                    })
                })
                .collect()
        })
        .transpose()
}

fn required_prepared_source_indexes(
    request: FramePreparationRequest<'_>,
) -> Result<Option<BTreeSet<u32>>, PipelineError> {
    if let Some(timeline) = resolved_authored_timeline(request)? {
        return Ok(Some(
            timeline
                .into_iter()
                .map(|frame| frame.source_frame_index)
                .collect(),
        ));
    }
    Ok(request
        .selected_frame_indexes
        .map(|indexes| indexes.iter().copied().collect()))
}

fn prepared_sequence_base_fps(sequence: &[ResolvedTimelineFrame]) -> Result<u32, PipelineError> {
    let duration_us = checked_timeline_duration(sequence)?;
    if duration_us == 0 {
        return Ok(1);
    }
    let numerator = sequence.len() as u128 * 1_000_000_u128;
    let rounded = (numerator + u128::from(duration_us) / 2) / u128::from(duration_us);
    Ok(u32::try_from(rounded).unwrap_or(u32::MAX).max(1))
}

fn prepare_native_search_source(
    request: FramePreparationRequest<'_>,
    context: &OperationContext,
    limits: MediaLimits,
) -> Result<PreparedSearchSource, PipelineError> {
    let requested_indexes = required_prepared_source_indexes(request)?;
    let mut decoded_bytes = 0_usize;
    let mut frames = Vec::new();
    let mut native_timing_grid = Vec::new();
    let mut decoded_dimensions = None;
    let mut resolved_crop_region = None;

    let decoded_count = visit_native_animation_frames(
        request.input_path,
        limits,
        context,
        |source_frame_id, frame| {
            let source_frame_index = source_frame_id - 1;
            let sticker_frame = image_frame_to_sticker_frame(frame);
            let dimensions = sticker_frame.pixels.dimensions();
            if let Some(expected_dimensions) = decoded_dimensions {
                if dimensions != expected_dimensions {
                    return Err(PipelineError::MalformedInput {
                        format: "animation",
                        reason: "decoded frame dimensions changed during preparation".into(),
                    });
                }
            } else {
                decoded_dimensions = Some(dimensions);
                resolved_crop_region = resolve_crop_region(
                    request.crop_region,
                    Some(dimensions.0),
                    Some(dimensions.1),
                    request.locale,
                )
                .map_err(|_| PipelineError::InvalidRequest {
                    reason: "invalid-crop",
                })?;
            }

            native_timing_grid.push(ResolvedTimelineFrame {
                source_frame_index,
                duration_us: sticker_frame.duration_us,
            });
            if requested_indexes
                .as_ref()
                .is_some_and(|indexes| !indexes.contains(&source_frame_index))
            {
                return Ok(());
            }

            context.checkpoint()?;
            let pixels = prepare_frame_for_search(sticker_frame.pixels, resolved_crop_region);
            let frame_bytes = checked_rgba_bytes(pixels.width(), pixels.height(), limits)?;
            decoded_bytes = checked_prepared_bytes(decoded_bytes, frame_bytes)?;
            frames.push(PreparedFrame {
                source_frame_id,
                pixels: Arc::new(pixels),
                duration_us: sticker_frame.duration_us,
            });
            context.checkpoint()
        },
    )?;

    if request
        .base_frame_count
        .is_some_and(|expected| expected != decoded_count)
    {
        return Err(PipelineError::InvalidRequest {
            reason: "invalid-frame-selection",
        });
    }

    let authored_timeline = resolved_authored_timeline(request)?;
    let (base_sequence, timing_authority) = match authored_timeline {
        Some(timeline) => (timeline, TimelineTimingAuthority::Authored),
        None => (
            project_timing_grid(&native_timing_grid, request.selected_frame_indexes)?,
            TimelineTimingAuthority::Native,
        ),
    };
    if base_sequence.is_empty()
        || base_sequence
            .iter()
            .any(|frame| frame.source_frame_index >= decoded_count)
    {
        return Err(PipelineError::InvalidRequest {
            reason: "invalid-frame-selection",
        });
    }
    for frame in &base_sequence {
        let source_frame_id = frame.source_frame_index + 1;
        if !frames
            .iter()
            .any(|prepared| prepared.source_frame_id == source_frame_id)
        {
            return Err(PipelineError::InvalidRequest {
                reason: "invalid-frame-selection",
            });
        }
    }

    let base_fps = prepared_sequence_base_fps(&base_sequence)?;
    Ok(PreparedSearchSource {
        frames,
        base_sequence,
        timing_authority,
        base_fps,
        decoded_bytes,
        tool_source: "native".into(),
        tool_command: None,
        tool_detail: Some(locale::native_apng_encode_detail(request.locale)),
    })
}

struct DefaultFrameSourceLoader;

fn build_prepared_video_filter(
    frame_indexes: &BTreeSet<u32>,
    crop_region: Option<ResolvedCropRegion>,
    target_width: u32,
    target_height: u32,
) -> String {
    let selection = frame_indexes
        .iter()
        .map(|frame| format!("eq(n,{frame})"))
        .collect::<Vec<_>>()
        .join("+");
    let crop = crop_region
        .map(|crop| format!("crop={}:{}:{}:{},", crop.width, crop.height, crop.x, crop.y))
        .unwrap_or_default();
    format!(
        "select='{selection}',{crop}scale={target_width}:{target_height}:flags=lanczos,format=rgba,setsar=1"
    )
}

fn prepare_video_base_sequence(
    request: FramePreparationRequest<'_>,
) -> Result<(Vec<ResolvedTimelineFrame>, TimelineTimingAuthority), PipelineError> {
    if let Some(authored) = resolved_authored_timeline(request)? {
        if authored.is_empty() {
            return Err(PipelineError::InvalidRequest {
                reason: "no-frames-selected",
            });
        }
        return Ok((authored, TimelineTimingAuthority::Authored));
    }

    let base_frame_count = request.base_frame_count.filter(|count| *count > 0).ok_or(
        PipelineError::InvalidRequest {
            reason: "invalid-frame-selection",
        },
    )?;
    let timing_grid = build_inspected_timing_grid(
        base_frame_count,
        request.source_duration_seconds,
        request.avg_fps,
    )?;
    Ok((
        project_timing_grid(&timing_grid, request.selected_frame_indexes)?,
        TimelineTimingAuthority::Inspected,
    ))
}

fn required_video_source_indexes(
    base_sequence: &[ResolvedTimelineFrame],
    base_fps: u32,
    optimizer_goal: &str,
    context: &OperationContext,
) -> Result<BTreeSet<u32>, PipelineError> {
    let placeholder = PreparedSearchSource {
        frames: Vec::new(),
        base_sequence: base_sequence.to_vec(),
        timing_authority: TimelineTimingAuthority::Inspected,
        base_fps,
        decoded_bytes: 0,
        tool_source: "pending".into(),
        tool_command: None,
        tool_detail: None,
    };
    let mut required = BTreeSet::new();
    let duration_seconds = duration_us_to_seconds(checked_timeline_duration(base_sequence)?);
    let frame_sample_steps = frame_sample_steps_for_goal(base_sequence.len(), optimizer_goal);
    for frame_sample_step in frame_sample_steps {
        for fps in (1..=30).rev() {
            context.checkpoint()?;
            let candidate = CandidatePreview {
                id: "preparation-union".into(),
                rank: 0,
                duration_seconds,
                fps,
                content_scale: 1.0,
                preset: "standard".into(),
                fit_mode: CANONICAL_FIT_MODE.into(),
                score: 0.0,
                source_similarity_score: 0.0,
                relative_size_factor: relative_size_factor_for(frame_sample_step, 1.0, "standard"),
                summary: String::new(),
                frame_sample_step,
            };
            let sequence = build_candidate_output_sequence(&placeholder, &candidate, context)?;
            required.extend(sequence.frames.iter().map(|frame| frame.source_frame_index));
        }
    }
    if required.is_empty() {
        return Err(PipelineError::InvalidRequest {
            reason: "plan-invalid",
        });
    }
    Ok(required)
}

fn prepare_video_search_source(
    request: FramePreparationRequest<'_>,
    _plan: &OptimizerPlanResponse,
    context: &OperationContext,
    limits: MediaLimits,
) -> Result<PreparedSearchSource, PipelineError> {
    let (base_sequence, timing_authority) = prepare_video_base_sequence(request)?;
    let base_fps = prepared_sequence_base_fps(&base_sequence)?;
    let required_indexes =
        required_video_source_indexes(&base_sequence, base_fps, request.optimizer_goal, context)?;
    if required_indexes.len() > limits.max_frame_count as usize {
        return Err(PipelineError::LimitExceeded {
            resource: "frame-count",
            limit: u64::from(limits.max_frame_count),
            actual: u64::try_from(required_indexes.len()).unwrap_or(u64::MAX),
        });
    }

    let source_width =
        request
            .input_width
            .filter(|width| *width > 0)
            .ok_or(PipelineError::InvalidRequest {
                reason: "invalid-crop",
            })?;
    let source_height =
        request
            .input_height
            .filter(|height| *height > 0)
            .ok_or(PipelineError::InvalidRequest {
                reason: "invalid-crop",
            })?;
    let crop_region = resolve_crop_region(
        request.crop_region,
        Some(source_width),
        Some(source_height),
        request.locale,
    )
    .map_err(|_| PipelineError::InvalidRequest {
        reason: "invalid-crop",
    })?;
    let effective_width = crop_region.map(|crop| crop.width).unwrap_or(source_width);
    let effective_height = crop_region.map(|crop| crop.height).unwrap_or(source_height);
    let (target_width, target_height) =
        scaled_output_dimensions(Some(effective_width), Some(effective_height), 1.0);
    let frame_size = checked_rgba_bytes(target_width, target_height, limits)?;
    let stdout_limit =
        frame_size
            .checked_mul(required_indexes.len())
            .ok_or(PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: MAX_PREPARED_SEARCH_BYTES as u64,
                actual: u64::MAX,
            })?;
    checked_prepared_bytes(0, stdout_limit)?;

    let filter =
        build_prepared_video_filter(&required_indexes, crop_region, target_width, target_height);
    let resolution = resolve_tool("ffmpeg", request.locale)
        .map_err(|_| PipelineError::ToolMissing { tool: "ffmpeg" })?;
    let args = [
        OsString::from("-v"),
        OsString::from("error"),
        OsString::from("-i"),
        request.input_path.as_os_str().to_os_string(),
        OsString::from("-map"),
        OsString::from("0:v:0"),
        OsString::from("-vf"),
        OsString::from(filter),
        OsString::from("-pix_fmt"),
        OsString::from("rgba"),
        OsString::from("-fps_mode"),
        OsString::from("passthrough"),
        OsString::from("-f"),
        OsString::from("rawvideo"),
        OsString::from("-an"),
        OsString::from("-"),
    ];
    let mut index_iter = required_indexes.iter().copied();
    let mut frames = Vec::with_capacity(required_indexes.len());
    let mut decoded_bytes = 0_usize;
    let summary = stream_fixed_rgba_frames(
        &resolution.command,
        &args,
        frame_size,
        required_indexes.len(),
        ProcessLimits {
            timeout: RAW_DECODE_PROCESS_TIMEOUT,
            max_stdout_bytes: stdout_limit,
            max_stderr_bytes: PROCESS_CAPTURE_LIMIT_BYTES,
        },
        context,
        |frame| {
            context.checkpoint()?;
            let source_frame_index =
                index_iter
                    .next()
                    .ok_or(PipelineError::MalformedProcessOutput {
                        reason: "raw RGBA output exceeded selected source indexes".into(),
                    })?;
            decoded_bytes = checked_prepared_bytes(decoded_bytes, frame.len())?;
            let pixels = rgba_frame_from_bytes(target_width, target_height, frame)?;
            let duration_us = base_sequence
                .iter()
                .find(|prepared| prepared.source_frame_index == source_frame_index)
                .map(|prepared| prepared.duration_us)
                .unwrap_or_default();
            frames.push(PreparedFrame {
                source_frame_id: source_frame_index + 1,
                pixels: Arc::new(pixels),
                duration_us,
            });
            context.checkpoint()
        },
    )?;
    validate_exact_selected_frame_stream(required_indexes.len(), summary.frame_count)?;
    if index_iter.next().is_some() {
        return Err(PipelineError::MalformedProcessOutput {
            reason: "raw RGBA output did not cover selected source indexes".into(),
        });
    }

    Ok(PreparedSearchSource {
        frames,
        base_sequence,
        timing_authority,
        base_fps,
        decoded_bytes,
        tool_source: resolution.source.into(),
        tool_command: Some(resolution.command_display),
        tool_detail: resolution.fallback_reason,
    })
}

impl FrameSourceLoader for DefaultFrameSourceLoader {
    fn prepare(
        &self,
        request: FramePreparationRequest<'_>,
        plan: &OptimizerPlanResponse,
        context: &OperationContext,
        limits: MediaLimits,
    ) -> Result<PreparedSearchSource, PipelineError> {
        let extension = request
            .input_path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match extension.as_str() {
            "gif" | "apng" | "png" => prepare_native_search_source(request, context, limits),
            _ => prepare_video_search_source(request, plan, context, limits),
        }
    }
}

fn decode_native_animation_frames(
    input_path: &str,
    limits: MediaLimits,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<Vec<StickerFrame>, PipelineError> {
    match lowercase_source_extension(input_path).as_deref() {
        Some("gif") => decode_gif_animation_frames(input_path, limits, || checkpoint()),
        Some("apng") | Some("png") => {
            decode_apng_animation_frames(input_path, limits, || checkpoint())
        }
        _ => Err(PipelineError::InvalidRequest {
            reason: "unsupported-source-format",
        }),
    }
}

fn decode_native_animation_frame(
    input_path: &str,
    source_frame_id: u32,
    limits: MediaLimits,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<StickerFrame, PipelineError> {
    if source_frame_id == 0 {
        return Err(PipelineError::InvalidRequest {
            reason: "invalid-frame-selection",
        });
    }

    let frame_index = (source_frame_id - 1) as usize;
    match lowercase_source_extension(input_path).as_deref() {
        Some("gif") => decode_gif_animation_frame(input_path, frame_index, limits, || checkpoint()),
        Some("apng") | Some("png") => {
            decode_apng_animation_frame(input_path, frame_index, limits, || checkpoint())
        }
        _ => Err(PipelineError::InvalidRequest {
            reason: "unsupported-source-format",
        }),
    }
}

fn write_native_png<W: Write>(writer: W, pixels: &RgbaImage) -> Result<(), PipelineError> {
    let mut encoder = NativePngEncoder::new(writer, pixels.width(), pixels.height());
    encoder.set_color(PngColorType::Rgba);
    encoder.set_depth(PngBitDepth::Eight);
    encoder.set_deflate_compression(PngDeflateCompression::Level(9));
    encoder.set_filter(PngFilter::Adaptive);
    let mut png_writer = encoder
        .write_header()
        .map_err(|error| pipeline_io_error("write PNG header", error))?;
    png_writer
        .write_image_data(pixels.as_raw())
        .map_err(|error| pipeline_io_error("write PNG pixels", error))?;
    png_writer
        .finish()
        .map_err(|error| pipeline_io_error("finish PNG output", error))
}

fn encode_native_png_bytes(pixels: &RgbaImage) -> Result<Vec<u8>, PipelineError> {
    let mut bytes = Vec::new();
    {
        let mut encoder = NativePngEncoder::new(&mut bytes, pixels.width(), pixels.height());
        encoder.set_color(PngColorType::Rgba);
        encoder.set_depth(PngBitDepth::Eight);
        encoder.set_deflate_compression(PngDeflateCompression::Level(9));
        encoder.set_filter(PngFilter::Adaptive);
        let mut png_writer = encoder
            .write_header()
            .map_err(|error| pipeline_io_error("encode preview PNG header", error))?;
        png_writer
            .write_image_data(pixels.as_raw())
            .map_err(|error| pipeline_io_error("encode preview PNG pixels", error))?;
        png_writer
            .finish()
            .map_err(|error| pipeline_io_error("finish preview PNG", error))?;
    }

    Ok(bytes)
}

fn normalize_preview_source_frame_ids(source_frame_ids: &[u32]) -> Result<Vec<u32>, PipelineError> {
    if source_frame_ids.len() > MAX_MEDIA_FRAME_COUNT {
        return Err(PipelineError::LimitExceeded {
            resource: "frame-count",
            limit: MAX_MEDIA_FRAME_COUNT as u64,
            actual: u64::try_from(source_frame_ids.len()).unwrap_or(u64::MAX),
        });
    }

    let mut seen = BTreeSet::new();
    let mut normalized = Vec::with_capacity(source_frame_ids.len().min(MAX_PREVIEW_BATCH_IDS));
    for source_frame_id in source_frame_ids.iter().copied() {
        if source_frame_id == 0 {
            return Err(PipelineError::InvalidRequest {
                reason: "invalid-frame-selection",
            });
        }
        if seen.insert(source_frame_id) {
            normalized.push(source_frame_id);
        }
    }

    if normalized.is_empty() {
        return Err(PipelineError::InvalidRequest {
            reason: "invalid-frame-selection",
        });
    }
    if normalized.len() > MAX_PREVIEW_BATCH_IDS {
        return Err(PipelineError::LimitExceeded {
            resource: "frame-count",
            limit: MAX_PREVIEW_BATCH_IDS as u64,
            actual: u64::try_from(normalized.len()).unwrap_or(u64::MAX),
        });
    }
    Ok(normalized)
}

fn preview_output_dimensions(width: u32, height: u32) -> Result<(u32, u32), PipelineError> {
    if width == 0 || height == 0 {
        return Err(PipelineError::InvalidRequest {
            reason: "invalid-frame-selection",
        });
    }
    let largest = width.max(height);
    if largest <= MAX_PREVIEW_EDGE {
        return Ok((width, height));
    }

    let scale = f64::from(MAX_PREVIEW_EDGE) / f64::from(largest);
    let scaled_width = (f64::from(width) * scale)
        .round()
        .clamp(1.0, f64::from(MAX_PREVIEW_EDGE)) as u32;
    let scaled_height = (f64::from(height) * scale)
        .round()
        .clamp(1.0, f64::from(MAX_PREVIEW_EDGE)) as u32;
    Ok((scaled_width, scaled_height))
}

fn ascending_video_preview_extraction(
    requested_source_frame_ids: &[u32],
) -> Result<Vec<(u32, u32)>, PipelineError> {
    let mut extraction = requested_source_frame_ids
        .iter()
        .copied()
        .map(|source_frame_id| {
            source_frame_id
                .checked_sub(1)
                .map(|frame_index| (frame_index, source_frame_id))
                .ok_or(PipelineError::InvalidRequest {
                    reason: "invalid-frame-selection",
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    extraction.sort_unstable_by_key(|(frame_index, _)| *frame_index);
    Ok(extraction)
}

fn build_native_preview_group(
    input_path: &Path,
    cache: &PreviewCache,
    key: &PreviewSourceKey,
    expected_source: &SourceIdentity,
    context: &OperationContext,
    progress: &mut impl FnMut(ProgressStage, u32, Option<u32>),
) -> Result<Vec<CachedPreview>, PipelineError> {
    let mut previews = Vec::new();
    visit_native_animation_frames(
        input_path,
        MediaLimits::default(),
        context,
        |source_frame_id, frame| {
            context.checkpoint()?;
            let pixels = frame.into_buffer();
            let (width, height) = preview_output_dimensions(pixels.width(), pixels.height())?;
            let scaled = if pixels.dimensions() == (width, height) {
                pixels
            } else {
                imageops::resize(&pixels, width, height, FilterType::Lanczos3)
            };
            context.checkpoint()?;
            let png_bytes = encode_native_png_bytes(&scaled)?;
            previews.push(CachedPreview {
                source_frame_id,
                png_bytes: Arc::from(png_bytes.into_boxed_slice()),
                width,
                height,
            });
            progress(
                ProgressStage::Decoding,
                u32::try_from(previews.len()).unwrap_or(u32::MAX),
                None,
            );
            context.checkpoint()
        },
    )?;
    finalize_preview_cache_publication(cache, key, expected_source, context, None)?;
    Ok(previews)
}

fn build_video_preview_filter(extraction: &[(u32, u32)], width: u32, height: u32) -> String {
    let selection = extraction
        .iter()
        .map(|(frame_index, _)| format!("eq(n,{frame_index})"))
        .collect::<Vec<_>>()
        .join("+");
    format!("select='{selection}',scale={width}:{height}:flags=lanczos,format=rgba,setsar=1")
}

fn extract_video_preview_batch(
    input_path: &Path,
    cache: &PreviewCache,
    key: &PreviewSourceKey,
    extraction: &[(u32, u32)],
    base_completed: u32,
    requested_total: u32,
    width: u32,
    height: u32,
    locale: UiLocale,
    expected_source: &SourceIdentity,
    context: &OperationContext,
    progress: &mut impl FnMut(ProgressStage, u32, Option<u32>),
) -> Result<Vec<CachedPreview>, PipelineError> {
    if extraction.is_empty() {
        return Ok(Vec::new());
    }
    let frame_size = checked_rgba_bytes(width, height, MediaLimits::default())?;
    let stdout_limit =
        frame_size
            .checked_mul(extraction.len())
            .ok_or(PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: MediaLimits::default().max_total_decoded_bytes,
                actual: u64::MAX,
            })?;
    let resolution = resolve_tool("ffmpeg", locale)
        .map_err(|_| PipelineError::ToolMissing { tool: "ffmpeg" })?;
    let filter = build_video_preview_filter(extraction, width, height);
    let args = vec![
        OsString::from("-v"),
        OsString::from("error"),
        OsString::from("-i"),
        input_path.as_os_str().to_os_string(),
        OsString::from("-map"),
        OsString::from("0:v:0"),
        OsString::from("-vf"),
        OsString::from(filter),
        OsString::from("-fps_mode"),
        OsString::from("passthrough"),
        OsString::from("-pix_fmt"),
        OsString::from("rgba"),
        OsString::from("-f"),
        OsString::from("rawvideo"),
        OsString::from("-an"),
        OsString::from("-"),
    ];
    let mut expected_frames = extraction.iter().copied();
    let mut previews = Vec::with_capacity(extraction.len());
    let summary = stream_fixed_rgba_frames(
        &resolution.command,
        &args,
        frame_size,
        extraction.len(),
        ProcessLimits {
            timeout: RAW_DECODE_PROCESS_TIMEOUT,
            max_stdout_bytes: stdout_limit,
            max_stderr_bytes: PROCESS_CAPTURE_LIMIT_BYTES,
        },
        context,
        |frame| {
            context.checkpoint()?;
            let (_, source_frame_id) =
                expected_frames
                    .next()
                    .ok_or(PipelineError::MalformedProcessOutput {
                        reason: "video preview stream produced an unexpected frame".into(),
                    })?;
            let pixels = rgba_frame_from_bytes(width, height, frame)?;
            let png_bytes = encode_native_png_bytes(&pixels)?;
            previews.push(CachedPreview {
                source_frame_id,
                png_bytes: Arc::from(png_bytes.into_boxed_slice()),
                width,
                height,
            });
            progress(
                ProgressStage::Encoding,
                base_completed.saturating_add(u32::try_from(previews.len()).unwrap_or(u32::MAX)),
                Some(requested_total),
            );
            context.checkpoint()
        },
    )?;
    validate_exact_selected_frame_stream(extraction.len(), summary.frame_count)?;
    finalize_preview_cache_publication(cache, key, expected_source, context, None)?;
    Ok(previews)
}

fn cached_preview_items_for_requested_ids(
    requested_source_frame_ids: &[u32],
    cached: &[CachedPreview],
) -> Result<Vec<FramePreviewItem>, PipelineError> {
    let by_id = cached
        .iter()
        .map(|preview| (preview.source_frame_id, preview))
        .collect::<BTreeMap<_, _>>();
    requested_source_frame_ids
        .iter()
        .map(|source_frame_id| {
            let preview = by_id
                .get(source_frame_id)
                .ok_or(PipelineError::InvalidRequest {
                    reason: "invalid-frame-selection",
                })?;
            Ok(FramePreviewItem {
                source_frame_id: *source_frame_id,
                data_url: format!(
                    "data:image/png;base64,{}",
                    general_purpose::STANDARD.encode(preview.png_bytes.as_ref())
                ),
                width: preview.width,
                height: preview.height,
            })
        })
        .collect()
}

fn cached_previews_for_requested_ids(
    requested_source_frame_ids: &[u32],
    cached: &[CachedPreview],
) -> Result<Vec<CachedPreview>, PipelineError> {
    let by_id = cached
        .iter()
        .map(|preview| (preview.source_frame_id, preview))
        .collect::<BTreeMap<_, _>>();
    requested_source_frame_ids
        .iter()
        .map(|source_frame_id| {
            by_id
                .get(source_frame_id)
                .cloned()
                .cloned()
                .ok_or(PipelineError::InvalidRequest {
                    reason: "invalid-frame-selection",
                })
        })
        .collect()
}

fn preview_source_key(
    input_path: &Path,
    identity: &SourceIdentity,
    source_revision: &str,
    source_width: Option<u32>,
    source_height: Option<u32>,
) -> Result<PreviewSourceKey, PipelineError> {
    let extension = input_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or(PipelineError::InvalidRequest {
            reason: "unsupported-source-format",
        })?;
    let variant = if matches!(extension.as_str(), "gif" | "apng" | "png") {
        PreviewVariant::Native
    } else if is_supported_video_extension(&extension) {
        let source_width = source_width.ok_or(PipelineError::InvalidRequest {
            reason: "invalid-frame-selection",
        })?;
        let source_height = source_height.ok_or(PipelineError::InvalidRequest {
            reason: "invalid-frame-selection",
        })?;
        let limits = MediaLimits::default();
        if source_width > limits.max_dimension || source_height > limits.max_dimension {
            return Err(PipelineError::LimitExceeded {
                resource: "image-dimensions",
                limit: u64::from(limits.max_dimension),
                actual: u64::from(source_width.max(source_height)),
            });
        }
        let (width, height) = preview_output_dimensions(source_width, source_height)?;
        PreviewVariant::Video { width, height }
    } else {
        return Err(PipelineError::InvalidRequest {
            reason: "unsupported-frame-preview",
        });
    };
    Ok(PreviewSourceKey {
        identity: identity.clone(),
        source_revision: source_revision.to_owned(),
        variant,
    })
}

fn finalize_preview_cache_publication(
    cache: &PreviewCache,
    key: &PreviewSourceKey,
    expected_source: &SourceIdentity,
    context: &OperationContext,
    publication: Option<&PreviewCachePublication>,
) -> Result<(), PipelineError> {
    finalize_preview_cache_publication_with_check(cache, key, context, publication, || {
        ensure_source_unchanged(expected_source, MediaLimits::default())
    })
}

fn finalize_preview_cache_publication_with_check(
    cache: &PreviewCache,
    key: &PreviewSourceKey,
    context: &OperationContext,
    publication: Option<&PreviewCachePublication>,
    source_check: impl FnOnce() -> Result<(), PipelineError>,
) -> Result<(), PipelineError> {
    context.checkpoint()?;
    let source_check = source_check();
    if matches!(&source_check, Err(PipelineError::SourceChanged)) {
        cache.invalidate(key);
    }
    if let Err(error) = context.checkpoint() {
        if source_check.is_ok() {
            if let Some(publication) = publication {
                cache.invalidate_publication(key, publication);
            }
        }
        return Err(error);
    }
    source_check
}

fn finalize_preview_source_publication<T>(
    cache: &PreviewCache,
    key: &PreviewSourceKey,
    expected_source: &SourceIdentity,
    context: &OperationContext,
    publication: Option<&PreviewCachePublication>,
    publisher: impl FnOnce() -> Result<T, PipelineError>,
) -> Result<T, PipelineError> {
    finalize_preview_cache_publication(cache, key, expected_source, context, publication)?;
    context.finalize(publisher)
}

fn checkpoint_preview_cache_publication(
    cache: &PreviewCache,
    key: &PreviewSourceKey,
    context: &OperationContext,
    publication: Option<&PreviewCachePublication>,
) -> Result<(), PipelineError> {
    if let Err(error) = context.checkpoint() {
        if let Some(publication) = publication {
            cache.invalidate_publication(key, publication);
        }
        return Err(error);
    }
    Ok(())
}

fn rollback_preview_finalization_error(
    cache: &PreviewCache,
    key: &PreviewSourceKey,
    publication: Option<&PreviewCachePublication>,
    error: &PipelineError,
) {
    if matches!(error, PipelineError::SourceChanged) {
        cache.invalidate(key);
    } else if let Some(publication) = publication {
        cache.invalidate_publication(key, publication);
    }
}

fn settle_preview_load_result<T>(
    cache: &PreviewCache,
    key: &PreviewSourceKey,
    context: &OperationContext,
    result: Result<T, PipelineError>,
) -> Result<T, PipelineError> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            if matches!(&error, PipelineError::SourceChanged) {
                cache.invalidate(key);
            }
            context.checkpoint()?;
            Err(error)
        }
    }
}

struct LoadedCachedFramePreviews {
    previews: Vec<CachedPreview>,
    publication: Option<PreviewCachePublication>,
}

fn load_cached_frame_previews(
    cache: &PreviewCache,
    key: &PreviewSourceKey,
    input_path: &Path,
    requested_source_frame_ids: &[u32],
    locale: UiLocale,
    expected_source: &SourceIdentity,
    context: &OperationContext,
    progress: &mut impl FnMut(ProgressStage, u32, Option<u32>),
) -> Result<LoadedCachedFramePreviews, PipelineError> {
    context.checkpoint()?;
    let requested = normalize_preview_source_frame_ids(requested_source_frame_ids)?;
    let requested_total = u32::try_from(requested.len()).unwrap_or(u32::MAX);
    let load_result = (|| -> Result<_, PipelineError> {
        Ok(match &key.variant {
            PreviewVariant::Native => {
                let lookup = cache.get_or_try_build_complete(key, &requested, || {
                    build_native_preview_group(
                        input_path,
                        cache,
                        key,
                        expected_source,
                        context,
                        progress,
                    )
                })?;
                (lookup.previews, lookup.publication)
            }
            PreviewVariant::Video { width, height } => {
                let mut available = Vec::new();
                let mut missing = Vec::new();
                let mut publication = None;
                for source_frame_id in requested.iter().copied() {
                    match cache.get_requested(key, &[source_frame_id]) {
                        Some(mut cached) => available.append(&mut cached),
                        None => missing.push(source_frame_id),
                    }
                }
                if !missing.is_empty() {
                    let extraction = ascending_video_preview_extraction(&missing)?;
                    let base_completed = u32::try_from(available.len()).unwrap_or(u32::MAX);
                    let extracted = extract_video_preview_batch(
                        input_path,
                        cache,
                        key,
                        &extraction,
                        base_completed,
                        requested_total,
                        *width,
                        *height,
                        locale,
                        expected_source,
                        context,
                        progress,
                    )?;
                    publication = cache.merge_partial(key.clone(), extracted.clone());
                    available.extend(extracted);
                }
                (
                    cached_previews_for_requested_ids(&requested, &available)?,
                    publication,
                )
            }
        })
    })();
    let (previews, publication) = settle_preview_load_result(cache, key, context, load_result)?;
    checkpoint_preview_cache_publication(cache, key, context, publication.as_ref())?;
    finalize_preview_cache_publication(cache, key, expected_source, context, publication.as_ref())?;
    Ok(LoadedCachedFramePreviews {
        previews,
        publication,
    })
}

fn frame_preview_response_from_cached(
    requested_source_frame_id: u32,
    cached: &[CachedPreview],
) -> Result<FramePreviewResponse, PipelineError> {
    let mut items = cached_preview_items_for_requested_ids(&[requested_source_frame_id], cached)?;
    let item = items.pop().ok_or(PipelineError::InvalidRequest {
        reason: "invalid-frame-selection",
    })?;
    Ok(FramePreviewResponse {
        ok: true,
        data_url: Some(item.data_url),
        width: Some(item.width),
        height: Some(item.height),
        reason_code: None,
        error_code: None,
        error_message: None,
    })
}

fn extract_frame_preview_internal(
    input_path: &str,
    source_frame_id: u32,
    locale: UiLocale,
) -> FramePreviewResponse {
    extract_frame_preview_with_callbacks(
        input_path,
        source_frame_id,
        locale,
        || Ok(()),
        |_, _, _| {},
    )
}

fn extract_frame_preview_with_operation(
    preflight: &SourcePreflight,
    source_frame_id: u32,
    locale: UiLocale,
    key: &PreviewSourceKey,
    cache: &PreviewCache,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> FramePreviewResponse {
    let mut publish = |stage, completed, total| {
        publish_progress(progress, context, stage, completed, total);
    };
    let decoding_total = match &key.variant {
        PreviewVariant::Native => None,
        PreviewVariant::Video { .. } => Some(1),
    };
    publish(ProgressStage::Decoding, 0, decoding_total);
    let loaded = match load_cached_frame_previews(
        cache,
        key,
        preflight.canonical_path(),
        &[source_frame_id],
        locale,
        preflight.identity(),
        context,
        &mut publish,
    ) {
        Ok(loaded) => loaded,
        Err(error) => return frame_preview_pipeline_error(&error, locale),
    };
    let response = match frame_preview_response_from_cached(source_frame_id, &loaded.previews) {
        Ok(response) => response,
        Err(error) => frame_preview_pipeline_error(&error, locale),
    };
    let publication = loaded.publication;
    match finalize_preview_source_publication(
        cache,
        key,
        preflight.identity(),
        context,
        publication.as_ref(),
        || {
            publish(ProgressStage::Finalizing, 1, Some(1));
            Ok(response)
        },
    ) {
        Ok(response) => response,
        Err(error) => {
            rollback_preview_finalization_error(cache, key, publication.as_ref(), &error);
            frame_preview_pipeline_error(&error, locale)
        }
    }
}

fn extract_frame_preview_with_callbacks(
    input_path: &str,
    source_frame_id: u32,
    locale: UiLocale,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
    mut progress: impl FnMut(ProgressStage, u32, Option<u32>),
) -> FramePreviewResponse {
    if let Err(error) = normalize_preview_source_frame_ids(&[source_frame_id]) {
        return frame_preview_pipeline_error(&error, locale);
    }
    let identity = match SourceIdentity::from_path(Path::new(input_path), MediaLimits::default()) {
        Ok(identity) => identity,
        Err(error) => return frame_preview_pipeline_error(&error, locale),
    };
    let revision = identity.revision();
    let key = match preview_source_key(Path::new(input_path), &identity, &revision, None, None) {
        Ok(key) => key,
        Err(error) => return frame_preview_pipeline_error(&error, locale),
    };
    let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
    let context = OperationContext::detached(MediaOperationKind::Preview.timeout());
    if let Err(error) = checkpoint() {
        return frame_preview_pipeline_error(&error, locale);
    }
    let loaded = load_cached_frame_previews(
        &cache,
        &key,
        Path::new(input_path),
        &[source_frame_id],
        locale,
        &identity,
        &context,
        &mut progress,
    );
    let (result, publication) = match loaded {
        Ok(loaded) => (
            frame_preview_response_from_cached(source_frame_id, &loaded.previews),
            loaded.publication,
        ),
        Err(error) => (Err(error), None),
    };
    if let Err(error) = checkpoint() {
        if let Some(publication) = publication.as_ref() {
            cache.invalidate_publication(&key, publication);
        }
        return frame_preview_pipeline_error(&error, locale);
    }
    match result {
        Ok(response) => response,
        Err(error) => frame_preview_pipeline_error(&error, locale),
    }
}

fn frame_preview_pipeline_error(error: &PipelineError, locale: UiLocale) -> FramePreviewResponse {
    FramePreviewResponse {
        ok: false,
        data_url: None,
        width: None,
        height: None,
        reason_code: error.media_reason_code(),
        error_code: Some(error.error_code()),
        error_message: Some(pipeline_error_diagnostic(error, locale)),
    }
}

fn frame_previews_pipeline_error(error: &PipelineError, locale: UiLocale) -> FramePreviewsResponse {
    FramePreviewsResponse {
        ok: false,
        previews: Vec::new(),
        reason_code: error.media_reason_code(),
        error_code: Some(error.error_code()),
        error_message: Some(pipeline_error_diagnostic(error, locale)),
    }
}

fn extract_frame_previews_internal(
    input_path: &str,
    source_frame_ids: &[u32],
    locale: UiLocale,
) -> FramePreviewsResponse {
    extract_frame_previews_with_callbacks(
        input_path,
        source_frame_ids,
        locale,
        || Ok(()),
        |_, _, _| {},
    )
}

fn extract_frame_previews_with_operation(
    preflight: &SourcePreflight,
    source_frame_ids: &[u32],
    locale: UiLocale,
    key: &PreviewSourceKey,
    cache: &PreviewCache,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> FramePreviewsResponse {
    let mut publish = |stage, completed, total| {
        publish_progress(progress, context, stage, completed, total);
    };
    let requested = match normalize_preview_source_frame_ids(source_frame_ids) {
        Ok(requested) => requested,
        Err(error) => return frame_previews_pipeline_error(&error, locale),
    };
    let total = u32::try_from(requested.len()).unwrap_or(u32::MAX);
    let decoding_total = match &key.variant {
        PreviewVariant::Native => None,
        PreviewVariant::Video { .. } => Some(total),
    };
    publish(ProgressStage::Decoding, 0, decoding_total);
    let loaded = match load_cached_frame_previews(
        cache,
        key,
        preflight.canonical_path(),
        &requested,
        locale,
        preflight.identity(),
        context,
        &mut publish,
    ) {
        Ok(loaded) => loaded,
        Err(error) => return frame_previews_pipeline_error(&error, locale),
    };
    let response = match cached_preview_items_for_requested_ids(&requested, &loaded.previews) {
        Ok(previews) => FramePreviewsResponse {
            ok: true,
            previews,
            reason_code: None,
            error_code: None,
            error_message: None,
        },
        Err(error) => frame_previews_pipeline_error(&error, locale),
    };
    let publication = loaded.publication;
    match finalize_preview_source_publication(
        cache,
        key,
        preflight.identity(),
        context,
        publication.as_ref(),
        || {
            publish(ProgressStage::Finalizing, total, Some(total));
            Ok(response)
        },
    ) {
        Ok(response) => response,
        Err(error) => {
            rollback_preview_finalization_error(cache, key, publication.as_ref(), &error);
            frame_previews_pipeline_error(&error, locale)
        }
    }
}

fn extract_frame_previews_with_callbacks(
    input_path: &str,
    source_frame_ids: &[u32],
    locale: UiLocale,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
    mut progress: impl FnMut(ProgressStage, u32, Option<u32>),
) -> FramePreviewsResponse {
    let requested = match normalize_preview_source_frame_ids(source_frame_ids) {
        Ok(requested) => requested,
        Err(error) => return frame_previews_pipeline_error(&error, locale),
    };
    let identity = match SourceIdentity::from_path(Path::new(input_path), MediaLimits::default()) {
        Ok(identity) => identity,
        Err(error) => return frame_previews_pipeline_error(&error, locale),
    };
    let revision = identity.revision();
    let key = match preview_source_key(Path::new(input_path), &identity, &revision, None, None) {
        Ok(key) => key,
        Err(error) => return frame_previews_pipeline_error(&error, locale),
    };
    let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
    let context = OperationContext::detached(MediaOperationKind::Preview.timeout());
    if let Err(error) = checkpoint() {
        return frame_previews_pipeline_error(&error, locale);
    }
    let loaded = load_cached_frame_previews(
        &cache,
        &key,
        Path::new(input_path),
        &requested,
        locale,
        &identity,
        &context,
        &mut progress,
    );
    let (result, publication) = match loaded {
        Ok(loaded) => (
            cached_preview_items_for_requested_ids(&requested, &loaded.previews),
            loaded.publication,
        ),
        Err(error) => (Err(error), None),
    };
    if let Err(error) = checkpoint() {
        if let Some(publication) = publication.as_ref() {
            cache.invalidate_publication(&key, publication);
        }
        return frame_previews_pipeline_error(&error, locale);
    }
    match result {
        Ok(previews) => FramePreviewsResponse {
            ok: true,
            previews,
            reason_code: None,
            error_code: None,
            error_message: None,
        },
        Err(error) => frame_previews_pipeline_error(&error, locale),
    }
}

fn full_frame_region(pixels: &RgbaImage) -> FrameRegion {
    FrameRegion {
        x: 0,
        y: 0,
        width: pixels.width(),
        height: pixels.height(),
    }
}

fn changed_frame_region(previous: &RgbaImage, current: &RgbaImage) -> FrameRegion {
    if previous.dimensions() != current.dimensions() {
        return full_frame_region(current);
    }

    let mut min_x = current.width();
    let mut min_y = current.height();
    let mut max_x = 0;
    let mut max_y = 0;
    let mut changed = false;

    for y in 0..current.height() {
        for x in 0..current.width() {
            if previous.get_pixel(x, y) != current.get_pixel(x, y) {
                changed = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    if changed {
        FrameRegion {
            x: min_x,
            y: min_y,
            width: max_x - min_x + 1,
            height: max_y - min_y + 1,
        }
    } else {
        FrameRegion {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }
    }
}

fn frame_region_pixels(pixels: &RgbaImage, region: FrameRegion) -> Vec<u8> {
    let mut data = Vec::with_capacity((region.width * region.height * 4) as usize);
    for y in region.y..region.y + region.height {
        for x in region.x..region.x + region.width {
            data.extend_from_slice(&pixels.get_pixel(x, y).0);
        }
    }

    data
}

fn pipeline_io_error(operation: &'static str, error: impl std::fmt::Display) -> PipelineError {
    PipelineError::Io {
        operation,
        message: error.to_string(),
    }
}

fn pending_output_size(output: &mut PendingOutput) -> Result<u64, PipelineError> {
    output
        .writer()
        .flush()
        .map_err(|error| pipeline_io_error("flush temporary output", error))?;
    output
        .writer()
        .metadata()
        .map(|metadata| metadata.len())
        .map_err(|error| pipeline_io_error("read temporary output metadata", error))
}

fn commit_output_after_source_validation(
    output: PendingOutput,
    expected_source: Option<&SourceIdentity>,
) -> Result<PathBuf, PipelineError> {
    if let Some(expected_source) = expected_source {
        ensure_source_unchanged(expected_source, MediaLimits::default())?;
    }
    output.commit()
}

fn publish_optimizer_selection(
    expected_source: &SourceIdentity,
    source_already_validated: bool,
    best_within_limit: Option<PendingSelectedEncodeOutput>,
    smallest_oversize: Option<PendingSelectedEncodeOutput>,
    attempts: &mut [SearchAttemptResult],
) -> Result<Option<PublishedSelectedEncodeOutput>, PipelineError> {
    let selected = match best_within_limit {
        Some(best) => {
            drop(smallest_oversize);
            best
        }
        None => match smallest_oversize {
            Some(oversize) => oversize,
            None => return Ok(None),
        },
    };
    let PendingSelectedEncodeOutput {
        selected,
        pending_output,
    } = selected;
    let output_path = if source_already_validated {
        pending_output.commit()?
    } else {
        commit_output_after_source_validation(pending_output, Some(expected_source))?
    }
    .to_string_lossy()
    .into_owned();
    if let Some(attempt) = attempts
        .iter_mut()
        .find(|attempt| attempt.candidate_id == selected.candidate_id)
    {
        attempt.output_path = Some(output_path.clone());
    }

    Ok(Some(PublishedSelectedEncodeOutput {
        selected,
        output_path,
    }))
}

pub(crate) struct NativeApngWriteSession<W: Write> {
    writer: png::Writer<W>,
    frame_delays: Vec<(u16, u16)>,
    width: u32,
    height: u32,
    declared_frame_count: usize,
    written_frame_count: usize,
    last_output_position: usize,
    validate_complete: bool,
}

impl<W: Write> NativeApngWriteSession<W> {
    pub(crate) fn begin(
        writer: W,
        width: u32,
        height: u32,
        declared_frame_count: usize,
        durations_us: &[u64],
        preset: &str,
        validate_complete: bool,
    ) -> Result<Self, PipelineError> {
        if declared_frame_count == 0 {
            return Err(PipelineError::InvalidRequest {
                reason: "no-frames-selected",
            });
        }
        if durations_us.len() != declared_frame_count {
            return Err(PipelineError::MalformedProcessOutput {
                reason: "APNG duration count did not match frame count".into(),
            });
        }
        let frame_count_u32 =
            u32::try_from(declared_frame_count).map_err(|_| PipelineError::LimitExceeded {
                resource: "frame-count",
                limit: u64::from(u32::MAX),
                actual: u64::try_from(declared_frame_count).unwrap_or(u64::MAX),
            })?;
        let frame_delays = quantize_apng_delays(durations_us)
            .map_err(|reason| PipelineError::InvalidRequest { reason })?;

        let mut encoder = NativePngEncoder::new(writer, width, height);
        encoder.set_color(PngColorType::Rgba);
        encoder.set_depth(PngBitDepth::Eight);
        encoder
            .set_animated(frame_count_u32, 0)
            .map_err(|error| pipeline_io_error("configure APNG output", error))?;
        encoder
            .set_sep_def_img(false)
            .map_err(|error| pipeline_io_error("configure APNG output", error))?;
        encoder.set_deflate_compression(native_png_deflate_for_preset(preset));
        encoder.set_filter(native_png_filter_for_preset(preset));
        encoder
            .set_blend_op(PngBlendOp::Source)
            .map_err(|error| pipeline_io_error("configure APNG output", error))?;
        encoder
            .set_dispose_op(PngDisposeOp::None)
            .map_err(|error| pipeline_io_error("configure APNG output", error))?;
        let (delay_num, delay_den) = frame_delays[0];
        encoder
            .set_frame_delay(delay_num, delay_den)
            .map_err(|error| pipeline_io_error("configure APNG output", error))?;
        encoder.validate_sequence(validate_complete);
        let writer = encoder
            .write_header()
            .map_err(|error| pipeline_io_error("write APNG header", error))?;

        Ok(Self {
            writer,
            frame_delays,
            width,
            height,
            declared_frame_count,
            written_frame_count: 0,
            last_output_position: 0,
            validate_complete,
        })
    }

    pub(crate) fn write_first(&mut self, pixels: &RgbaImage) -> Result<(), PipelineError> {
        if self.written_frame_count != 0 || pixels.dimensions() != (self.width, self.height) {
            return Err(PipelineError::InvalidRequest {
                reason: "invalid-frame-selection",
            });
        }
        self.writer
            .write_image_data(pixels.as_raw())
            .map_err(|error| pipeline_io_error("write APNG frame", error))?;
        self.written_frame_count = 1;
        Ok(())
    }

    pub(crate) fn write_transition(
        &mut self,
        output_position: usize,
        previous: &RgbaImage,
        current: &RgbaImage,
    ) -> Result<(), PipelineError> {
        if self.written_frame_count == 0
            || output_position == 0
            || output_position >= self.declared_frame_count
            || output_position <= self.last_output_position
            || previous.dimensions() != (self.width, self.height)
            || current.dimensions() != (self.width, self.height)
        {
            return Err(PipelineError::InvalidRequest {
                reason: "invalid-frame-selection",
            });
        }
        let (delay_num, delay_den) = self.frame_delays[output_position];
        let region = changed_frame_region(previous, current);
        self.writer
            .reset_frame_position()
            .map_err(|error| pipeline_io_error("configure APNG frame", error))?;
        self.writer
            .set_frame_dimension(region.width, region.height)
            .map_err(|error| pipeline_io_error("configure APNG frame", error))?;
        self.writer
            .set_frame_position(region.x, region.y)
            .map_err(|error| pipeline_io_error("configure APNG frame", error))?;
        self.writer
            .set_frame_delay(delay_num, delay_den)
            .map_err(|error| pipeline_io_error("configure APNG frame", error))?;
        self.writer
            .set_blend_op(PngBlendOp::Source)
            .map_err(|error| pipeline_io_error("configure APNG frame", error))?;
        self.writer
            .set_dispose_op(PngDisposeOp::None)
            .map_err(|error| pipeline_io_error("configure APNG frame", error))?;
        let region_pixels = frame_region_pixels(current, region);
        self.writer
            .write_image_data(&region_pixels)
            .map_err(|error| pipeline_io_error("write APNG frame", error))?;
        self.written_frame_count += 1;
        self.last_output_position = output_position;
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<(), PipelineError> {
        if self.written_frame_count == 0 {
            return Err(PipelineError::MalformedProcessOutput {
                reason: "APNG frame iterator ended before the first frame".into(),
            });
        }
        if self.validate_complete && self.written_frame_count != self.declared_frame_count {
            return Err(PipelineError::MalformedProcessOutput {
                reason: "APNG frame iterator ended before the declared frame count".into(),
            });
        }
        self.writer
            .finish()
            .map_err(|error| pipeline_io_error("finish APNG output", error))
    }
}

fn write_native_apng<W: Write>(
    writer: W,
    frames: &[StickerFrame],
    preset: &str,
) -> Result<(), PipelineError> {
    write_native_apng_with_checkpoint(writer, frames, preset, || Ok(()))
}

fn write_native_apng_with_checkpoint<W: Write>(
    writer: W,
    frames: &[StickerFrame],
    preset: &str,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<(), PipelineError> {
    let durations_us = frames
        .iter()
        .map(|frame| frame.duration_us)
        .collect::<Vec<_>>();
    write_native_apng_iterator_with_checkpoint(
        writer,
        frames.len(),
        &durations_us,
        frames.iter().cloned().map(Ok),
        preset,
        || checkpoint(),
    )
}

fn write_native_apng_iterator_with_checkpoint<W, I>(
    writer: W,
    frame_count: usize,
    durations_us: &[u64],
    mut frames: I,
    preset: &str,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<(), PipelineError>
where
    W: Write,
    I: Iterator<Item = Result<StickerFrame, PipelineError>>,
{
    checkpoint()?;
    if frame_count == 0 {
        return Err(PipelineError::InvalidRequest {
            reason: "no-frames-selected",
        });
    }
    if durations_us.len() != frame_count {
        return Err(PipelineError::MalformedProcessOutput {
            reason: "APNG duration count did not match frame count".into(),
        });
    }
    let first = frames
        .next()
        .ok_or_else(|| PipelineError::MalformedProcessOutput {
            reason: "APNG frame iterator ended before the declared frame count".into(),
        })??;
    if first.duration_us != durations_us[0] {
        return Err(PipelineError::MalformedProcessOutput {
            reason: "APNG frame duration did not match sequence metadata".into(),
        });
    }

    let width = first.pixels.width();
    let height = first.pixels.height();
    let mut session = NativeApngWriteSession::begin(
        writer,
        width,
        height,
        frame_count,
        durations_us,
        preset,
        true,
    )?;
    checkpoint()?;
    session.write_first(&first.pixels)?;
    checkpoint()?;

    let mut previous_frame = first.pixels;
    for frame_index in 1..frame_count {
        checkpoint()?;
        let frame = frames
            .next()
            .ok_or_else(|| PipelineError::MalformedProcessOutput {
                reason: "APNG frame iterator ended before the declared frame count".into(),
            })??;
        if frame.duration_us != durations_us[frame_index] {
            return Err(PipelineError::MalformedProcessOutput {
                reason: "APNG frame duration did not match sequence metadata".into(),
            });
        }
        if frame.pixels.width() != width || frame.pixels.height() != height {
            return Err(PipelineError::InvalidRequest {
                reason: "invalid-frame-selection",
            });
        }
        checkpoint()?;
        session.write_transition(frame_index, &previous_frame, &frame.pixels)?;
        checkpoint()?;
        previous_frame = frame.pixels;
    }

    checkpoint()?;
    match frames.next() {
        None => {}
        Some(Ok(_)) => {
            return Err(PipelineError::MalformedProcessOutput {
                reason: "APNG frame iterator exceeded the declared frame count".into(),
            })
        }
        Some(Err(error)) => return Err(error),
    }

    checkpoint()?;
    session.finish()
}

fn validate_raw_rgba_output(
    width: u32,
    height: u32,
    output_len: usize,
    expected_frame_count: Option<usize>,
    limits: MediaLimits,
) -> Result<(usize, usize), PipelineError> {
    let frame_size = checked_rgba_bytes(width, height, limits)?;
    if frame_size == 0 || output_len == 0 || output_len % frame_size != 0 {
        return Err(PipelineError::MalformedProcessOutput {
            reason: "raw RGBA output is not a whole frame sequence".into(),
        });
    }
    let frame_count = output_len / frame_size;
    if frame_count > limits.max_frame_count as usize {
        return Err(PipelineError::LimitExceeded {
            resource: "frame-count",
            limit: u64::from(limits.max_frame_count),
            actual: u64::try_from(frame_count).unwrap_or(u64::MAX),
        });
    }
    let total_bytes = u64::try_from(output_len).unwrap_or(u64::MAX);
    if total_bytes > limits.max_total_decoded_bytes {
        return Err(PipelineError::LimitExceeded {
            resource: "decoded-bytes",
            limit: limits.max_total_decoded_bytes,
            actual: total_bytes,
        });
    }
    if expected_frame_count.is_some_and(|expected| expected != frame_count) {
        return Err(PipelineError::MalformedProcessOutput {
            reason: "raw RGBA output frame count did not match the request".into(),
        });
    }
    Ok((frame_size, frame_count))
}

fn rgba_frame_from_bytes(
    width: u32,
    height: u32,
    bytes: Vec<u8>,
) -> Result<RgbaImage, PipelineError> {
    RgbaImage::from_raw(width, height, bytes).ok_or_else(|| PipelineError::MalformedProcessOutput {
        reason: "raw RGBA frame size did not match image dimensions".into(),
    })
}

fn extract_video_source_frames_rgba(
    input_path: &str,
    frame_indexes: &BTreeSet<u32>,
    frame_width: u32,
    frame_height: u32,
    locale: UiLocale,
    context: Option<&OperationContext>,
) -> Result<(BTreeMap<u32, RgbaImage>, ToolResolution), PipelineError> {
    if frame_indexes.is_empty() {
        return Err(PipelineError::InvalidRequest {
            reason: "no-frames-selected",
        });
    }
    let limits = MediaLimits::default();
    let frame_size = checked_rgba_bytes(frame_width, frame_height, limits)?;
    preflight_animation_decoded_bytes(
        frame_size,
        u64::try_from(frame_indexes.len()).unwrap_or(u64::MAX),
        limits,
    )?;
    let stdout_limit =
        frame_size
            .checked_mul(frame_indexes.len())
            .ok_or(PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: limits.max_total_decoded_bytes,
                actual: u64::MAX,
            })?;

    let select_filter = build_source_frame_select_filter(frame_indexes);
    let args = [
        OsString::from("-v"),
        OsString::from("error"),
        OsString::from("-i"),
        OsString::from(input_path),
        OsString::from("-vf"),
        OsString::from(select_filter),
        OsString::from("-pix_fmt"),
        OsString::from("rgba"),
        OsString::from("-f"),
        OsString::from("rawvideo"),
        OsString::from("-an"),
        OsString::from("-"),
    ];
    let resolution = resolve_tool("ffmpeg", locale)
        .map_err(|_| PipelineError::ToolMissing { tool: "ffmpeg" })?;
    let detached = context
        .is_none()
        .then(|| OperationContext::detached(RAW_DECODE_PROCESS_TIMEOUT));
    let context = context.unwrap_or_else(|| detached.as_ref().expect("detached decode context"));
    let mut indexes = frame_indexes.iter().copied();
    let mut frames = BTreeMap::new();
    let summary = stream_fixed_rgba_frames(
        &resolution.command,
        &args,
        frame_size,
        frame_indexes.len(),
        ProcessLimits {
            timeout: RAW_DECODE_PROCESS_TIMEOUT,
            max_stdout_bytes: stdout_limit,
            max_stderr_bytes: PROCESS_CAPTURE_LIMIT_BYTES,
        },
        context,
        |frame| {
            let frame_index = indexes.next().ok_or(PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: u64::try_from(stdout_limit).unwrap_or(u64::MAX),
                actual: u64::try_from(stdout_limit)
                    .unwrap_or(u64::MAX)
                    .saturating_add(u64::try_from(frame_size).unwrap_or(u64::MAX)),
            })?;
            let pixels = rgba_frame_from_bytes(frame_width, frame_height, frame)?;
            frames.insert(frame_index, pixels);
            Ok(())
        },
    )?;
    validate_exact_selected_frame_stream(frame_indexes.len(), summary.frame_count)?;

    Ok((frames, resolution))
}

fn validate_resampled_frame_stream(frame_count: usize) -> Result<(), PipelineError> {
    if frame_count == 0 {
        return Err(PipelineError::MalformedProcessOutput {
            reason: "raw RGBA output did not contain any frames".into(),
        });
    }
    Ok(())
}

fn validate_exact_selected_frame_stream(
    expected_frame_count: usize,
    actual_frame_count: usize,
) -> Result<(), PipelineError> {
    if expected_frame_count != actual_frame_count {
        return Err(PipelineError::MalformedProcessOutput {
            reason: "raw RGBA output frame count did not match the request".into(),
        });
    }
    Ok(())
}

fn normalized_fit_mode(raw: Option<&str>, locale: UiLocale) -> (&'static str, Option<String>) {
    match raw {
        None | Some(CANONICAL_FIT_MODE) => (CANONICAL_FIT_MODE, None),
        Some(_) => (
            CANONICAL_FIT_MODE,
            Some(locale::legacy_fit_mode_fallback_warning(locale)),
        ),
    }
}

fn build_candidate_ladder(
    selected_frame_count: usize,
    source_fps: f64,
    input_width: Option<u32>,
    input_height: Option<u32>,
    preset_strategy: &str,
    optimizer_goal: &str,
    search_budget: usize,
    locale: UiLocale,
) -> Vec<CandidatePreview> {
    let source_duration_seconds =
        natural_selection_duration_seconds(selected_frame_count, source_fps);
    build_candidate_ladder_with_checkpoint(
        selected_frame_count,
        source_fps,
        source_duration_seconds,
        input_width,
        input_height,
        preset_strategy,
        optimizer_goal,
        search_budget,
        locale,
        &mut || Ok(()),
    )
    .unwrap_or_default()
}

fn build_candidate_universe_with_checkpoint(
    selected_frame_count: usize,
    source_fps: f64,
    source_duration_seconds: f64,
    input_width: Option<u32>,
    input_height: Option<u32>,
    preset_strategy: &str,
    optimizer_goal: &str,
    locale: UiLocale,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> Result<Vec<CandidatePreview>, PipelineError> {
    checkpoint()?;
    let largest_input = input_width.unwrap_or(320).max(input_height.unwrap_or(320));
    let scale_ladder: Vec<f64> = match optimizer_goal {
        "motion" => vec![1.0, 0.92, 0.84, 0.76, 0.68, 0.60, 0.52, 0.44, 0.36],
        "quality" if largest_input <= 320 => vec![1.0, 0.96, 0.92, 0.88, 0.84, 0.80, 0.76],
        "quality" => vec![1.0, 0.96, 0.92, 0.88, 0.84, 0.80, 0.76, 0.68],
        _ => vec![1.0, 0.92, 0.84, 0.76, 0.68, 0.60, 0.52, 0.44],
    };

    let mut candidates = Vec::new();
    let frame_sample_steps = frame_sample_steps_for_goal(selected_frame_count, optimizer_goal);

    for frame_sample_step in frame_sample_steps {
        checkpoint()?;
        let encoded_frame_count = sampled_frame_count(selected_frame_count, frame_sample_step);
        let fps_ladder: Vec<u32> = vec![30, 27, 24, 21, 18, 15, 12, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1];

        for fps in fps_ladder {
            checkpoint()?;
            let duration_seconds = source_duration_seconds;
            let preset_ladder = preset_ladder_for_strategy(duration_seconds, preset_strategy);

            for scale in &scale_ladder {
                for preset in &preset_ladder {
                    checkpoint()?;
                    let summary =
                        locale::candidate_summary(locale, fps, *scale, preset, duration_seconds);
                    let frame_retention_score =
                        encoded_frame_count as f64 / selected_frame_count.max(1) as f64;
                    let score = optimizer_goal_score(
                        optimizer_goal,
                        source_fps,
                        fps,
                        *scale,
                        preset,
                        source_duration_seconds,
                        duration_seconds,
                        frame_retention_score,
                    );
                    let sample_suffix = if frame_sample_step > 1 {
                        format!("-every{frame_sample_step}")
                    } else {
                        String::new()
                    };

                    candidates.push(CandidatePreview {
                        id: format!(
                            "{}-{}-{}fps-{}scale{}-{}ms",
                            CANONICAL_FIT_MODE,
                            preset,
                            fps,
                            (scale * 100.0).round() as u32,
                            sample_suffix,
                            (duration_seconds * 1000.0).round() as u64
                        ),
                        rank: 0,
                        duration_seconds,
                        fps,
                        content_scale: *scale,
                        preset: (*preset).into(),
                        fit_mode: CANONICAL_FIT_MODE.into(),
                        score,
                        source_similarity_score: score,
                        relative_size_factor: relative_size_factor_for(
                            frame_sample_step,
                            *scale,
                            preset,
                        ),
                        summary,
                        frame_sample_step,
                    });
                }
            }
        }
    }

    checkpoint()?;
    Ok(candidates)
}

fn build_candidate_ladder_with_checkpoint(
    selected_frame_count: usize,
    source_fps: f64,
    source_duration_seconds: f64,
    input_width: Option<u32>,
    input_height: Option<u32>,
    preset_strategy: &str,
    optimizer_goal: &str,
    search_budget: usize,
    locale: UiLocale,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> Result<Vec<CandidatePreview>, PipelineError> {
    let candidates = build_candidate_universe_with_checkpoint(
        selected_frame_count,
        source_fps,
        source_duration_seconds,
        input_width,
        input_height,
        preset_strategy,
        optimizer_goal,
        locale,
        checkpoint,
    )?;
    Ok(select_ranked_candidate_subset(candidates, search_budget))
}

fn synchronize_candidates_with_prepared_duration(
    plan: &mut OptimizerPlanResponse,
    prepared: &PreparedSearchSource,
    input_width: Option<u32>,
    input_height: Option<u32>,
    preset_strategy: &str,
    optimizer_goal: &str,
    locale: UiLocale,
    context: &OperationContext,
) -> Result<(), PipelineError> {
    let duration_us = checked_timeline_duration(&prepared.base_sequence)?;
    let duration_seconds = duration_us_to_seconds(duration_us);
    plan.selected_duration_seconds = Some(duration_seconds);
    if duration_us > DISCORD_MAX_DURATION_US {
        return Err(PipelineError::InvalidRequest {
            reason: "duration-too-long",
        });
    }
    let base_frame_count = prepared.base_sequence.len().max(1);
    plan.candidates = match prepared.timing_authority {
        TimelineTimingAuthority::Authored => build_candidate_universe_fixed_duration(
            duration_seconds,
            prepared.base_fps.min(30),
            input_width,
            input_height,
            preset_strategy,
            optimizer_goal,
            locale,
        ),
        TimelineTimingAuthority::Native | TimelineTimingAuthority::Inspected => {
            build_candidate_universe_with_checkpoint(
                base_frame_count,
                f64::from(prepared.base_fps),
                duration_seconds,
                input_width,
                input_height,
                preset_strategy,
                optimizer_goal,
                locale,
                &mut || context.checkpoint(),
            )?
        }
    };
    let mut synchronized = Vec::with_capacity(plan.candidates.len());

    for mut candidate in plan.candidates.drain(..) {
        context.checkpoint()?;
        let sequence = build_candidate_output_sequence(prepared, &candidate, context)?;
        let frame_retention_score = sequence.frames.len() as f64 / base_frame_count as f64;
        let score = optimizer_goal_score(
            optimizer_goal,
            f64::from(prepared.base_fps),
            candidate.fps,
            candidate.content_scale,
            &candidate.preset,
            duration_seconds,
            duration_seconds,
            frame_retention_score,
        );
        candidate.duration_seconds = duration_seconds;
        candidate.score = score;
        candidate.source_similarity_score = score;
        candidate.summary = locale::candidate_summary(
            locale,
            candidate.fps,
            candidate.content_scale,
            &candidate.preset,
            duration_seconds,
        );
        let sample_suffix = if candidate.frame_sample_step > 1 {
            format!("-every{}", candidate.frame_sample_step)
        } else {
            String::new()
        };
        candidate.id = format!(
            "{}-{}-{}fps-{}scale{}-{}ms",
            candidate.fit_mode,
            candidate.preset,
            candidate.fps,
            (candidate.content_scale * 100.0).round() as u32,
            sample_suffix,
            (duration_seconds * 1000.0).round() as u64
        );
        synchronized.push(candidate);
    }

    plan.candidates = select_ranked_candidate_subset(synchronized, plan.search_budget);
    Ok(())
}

fn prepare_optimizer_plan(
    request: &OptimizerPlanRequest,
    locale: UiLocale,
) -> OptimizerPlanResponse {
    prepare_optimizer_plan_with_checkpoint(request, locale, &mut || Ok(()))
}

fn prepare_optimizer_plan_with_checkpoint(
    request: &OptimizerPlanRequest,
    locale: UiLocale,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> OptimizerPlanResponse {
    let (fit_mode, fit_warning) = normalized_fit_mode(request.fit_mode.as_deref(), locale);
    let optimizer_goal = normalized_optimizer_goal(
        request.optimizer_goal.as_deref(),
        request.preset_strategy.as_deref(),
    );
    let quality_frame_drop_interval =
        normalized_quality_frame_drop_interval(request.quality_frame_drop_interval);
    let preset_strategy = normalized_preset_strategy(request.preset_strategy.as_deref());
    let search_depth = normalized_search_depth(request.search_depth.as_deref());
    let search_budget = search_budget_for_depth(search_depth);
    let mut warnings = Vec::new();

    if let Some(warning) = fit_warning {
        warnings.push(warning);
    }
    if let Err(error) = checkpoint() {
        return optimizer_plan_pipeline_error(locale, warnings, &error);
    }

    let resolved_crop_region = match resolve_crop_region(
        request.crop_region.as_ref(),
        request.input_width,
        request.input_height,
        locale,
    ) {
        Ok(region) => region,
        Err(error) => {
            return OptimizerPlanResponse {
                ok: false,
                fit_mode: fit_mode.into(),
                selected_duration_seconds: None,
                recommended_max_duration_seconds: duration_us_to_seconds(
                    RECOMMENDED_MAX_DURATION_US,
                ),
                search_budget,
                warnings,
                candidates: Vec::new(),
                reason_code: Some(MediaOperationReasonCode::InvalidCrop),
                error_code: Some(MediaOperationErrorCode::InvalidRequest),
                error_message: Some(error),
            }
        }
    };

    if resolved_crop_region.is_some() {
        warnings.push(locale::crop_applied_before_scale_warning(locale));
    }

    let effective_input_width = request.input_width;
    let effective_input_height = request.input_height;

    if let Some(timeline_frames) =
        match resolve_timeline_frames(request.timeline_frames.as_ref(), request.base_frame_count) {
            Ok(timeline_frames) => match (timeline_frames, optimizer_goal) {
                (Some(timeline_frames), "quality") => {
                    match apply_quality_frame_drop_to_timeline_frames(
                        timeline_frames,
                        quality_frame_drop_interval,
                    ) {
                        Ok(timeline_frames) => Some(timeline_frames),
                        Err(error) => {
                            return OptimizerPlanResponse {
                                ok: false,
                                fit_mode: fit_mode.into(),
                                selected_duration_seconds: None,
                                recommended_max_duration_seconds: duration_us_to_seconds(
                                    RECOMMENDED_MAX_DURATION_US,
                                ),
                                search_budget,
                                warnings,
                                candidates: Vec::new(),
                                reason_code: MediaOperationReasonCode::from_code(error),
                                error_code: Some(
                                    MediaOperationErrorCode::for_invalid_request_reason(error),
                                ),
                                error_message: Some(locale::frame_selection_required_error(locale)),
                            }
                        }
                    }
                }
                (timeline_frames, _) => timeline_frames,
            },
            Err("no-frames-selected") => {
                return OptimizerPlanResponse {
                    ok: false,
                    fit_mode: fit_mode.into(),
                    selected_duration_seconds: None,
                    recommended_max_duration_seconds: duration_us_to_seconds(
                        RECOMMENDED_MAX_DURATION_US,
                    ),
                    search_budget,
                    warnings,
                    candidates: Vec::new(),
                    reason_code: Some(MediaOperationReasonCode::NoFramesSelected),
                    error_code: Some(MediaOperationErrorCode::InvalidRequest),
                    error_message: Some(locale::frame_selection_required_error(locale)),
                };
            }
            Err(reason) => {
                let error_message = if reason == "invalid-frame-duration" {
                    locale::invalid_frame_duration_error(locale)
                } else {
                    locale::invalid_frame_selection_error(locale)
                };
                return OptimizerPlanResponse {
                    ok: false,
                    fit_mode: fit_mode.into(),
                    selected_duration_seconds: None,
                    recommended_max_duration_seconds: duration_us_to_seconds(
                        RECOMMENDED_MAX_DURATION_US,
                    ),
                    search_budget,
                    warnings,
                    candidates: Vec::new(),
                    reason_code: MediaOperationReasonCode::from_code(reason),
                    error_code: Some(MediaOperationErrorCode::for_invalid_request_reason(reason)),
                    error_message: Some(error_message),
                };
            }
        }
    {
        let total_duration_us = match timeline_duration_us(&timeline_frames) {
            Ok(duration_us) => duration_us,
            Err(error) => {
                return OptimizerPlanResponse {
                    ok: false,
                    fit_mode: fit_mode.into(),
                    selected_duration_seconds: None,
                    recommended_max_duration_seconds: duration_us_to_seconds(
                        RECOMMENDED_MAX_DURATION_US,
                    ),
                    search_budget,
                    warnings,
                    candidates: Vec::new(),
                    reason_code: MediaOperationReasonCode::from_code(error),
                    error_code: Some(MediaOperationErrorCode::for_invalid_request_reason(error)),
                    error_message: Some(locale::invalid_frame_selection_error(locale)),
                };
            }
        };
        let total_duration_seconds = duration_us_to_seconds(total_duration_us);

        if total_duration_us > DISCORD_MAX_DURATION_US {
            return OptimizerPlanResponse {
                ok: false,
                fit_mode: fit_mode.into(),
                selected_duration_seconds: Some(total_duration_seconds),
                recommended_max_duration_seconds: duration_us_to_seconds(
                    RECOMMENDED_MAX_DURATION_US,
                ),
                search_budget,
                warnings,
                candidates: Vec::new(),
                reason_code: Some(MediaOperationReasonCode::DurationTooLong),
                error_code: Some(MediaOperationErrorCode::InvalidRequest),
                error_message: Some(locale::selected_duration_limit_error(locale)),
            };
        }

        if total_duration_us > RECOMMENDED_MAX_DURATION_US {
            warnings.push(locale::recommended_duration_warning(locale));
        }

        let effective_fps = timeline_average_fps(&timeline_frames, total_duration_us)
            .round()
            .clamp(1.0, 30.0) as u32;

        return OptimizerPlanResponse {
            ok: true,
            fit_mode: fit_mode.into(),
            selected_duration_seconds: Some(total_duration_seconds),
            recommended_max_duration_seconds: duration_us_to_seconds(RECOMMENDED_MAX_DURATION_US),
            search_budget,
            warnings,
            candidates: build_candidate_ladder_fixed_duration(
                total_duration_seconds,
                effective_fps,
                effective_input_width,
                effective_input_height,
                preset_strategy,
                optimizer_goal,
                search_budget,
                locale,
            ),
            reason_code: None,
            error_code: None,
            error_message: None,
        };
    }

    let frame_selection =
        match resolve_frame_selection(request.selected_frames.as_ref(), request.base_frame_count) {
            Ok(selection) => selection,
            Err("no-frames-selected") => {
                return OptimizerPlanResponse {
                    ok: false,
                    fit_mode: fit_mode.into(),
                    selected_duration_seconds: None,
                    recommended_max_duration_seconds: duration_us_to_seconds(
                        RECOMMENDED_MAX_DURATION_US,
                    ),
                    search_budget,
                    warnings,
                    candidates: Vec::new(),
                    reason_code: Some(MediaOperationReasonCode::NoFramesSelected),
                    error_code: Some(MediaOperationErrorCode::InvalidRequest),
                    error_message: Some(locale::frame_selection_required_error(locale)),
                };
            }
            Err(_) => {
                return OptimizerPlanResponse {
                    ok: false,
                    fit_mode: fit_mode.into(),
                    selected_duration_seconds: None,
                    recommended_max_duration_seconds: duration_us_to_seconds(
                        RECOMMENDED_MAX_DURATION_US,
                    ),
                    search_budget,
                    warnings,
                    candidates: Vec::new(),
                    reason_code: Some(MediaOperationReasonCode::InvalidFrameSelection),
                    error_code: Some(MediaOperationErrorCode::InvalidRequest),
                    error_message: Some(locale::invalid_frame_selection_error(locale)),
                };
            }
        };
    let frame_selection = if optimizer_goal == "quality" {
        match apply_quality_frame_drop_to_selection(frame_selection, quality_frame_drop_interval) {
            Ok(selection) => selection,
            Err(error) => {
                return OptimizerPlanResponse {
                    ok: false,
                    fit_mode: fit_mode.into(),
                    selected_duration_seconds: None,
                    recommended_max_duration_seconds: duration_us_to_seconds(
                        RECOMMENDED_MAX_DURATION_US,
                    ),
                    search_budget,
                    warnings,
                    candidates: Vec::new(),
                    reason_code: MediaOperationReasonCode::from_code(error),
                    error_code: Some(MediaOperationErrorCode::for_invalid_request_reason(error)),
                    error_message: Some(locale::frame_selection_required_error(locale)),
                }
            }
        }
    } else {
        frame_selection
    };

    let source_fps = derive_source_fps(
        request.avg_fps,
        request.source_duration_seconds,
        frame_selection.base_frame_count,
    );
    let natural_duration_seconds = projected_selection_duration_seconds(
        frame_selection.selected_frame_count,
        frame_selection.base_frame_count,
        request.source_duration_seconds,
        source_fps,
    );

    if natural_duration_seconds > duration_us_to_seconds(DISCORD_MAX_DURATION_US) {
        return OptimizerPlanResponse {
            ok: false,
            fit_mode: fit_mode.into(),
            selected_duration_seconds: Some(natural_duration_seconds),
            recommended_max_duration_seconds: duration_us_to_seconds(RECOMMENDED_MAX_DURATION_US),
            search_budget,
            warnings,
            candidates: Vec::new(),
            reason_code: Some(MediaOperationReasonCode::DurationTooLong),
            error_code: Some(MediaOperationErrorCode::InvalidRequest),
            error_message: Some(locale::selected_duration_limit_error(locale)),
        };
    }

    if natural_duration_seconds > duration_us_to_seconds(RECOMMENDED_MAX_DURATION_US) {
        warnings.push(locale::recommended_duration_warning(locale));
    }

    let candidates = match build_candidate_ladder_with_checkpoint(
        frame_selection.selected_frame_count,
        source_fps,
        natural_duration_seconds,
        effective_input_width,
        effective_input_height,
        preset_strategy,
        optimizer_goal,
        search_budget,
        locale,
        checkpoint,
    ) {
        Ok(candidates) => candidates,
        Err(error) => return optimizer_plan_pipeline_error(locale, warnings, &error),
    };

    OptimizerPlanResponse {
        ok: true,
        fit_mode: fit_mode.into(),
        selected_duration_seconds: Some(natural_duration_seconds),
        recommended_max_duration_seconds: duration_us_to_seconds(RECOMMENDED_MAX_DURATION_US),
        search_budget,
        warnings,
        candidates,
        reason_code: None,
        error_code: None,
        error_message: None,
    }
}

fn prepare_optimizer_plan_with_operation(
    request: &OptimizerPlanRequest,
    locale: UiLocale,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> OptimizerPlanResponse {
    publish_progress(progress, context, ProgressStage::Estimating, 0, None);
    let response =
        prepare_optimizer_plan_with_checkpoint(request, locale, &mut || context.checkpoint());
    let warnings = response.warnings.clone();
    match context.finalize(|| {
        publish_progress(progress, context, ProgressStage::Finalizing, 1, Some(1));
        Ok(response)
    }) {
        Ok(response) => response,
        Err(error) => optimizer_plan_pipeline_error(locale, warnings, &error),
    }
}

fn validate_candidate_ids(candidate_ids: &[String]) -> Result<(), PipelineError> {
    if candidate_ids.is_empty() || candidate_ids.len() > 5 {
        return Err(PipelineError::InvalidRequestWithoutReason);
    }
    let mut unique = BTreeSet::new();
    for candidate_id in candidate_ids {
        if candidate_id.trim().is_empty() || !unique.insert(candidate_id.as_str()) {
            return Err(PipelineError::InvalidRequestWithoutReason);
        }
    }
    Ok(())
}

fn resolve_candidate_frame_view(
    request: &OptimizerPlanRequest,
) -> Result<(Option<Vec<ResolvedTimelineFrame>>, Option<Vec<u32>>), PipelineError> {
    let optimizer_goal = normalized_optimizer_goal(
        request.optimizer_goal.as_deref(),
        request.preset_strategy.as_deref(),
    );
    let quality_interval =
        normalized_quality_frame_drop_interval(request.quality_frame_drop_interval);
    let timeline_frames =
        resolve_timeline_frames(request.timeline_frames.as_ref(), request.base_frame_count)
            .map_err(|reason| PipelineError::InvalidRequest { reason })?;
    if let Some(timeline_frames) = timeline_frames {
        let timeline_frames = if optimizer_goal == "quality" {
            apply_quality_frame_drop_to_timeline_frames(timeline_frames, quality_interval)
                .map_err(|reason| PipelineError::InvalidRequest { reason })?
        } else {
            timeline_frames
        };
        return Ok((Some(timeline_frames), None));
    }

    let selection =
        resolve_frame_selection(request.selected_frames.as_ref(), request.base_frame_count)
            .map_err(|reason| PipelineError::InvalidRequest { reason })?;
    let selection = if optimizer_goal == "quality" {
        apply_quality_frame_drop_to_selection(selection, quality_interval)
            .map_err(|reason| PipelineError::InvalidRequest { reason })?
    } else {
        selection
    };
    Ok((None, selection.selected_frames))
}

fn prepare_candidate_estimation_source(
    request: &OptimizerPlanRequest,
    requested_candidate_ids: &[String],
    input_path: &Path,
    source_revision: &str,
    locale: UiLocale,
    loader: &impl FrameSourceLoader,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> Result<(OptimizerPlanResponse, PreparedSearchSource), PipelineError> {
    context.checkpoint()?;
    let plan =
        prepare_optimizer_plan_with_checkpoint(request, locale, &mut || context.checkpoint());
    if !plan.ok {
        return Err(PipelineError::InvalidRequest {
            reason: "plan-invalid",
        });
    }
    if requested_candidate_ids.iter().any(|candidate_id| {
        !plan
            .candidates
            .iter()
            .any(|candidate| &candidate.id == candidate_id)
    }) {
        return Err(PipelineError::InvalidRequestWithoutReason);
    }
    let (resolved_timeline_frames, selected_frame_indexes) = resolve_candidate_frame_view(request)?;
    let optimizer_goal = normalized_optimizer_goal(
        request.optimizer_goal.as_deref(),
        request.preset_strategy.as_deref(),
    );
    publish_progress(progress, context, ProgressStage::Decoding, 0, Some(1));
    let prepared = loader.prepare(
        FramePreparationRequest {
            input_path,
            source_revision,
            crop_region: request.crop_region.as_ref(),
            input_width: request.input_width,
            input_height: request.input_height,
            base_frame_count: request.base_frame_count,
            timeline_frames: request.timeline_frames.as_deref(),
            resolved_timeline_frames: resolved_timeline_frames.as_deref(),
            selected_frame_indexes: selected_frame_indexes.as_deref(),
            source_duration_seconds: request.source_duration_seconds,
            avg_fps: request.avg_fps,
            locale,
            optimizer_goal,
        },
        &plan,
        context,
        MediaLimits::default(),
    )?;
    publish_progress(progress, context, ProgressStage::Decoding, 1, Some(1));
    Ok((plan, prepared))
}

fn synchronize_candidate_estimation_plan(
    plan: &mut OptimizerPlanResponse,
    prepared: &PreparedSearchSource,
    request: &OptimizerPlanRequest,
    locale: UiLocale,
    context: &OperationContext,
) -> Result<(), PipelineError> {
    synchronize_candidates_with_prepared_duration(
        plan,
        prepared,
        request.input_width,
        request.input_height,
        normalized_preset_strategy(request.preset_strategy.as_deref()),
        normalized_optimizer_goal(
            request.optimizer_goal.as_deref(),
            request.preset_strategy.as_deref(),
        ),
        locale,
        context,
    )
}

fn estimate_optimizer_candidates_with_loader(
    request: &OptimizerSizeEstimateRequest,
    preflight: &SourcePreflight,
    locale: UiLocale,
    loader: &impl FrameSourceLoader,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> Result<Vec<OutputSizeEstimate>, PipelineError> {
    validate_candidate_ids(&request.candidate_ids)?;
    let sample_seed = parse_sample_seed(&request.sample_seed)?;
    let source_revision = preflight.identity().revision();
    let (mut plan, prepared) = prepare_candidate_estimation_source(
        &request.plan,
        &request.candidate_ids,
        preflight.canonical_path(),
        &source_revision,
        locale,
        loader,
        context,
        progress,
    )?;
    checkpointed_source_check(context, preflight.identity(), MediaLimits::default())?;
    synchronize_candidate_estimation_plan(&mut plan, &prepared, &request.plan, locale, context)?;
    if request.candidate_ids.iter().any(|candidate_id| {
        !plan
            .candidates
            .iter()
            .any(|candidate| &candidate.id == candidate_id)
    }) {
        return Err(PipelineError::InvalidRequestWithoutReason);
    }

    let total = u32::try_from(request.candidate_ids.len()).unwrap_or(u32::MAX);
    publish_progress(progress, context, ProgressStage::Estimating, 0, Some(total));
    let mut estimates = Vec::with_capacity(request.candidate_ids.len());
    for (index, candidate_id) in request.candidate_ids.iter().enumerate() {
        context.checkpoint()?;
        checkpointed_source_check(context, preflight.identity(), MediaLimits::default())?;
        let candidate = plan
            .candidates
            .iter()
            .find(|candidate| &candidate.id == candidate_id)
            .ok_or(PipelineError::InvalidRequestWithoutReason)?;
        let sequence = build_candidate_output_sequence(&prepared, candidate, context)?;
        estimates.push(estimate_candidate_size(&sequence, sample_seed, context)?);
        publish_progress(
            progress,
            context,
            ProgressStage::Estimating,
            u32::try_from(index + 1).unwrap_or(u32::MAX),
            Some(total),
        );
    }
    checkpointed_source_check(context, preflight.identity(), MediaLimits::default())?;
    Ok(estimates)
}

fn probe_optimizer_candidate_with_loader(
    request: &CandidateSizeProbeRequest,
    preflight: &SourcePreflight,
    locale: UiLocale,
    loader: &impl FrameSourceLoader,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> Result<OutputSizeEstimate, PipelineError> {
    validate_candidate_ids(std::slice::from_ref(&request.candidate_id))?;
    let source_revision = preflight.identity().revision();
    let (mut plan, prepared) = prepare_candidate_estimation_source(
        &request.plan,
        std::slice::from_ref(&request.candidate_id),
        preflight.canonical_path(),
        &source_revision,
        locale,
        loader,
        context,
        progress,
    )?;
    checkpointed_source_check(context, preflight.identity(), MediaLimits::default())?;
    synchronize_candidate_estimation_plan(&mut plan, &prepared, &request.plan, locale, context)?;
    let candidate = plan
        .candidates
        .iter()
        .find(|candidate| candidate.id == request.candidate_id)
        .ok_or(PipelineError::InvalidRequestWithoutReason)?;
    publish_progress(progress, context, ProgressStage::Encoding, 0, Some(1));
    let sequence = build_candidate_output_sequence(&prepared, candidate, context)?;
    let estimate = probe_candidate_size(&sequence, context)?;
    checkpointed_source_check(context, preflight.identity(), MediaLimits::default())?;
    publish_progress(progress, context, ProgressStage::Encoding, 1, Some(1));
    Ok(estimate)
}

fn encode_prepared_candidate_with_checkpoint(
    input_path: &Path,
    output_directory: Option<&str>,
    locale: UiLocale,
    sequence: &PreparedCandidateSequence<'_, '_>,
    expected_source: Option<&SourceIdentity>,
    context: &OperationContext,
) -> Result<EncodeResult, PipelineError> {
    context.checkpoint()?;
    let input_path_display = input_path.to_string_lossy();
    let output_directory = resolve_output_directory(output_directory, &input_path_display, locale)
        .map_err(|_| PipelineError::InvalidRequest {
            reason: "invalid-output-directory",
        })?;
    if let Some(expected_source) = expected_source {
        checkpointed_source_check(context, expected_source, MediaLimits::default())?;
    }

    let durations_us = sequence
        .frames
        .iter()
        .map(|frame| frame.duration_us)
        .collect::<Vec<_>>();
    if checked_timeline_duration(sequence.frames.as_ref())? != sequence.duration_us {
        return Err(PipelineError::InvalidRequest {
            reason: "invalid-frame-selection",
        });
    }
    let transformed_frames = sequence.frames.iter().map(|frame| {
        let source_pixels = sequence
            .source
            .pixels_for_source_index(frame.source_frame_index)?;
        Ok(StickerFrame {
            pixels: scale_prepared_frame_for_candidate(
                source_pixels.as_ref(),
                sequence.candidate.content_scale,
            ),
            duration_us: frame.duration_us,
        })
    });

    let started = Instant::now();
    let mut pending_output =
        PendingOutput::new(&output_directory, input_path, &sequence.candidate.id, "png")?;
    write_native_apng_iterator_with_checkpoint(
        pending_output.writer(),
        sequence.frames.len(),
        &durations_us,
        transformed_frames,
        &sequence.candidate.preset,
        || context.checkpoint(),
    )?;
    let size_bytes = pending_output_size(&mut pending_output)?;

    Ok(EncodeResult {
        pending_output,
        size_bytes,
        elapsed_ms: started.elapsed().as_millis() as u64,
        tool_source: sequence.source.tool_source.clone(),
        tool_command: sequence.source.tool_command.clone(),
        tool_detail: sequence.source.tool_detail.clone(),
    })
}

fn encode_candidate_internal(
    input_path: &str,
    output_directory: Option<&str>,
    locale: UiLocale,
    crop_region: Option<&CropRegion>,
    input_width: Option<u32>,
    input_height: Option<u32>,
    candidate: &CandidatePreview,
    selected_frames: Option<&[u32]>,
    timeline_frames: Option<&[ResolvedTimelineFrame]>,
    expected_source: Option<&SourceIdentity>,
) -> Result<EncodeResult, PipelineError> {
    let context = OperationContext::detached(MediaOperationKind::OptimizerSearch.timeout());
    let input_path = Path::new(input_path);
    let source_revision = expected_source
        .map(SourceIdentity::revision)
        .unwrap_or_default();
    let source_duration_seconds = timeline_frames
        .and_then(|frames| checked_timeline_duration(frames).ok())
        .map(duration_us_to_seconds);
    let plan = OptimizerPlanResponse {
        ok: true,
        fit_mode: CANONICAL_FIT_MODE.into(),
        selected_duration_seconds: source_duration_seconds,
        recommended_max_duration_seconds: duration_us_to_seconds(RECOMMENDED_MAX_DURATION_US),
        search_budget: 1,
        warnings: Vec::new(),
        candidates: vec![candidate.clone()],
        error_code: None,
        reason_code: None,
        error_message: None,
    };
    let loader = DefaultFrameSourceLoader;
    let prepared = loader.prepare(
        FramePreparationRequest {
            input_path,
            source_revision: &source_revision,
            crop_region,
            input_width,
            input_height,
            base_frame_count: None,
            timeline_frames: None,
            resolved_timeline_frames: timeline_frames,
            selected_frame_indexes: selected_frames,
            source_duration_seconds,
            avg_fps: Some(f64::from(candidate.fps.max(1))),
            locale,
            optimizer_goal: "balanced",
        },
        &plan,
        &context,
        MediaLimits::default(),
    )?;
    let sequence = build_candidate_output_sequence(&prepared, candidate, &context)?;
    encode_prepared_candidate_with_checkpoint(
        input_path,
        output_directory,
        locale,
        &sequence,
        expected_source,
        &context,
    )
}

fn convert_static_image_to_png_internal(
    input_path: &str,
    output_directory: Option<&str>,
    crop_region: Option<&CropRegion>,
    locale: UiLocale,
    expected_source: Option<&SourceIdentity>,
) -> StaticImageConversionResult {
    convert_static_image_to_png_with_callbacks(
        input_path,
        output_directory,
        crop_region,
        locale,
        expected_source,
        None,
        None,
        || Ok(()),
        |_, _, _| {},
    )
}

fn convert_static_image_to_png_with_operation(
    input_path: &str,
    output_directory: Option<&str>,
    crop_region: Option<&CropRegion>,
    locale: UiLocale,
    preflight: &SourcePreflight,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> StaticImageConversionResult {
    convert_static_image_to_png_with_callbacks(
        input_path,
        output_directory,
        crop_region,
        locale,
        Some(preflight.identity()),
        Some(preflight),
        Some(context),
        || context.checkpoint(),
        |stage, completed, total| {
            publish_progress(progress, context, stage, completed, total);
        },
    )
}

fn convert_static_image_to_png_with_callbacks(
    input_path: &str,
    output_directory: Option<&str>,
    crop_region: Option<&CropRegion>,
    locale: UiLocale,
    expected_source: Option<&SourceIdentity>,
    preflight: Option<&SourcePreflight>,
    context: Option<&OperationContext>,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
    mut progress: impl FnMut(ProgressStage, u32, Option<u32>),
) -> StaticImageConversionResult {
    progress(ProgressStage::Inspecting, 0, Some(1));
    let inspection = match preflight {
        Some(preflight) => {
            if let Err(error) = checkpoint() {
                return static_conversion_pipeline_error(&error, locale);
            }
            inspect_input_media_preflighted_with_callbacks(
                input_path,
                preflight,
                locale,
                context,
                &mut checkpoint,
            )
        }
        None => inspect_input_media_with_callbacks(
            input_path,
            locale,
            context,
            || checkpoint(),
            |_, _, _| {},
        ),
    };

    if !inspection.ok {
        return StaticImageConversionResult {
            ok: false,
            output_path: None,
            size_bytes: None,
            elapsed_ms: None,
            tool_source: inspection.tool_source,
            tool_command: inspection.tool_command,
            tool_detail: inspection.tool_detail,
            warnings: Vec::new(),
            reason_code: inspection.reason_code,
            error_code: inspection.error_code,
            error_message: inspection.error_message,
        };
    }

    if !inspection.is_static_image {
        return StaticImageConversionResult {
            ok: false,
            output_path: None,
            size_bytes: None,
            elapsed_ms: None,
            tool_source: inspection.tool_source,
            tool_command: inspection.tool_command,
            tool_detail: inspection.tool_detail,
            warnings: Vec::new(),
            reason_code: Some(MediaOperationReasonCode::UnsupportedSourceFormat),
            error_code: Some(MediaOperationErrorCode::InvalidRequest),
            error_message: Some(locale::unsupported_still_image_error(locale)),
        };
    }

    let output_directory = match resolve_output_directory(output_directory, input_path, locale) {
        Ok(directory) => directory,
        Err(_) => {
            return static_conversion_pipeline_error(
                &PipelineError::InvalidRequest {
                    reason: "invalid-output-directory",
                },
                locale,
            )
        }
    };

    let resolved_crop_region =
        match resolve_crop_region(crop_region, inspection.width, inspection.height, locale) {
            Ok(region) => region,
            Err(error) => {
                return StaticImageConversionResult {
                    ok: false,
                    output_path: None,
                    size_bytes: None,
                    elapsed_ms: None,
                    tool_source: inspection.tool_source,
                    tool_command: inspection.tool_command,
                    tool_detail: inspection.tool_detail,
                    warnings: Vec::new(),
                    reason_code: Some(MediaOperationReasonCode::InvalidCrop),
                    error_code: Some(MediaOperationErrorCode::InvalidRequest),
                    error_message: Some(error),
                }
            }
        };

    let started = Instant::now();
    progress(ProgressStage::Decoding, 0, Some(1));
    let source_pixels =
        match decode_still_rgba_image(input_path, MediaLimits::default(), || checkpoint()) {
            Ok(pixels) => pixels,
            Err(error) => {
                return StaticImageConversionResult {
                    ok: false,
                    output_path: None,
                    size_bytes: None,
                    elapsed_ms: None,
                    tool_source: Some("native".into()),
                    tool_command: None,
                    tool_detail: Some(locale::native_image_detail(locale)),
                    warnings: Vec::new(),
                    reason_code: error.media_reason_code(),
                    error_code: Some(error.error_code()),
                    error_message: Some(pipeline_error_diagnostic(&error, locale)),
                }
            }
        };
    if let Some(expected_source) = expected_source {
        if let Err(error) = checkpoint() {
            return static_conversion_pipeline_error(&error, locale);
        }
        let source_check = ensure_source_unchanged(expected_source, MediaLimits::default());
        if let Err(error) = checkpoint() {
            return static_conversion_pipeline_error(&error, locale);
        }
        if let Err(error) = source_check {
            return StaticImageConversionResult {
                ok: false,
                output_path: None,
                size_bytes: None,
                elapsed_ms: None,
                tool_source: Some("native".into()),
                tool_command: None,
                tool_detail: Some(locale::native_image_detail(locale)),
                warnings: Vec::new(),
                reason_code: error.media_reason_code(),
                error_code: Some(error.error_code()),
                error_message: Some(pipeline_error_diagnostic(&error, locale)),
            };
        }
    }
    if let Err(error) = checkpoint() {
        return static_conversion_pipeline_error(&error, locale);
    }
    let output_pixels = transform_frame_for_static_png(&source_pixels, resolved_crop_region);
    if let Err(error) = checkpoint() {
        return static_conversion_pipeline_error(&error, locale);
    }
    progress(ProgressStage::Encoding, 0, Some(1));
    let publication = (|| -> Result<(PathBuf, u64), PipelineError> {
        let mut pending_output =
            PendingOutput::new(&output_directory, Path::new(input_path), "png", "png")?;
        checkpoint()?;
        write_native_png(pending_output.writer(), &output_pixels)?;
        checkpoint()?;
        let size_bytes = pending_output_size(&mut pending_output)?;
        progress(ProgressStage::Encoding, 1, Some(1));
        let output_path = match (context, expected_source) {
            (Some(context), Some(expected_source)) => {
                finalize_source_publication(context, expected_source, || {
                    progress(ProgressStage::Finalizing, 1, Some(1));
                    pending_output.commit()
                })?
            }
            _ => {
                progress(ProgressStage::Finalizing, 1, Some(1));
                checkpoint()?;
                commit_output_after_source_validation(pending_output, expected_source)?
            }
        };
        Ok((output_path, size_bytes))
    })();

    match publication {
        Ok((output_path, size_bytes)) => StaticImageConversionResult {
            ok: true,
            output_path: Some(output_path.to_string_lossy().into_owned()),
            size_bytes: Some(size_bytes),
            elapsed_ms: Some(started.elapsed().as_millis() as u64),
            tool_source: Some("native".into()),
            tool_command: None,
            tool_detail: Some(locale::native_png_encode_detail(locale)),
            warnings: Vec::new(),
            reason_code: None,
            error_code: None,
            error_message: None,
        },
        Err(error) => static_conversion_pipeline_error(&error, locale),
    }
}

fn static_conversion_pipeline_error(
    error: &PipelineError,
    locale: UiLocale,
) -> StaticImageConversionResult {
    StaticImageConversionResult {
        ok: false,
        output_path: None,
        size_bytes: None,
        elapsed_ms: None,
        tool_source: None,
        tool_command: None,
        tool_detail: None,
        warnings: Vec::new(),
        reason_code: error.media_reason_code(),
        error_code: Some(error.error_code()),
        error_message: Some(pipeline_error_diagnostic(error, locale)),
    }
}

fn optimizer_plan_pipeline_error(
    locale: UiLocale,
    warnings: Vec<String>,
    error: &PipelineError,
) -> OptimizerPlanResponse {
    OptimizerPlanResponse {
        ok: false,
        fit_mode: CANONICAL_FIT_MODE.into(),
        selected_duration_seconds: None,
        recommended_max_duration_seconds: duration_us_to_seconds(RECOMMENDED_MAX_DURATION_US),
        search_budget: MAX_SEARCH_BUDGET,
        warnings,
        candidates: Vec::new(),
        reason_code: error.media_reason_code(),
        error_code: Some(error.error_code()),
        error_message: Some(pipeline_error_diagnostic(error, locale)),
    }
}

fn inspection_pipeline_error(
    input_path: &str,
    error: &PipelineError,
    locale: UiLocale,
) -> MediaInspection {
    inspection_error(
        input_path,
        Some("native".into()),
        None,
        None,
        error.code(),
        pipeline_error_diagnostic(error, locale),
    )
}

fn inspection_pipeline_error_with_provenance(
    input_path: &str,
    inspection: &MediaInspection,
    error: &PipelineError,
    locale: UiLocale,
) -> MediaInspection {
    inspection_error_with_fallback_reason(
        input_path,
        inspection.tool_source.clone(),
        inspection.tool_command.clone(),
        inspection.tool_detail.clone(),
        inspection.fallback_reason_code.clone(),
        error.code(),
        pipeline_error_diagnostic(error, locale),
    )
}

fn inspection_error(
    input_path: &str,
    tool_source: Option<String>,
    tool_command: Option<String>,
    tool_detail: Option<String>,
    error_code: &str,
    error_message: String,
) -> MediaInspection {
    inspection_error_with_fallback_reason(
        input_path,
        tool_source,
        tool_command,
        tool_detail,
        None,
        error_code,
        error_message,
    )
}

fn inspection_error_with_fallback_reason(
    input_path: &str,
    tool_source: Option<String>,
    tool_command: Option<String>,
    tool_detail: Option<String>,
    fallback_reason_code: Option<String>,
    error_code: &str,
    error_message: String,
) -> MediaInspection {
    let (error_code, reason_code) = normalize_backend_error_code(error_code);
    MediaInspection {
        ok: false,
        input_path: input_path.to_string(),
        source_revision: None,
        tool_source,
        tool_command,
        tool_detail,
        fallback_reason_code,
        format_name: None,
        duration_seconds: None,
        size_bytes: None,
        width: None,
        height: None,
        codec_name: None,
        pixel_format: None,
        avg_fps: None,
        frame_rate_label: None,
        estimated_frames: None,
        frame_durations_seconds: None,
        warnings: Vec::new(),
        is_static_image: false,
        can_convert_to_png: false,
        reason_code,
        error_code: Some(error_code),
        error_message: Some(error_message),
    }
}

fn annotate_media_foundation_fallback(
    mut inspection: MediaInspection,
    locale: UiLocale,
) -> MediaInspection {
    inspection.fallback_reason_code = Some(MEDIA_FOUNDATION_FAILED_REASON_CODE.into());
    if inspection.tool_source.as_deref() == Some("sidecar") {
        inspection.tool_detail = Some(if inspection.ok {
            locale::media_foundation_fallback_warning(locale)
        } else {
            locale::media_foundation_fallback_attempt_detail(locale)
        });
    }
    inspection
}

fn apply_inspection_source_revision(
    identity: &SourceIdentity,
    input_path: &str,
    mut inspection: MediaInspection,
) -> MediaInspection {
    inspection.input_path = input_path.to_string();
    inspection.source_revision = inspection.ok.then(|| identity.revision());
    inspection
}

fn enforce_desktop_inspection_frame_limit(
    inspection: MediaInspection,
    locale: UiLocale,
) -> MediaInspection {
    let limit = u64::from(MediaLimits::default().max_frame_count);
    let Some(actual) = inspection.estimated_frames.filter(|count| *count > limit) else {
        return inspection;
    };
    let error = PipelineError::LimitExceeded {
        resource: "frame-count",
        limit,
        actual,
    };
    let MediaInspection {
        input_path,
        tool_source,
        tool_command,
        tool_detail,
        fallback_reason_code,
        ..
    } = inspection;

    inspection_error_with_fallback_reason(
        &input_path,
        tool_source,
        tool_command,
        tool_detail,
        fallback_reason_code,
        error.code(),
        pipeline_error_diagnostic(&error, locale),
    )
}

fn normalize_backend_error_code(
    error_code: &str,
) -> (MediaOperationErrorCode, Option<MediaOperationReasonCode>) {
    if let Some(error_code) = MediaOperationErrorCode::from_code(error_code) {
        return (error_code, None);
    }
    if let Some(reason_code) = MediaOperationReasonCode::from_code(error_code) {
        return (MediaOperationErrorCode::InvalidRequest, Some(reason_code));
    }
    match error_code {
        "inspect-failed" => (
            MediaOperationErrorCode::MalformedMedia,
            Some(MediaOperationReasonCode::DecodeFailed),
        ),
        "tool-unavailable" => (MediaOperationErrorCode::ToolMissing, None),
        _ => (MediaOperationErrorCode::InternalTaskFailed, None),
    }
}

fn pipeline_error_diagnostic(error: &PipelineError, locale: UiLocale) -> String {
    let base = locale::media_pipeline_diagnostic(locale, error.code());
    let detail = match error {
        PipelineError::MalformedInput { reason, .. } if reason == "decoder panicked" => {
            Some("decoder-panic".to_string())
        }
        PipelineError::MalformedInput {
            format: "png",
            reason,
        } if reason.starts_with("CRC mismatch") => Some("png-parser-crc".to_string()),
        PipelineError::MalformedInput { format: "png", .. } => {
            Some("png-parser-validation".to_string())
        }
        PipelineError::MalformedInput { .. } => Some("decoder-validation".to_string()),
        PipelineError::MalformedProcessOutput { .. } => {
            Some("process-output-validation".to_string())
        }
        PipelineError::LimitExceeded {
            resource,
            limit,
            actual,
        } => {
            let resource = match *resource {
                "input-bytes" => "input-bytes",
                "image-dimensions" => "image-dimensions",
                "image-pixels" => "image-pixels",
                "frame-count" => "frame-count",
                "decoded-bytes" => "decoded-bytes",
                "png-chunk-bytes" => "png-chunk-bytes",
                _ => "unknown-limit",
            };
            Some(format!("limit:{resource}:{actual}>{limit}"))
        }
        _ => None,
    };

    match detail {
        Some(detail) => format!("{base} [{detail}]"),
        None => base,
    }
}

fn ffmpeg_inspection_failure_diagnostic(locale: UiLocale, process_succeeded: bool) -> String {
    if process_succeeded {
        locale::no_usable_video_stream_error(locale)
    } else {
        locale::media_pipeline_diagnostic(locale, "malformed-media")
    }
}

#[derive(Clone)]
struct SourcePreflight {
    identity: SourceIdentity,
    canonical_path: PathBuf,
}

impl SourcePreflight {
    fn capture(input_path: &Path, limits: MediaLimits) -> Result<Self, PipelineError> {
        let identity = SourceIdentity::from_path(input_path, limits)?;
        Ok(Self {
            canonical_path: identity.canonical_path.clone(),
            identity,
        })
    }

    fn validate_expected(
        input_path: &Path,
        source_revision: Option<&str>,
        limits: MediaLimits,
    ) -> Result<Self, PipelineError> {
        let identity = validate_source_revision(input_path, source_revision, limits)?;
        Ok(Self {
            canonical_path: identity.canonical_path.clone(),
            identity,
        })
    }

    fn identity(&self) -> &SourceIdentity {
        &self.identity
    }

    fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }
}

fn run_reserved_preflight<T>(
    context: &OperationContext,
    work: impl FnOnce() -> Result<T, PipelineError>,
) -> Result<T, PipelineError> {
    context.checkpoint()?;
    let result = work();
    context.checkpoint()?;
    result
}

fn validate_source_revision(
    input_path: &Path,
    source_revision: Option<&str>,
    limits: MediaLimits,
) -> Result<SourceIdentity, PipelineError> {
    let source_revision = source_revision
        .filter(|revision| !revision.trim().is_empty())
        .ok_or(PipelineError::InvalidRequestWithoutReason)?;
    let identity =
        SourceIdentity::from_path(input_path, limits).map_err(|_| PipelineError::SourceChanged)?;
    if identity.revision() != source_revision {
        return Err(PipelineError::SourceChanged);
    }
    Ok(identity)
}

fn ensure_source_unchanged(
    expected: &SourceIdentity,
    limits: MediaLimits,
) -> Result<(), PipelineError> {
    let current = SourceIdentity::from_path(&expected.canonical_path, limits)
        .map_err(|_| PipelineError::SourceChanged)?;
    if &current != expected {
        return Err(PipelineError::SourceChanged);
    }
    Ok(())
}

fn checkpointed_source_check(
    context: &OperationContext,
    expected: &SourceIdentity,
    limits: MediaLimits,
) -> Result<(), PipelineError> {
    context.checkpoint()?;
    let source_check = ensure_source_unchanged(expected, limits);
    context.checkpoint()?;
    source_check
}

fn finalize_managed_response<T>(
    context: &OperationContext,
    response: T,
) -> Result<T, PipelineError> {
    if context.is_completed() {
        Ok(response)
    } else {
        context.finalize(|| Ok(response))
    }
}

fn finalize_source_publication<T>(
    context: &OperationContext,
    expected: &SourceIdentity,
    publisher: impl FnOnce() -> Result<T, PipelineError>,
) -> Result<T, PipelineError> {
    context.checkpoint()?;
    let source_check = ensure_source_unchanged(expected, MediaLimits::default());
    context.checkpoint()?;
    source_check?;
    context.finalize(publisher)
}

fn finalize_source_checked<T>(
    expected: &SourceIdentity,
    response: T,
    limits: MediaLimits,
    on_source_error: impl FnOnce(PipelineError) -> T,
) -> T {
    match ensure_source_unchanged(expected, limits) {
        Ok(()) => response,
        Err(error) => on_source_error(error),
    }
}

fn finalize_static_conversion_source(
    expected: &SourceIdentity,
    response: StaticImageConversionResult,
    locale: UiLocale,
) -> StaticImageConversionResult {
    finalize_source_checked(expected, response, MediaLimits::default(), |error| {
        static_conversion_pipeline_error(&error, locale)
    })
}

fn finalize_static_conversion_source_unless_published(
    expected: &SourceIdentity,
    response: StaticImageConversionResult,
    locale: UiLocale,
) -> StaticImageConversionResult {
    if response.output_path.is_some() {
        response
    } else {
        finalize_static_conversion_source(expected, response, locale)
    }
}

fn finalize_optimizer_search_source(
    expected: &SourceIdentity,
    response: OptimizerSearchResponse,
    locale: UiLocale,
) -> OptimizerSearchResponse {
    let warnings = response.warnings.clone();
    finalize_source_checked(expected, response, MediaLimits::default(), |error| {
        optimizer_search_pipeline_error(locale, warnings, error)
    })
}

fn finalize_optimizer_search_source_unless_published(
    expected: &SourceIdentity,
    response: OptimizerSearchResponse,
    locale: UiLocale,
) -> OptimizerSearchResponse {
    if response.best_output_path.is_some() {
        response
    } else {
        finalize_optimizer_search_source(expected, response, locale)
    }
}

fn finalize_frame_preview_source(
    expected: &SourceIdentity,
    response: FramePreviewResponse,
    locale: UiLocale,
) -> FramePreviewResponse {
    finalize_source_checked(expected, response, MediaLimits::default(), |error| {
        frame_preview_pipeline_error(&error, locale)
    })
}

fn finalize_frame_previews_source(
    expected: &SourceIdentity,
    response: FramePreviewsResponse,
    locale: UiLocale,
) -> FramePreviewsResponse {
    finalize_source_checked(expected, response, MediaLimits::default(), |error| {
        frame_previews_pipeline_error(&error, locale)
    })
}

fn malformed_png(reason: impl Into<String>) -> PipelineError {
    PipelineError::MalformedInput {
        format: "png",
        reason: reason.into(),
    }
}

fn read_png_exact<R: Read>(
    reader: &mut R,
    buffer: &mut [u8],
    context: &'static str,
) -> Result<(), PipelineError> {
    reader
        .read_exact(buffer)
        .map_err(|_| malformed_png(format!("truncated {context}")))
}

fn read_png_metadata_from_reader<R: Read>(
    reader: &mut R,
    file_length: u64,
    limits: MediaLimits,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<PngFileMetadata, PipelineError> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    const SCRATCH_BYTES: usize = 64 * 1024;

    let mut signature = [0u8; 8];
    read_png_exact(reader, &mut signature, "PNG signature")?;
    if &signature != PNG_SIGNATURE {
        return Err(malformed_png("invalid PNG signature"));
    }

    let mut offset = 8u64;
    let mut width = None;
    let mut height = None;
    let mut bit_depth = None;
    let mut color_type = None;
    let mut frame_count = None;
    let mut frame_durations = Vec::new();
    let mut saw_animation_control = false;
    let mut saw_image_data = false;
    let mut saw_nonempty_idat = false;
    let mut saw_ihdr = false;
    let mut saw_plte = false;
    let mut saw_iend = false;
    let mut frame_control_count = 0u64;
    let mut expected_apng_sequence = 0u32;
    let mut pending_frame_data: Option<&'static str> = None;
    let mut active_fdat_frame = false;
    let mut saw_fdat = false;
    let mut last_chunk_was_idat = false;
    let mut saw_palette_follower = false;
    let mut seen_singleton_ancillary_chunks = BTreeSet::new();
    let mut warnings = Vec::new();

    while !saw_iend {
        checkpoint()?;
        if offset == file_length {
            return Err(malformed_png("missing IEND chunk"));
        }
        if file_length.saturating_sub(offset) < 8 {
            return Err(malformed_png("truncated PNG chunk header"));
        }

        let mut length_bytes = [0u8; 4];
        read_png_exact(reader, &mut length_bytes, "PNG chunk length")?;
        let length = u32::from_be_bytes(length_bytes);
        let mut chunk_type = [0u8; 4];
        read_png_exact(reader, &mut chunk_type, "PNG chunk type")?;
        offset = offset
            .checked_add(8)
            .ok_or_else(|| malformed_png("PNG offset overflow"))?;

        if !saw_ihdr && &chunk_type != b"IHDR" {
            return Err(malformed_png("IHDR must be the first chunk"));
        }
        if !chunk_type.iter().all(u8::is_ascii_alphabetic) || !chunk_type[2].is_ascii_uppercase() {
            return Err(malformed_png("PNG chunk type is invalid"));
        }
        if length > 0x7fff_ffff {
            return Err(malformed_png("PNG chunk length exceeds the 31-bit maximum"));
        }
        if chunk_type[0].is_ascii_uppercase()
            && !matches!(&chunk_type, b"IHDR" | b"PLTE" | b"IDAT" | b"IEND")
        {
            return Err(malformed_png("unknown critical PNG chunk"));
        }
        if !matches!(&chunk_type, b"IDAT" | b"fdAT") && length > limits.max_png_chunk_bytes {
            return Err(PipelineError::LimitExceeded {
                resource: "png-chunk-bytes",
                limit: u64::from(limits.max_png_chunk_bytes),
                actual: u64::from(length),
            });
        }

        let payload_and_crc = u64::from(length)
            .checked_add(4)
            .ok_or_else(|| malformed_png("PNG chunk length overflow"))?;
        if payload_and_crc > file_length.saturating_sub(offset) {
            return Err(malformed_png("truncated PNG chunk payload or CRC"));
        }

        let mut crc = crc32fast::Hasher::new();
        crc.update(&chunk_type);

        if matches!(
            &chunk_type,
            b"cHRM"
                | b"cICP"
                | b"gAMA"
                | b"iCCP"
                | b"mDCV"
                | b"cLLI"
                | b"sBIT"
                | b"sRGB"
                | b"bKGD"
                | b"hIST"
                | b"tRNS"
                | b"eXIf"
                | b"pHYs"
                | b"tIME"
        ) && !seen_singleton_ancillary_chunks.insert(chunk_type)
        {
            return Err(malformed_png(format!(
                "duplicate {} chunk",
                String::from_utf8_lossy(&chunk_type)
            )));
        }

        if matches!(
            &chunk_type,
            b"cHRM" | b"cICP" | b"gAMA" | b"iCCP" | b"mDCV" | b"cLLI" | b"sBIT" | b"sRGB"
        ) && (saw_plte || saw_image_data)
        {
            return Err(malformed_png(format!(
                "{} must precede PLTE and IDAT",
                String::from_utf8_lossy(&chunk_type)
            )));
        }

        if matches!(&chunk_type, b"eXIf" | b"pHYs" | b"sPLT") && saw_image_data {
            return Err(malformed_png(format!(
                "{} must precede IDAT",
                String::from_utf8_lossy(&chunk_type)
            )));
        }

        if matches!(&chunk_type, b"bKGD" | b"hIST" | b"tRNS") {
            if saw_image_data {
                return Err(malformed_png(format!(
                    "{} must precede IDAT",
                    String::from_utf8_lossy(&chunk_type)
                )));
            }
            if &chunk_type == b"hIST" && !saw_plte {
                return Err(malformed_png("hIST requires a preceding PLTE chunk"));
            }
            saw_palette_follower = true;
        }

        if &chunk_type == b"PLTE" {
            if saw_plte {
                return Err(malformed_png("duplicate PLTE chunk"));
            }
            if saw_image_data {
                return Err(malformed_png("PLTE must precede IDAT"));
            }
            if saw_palette_follower {
                return Err(malformed_png("PLTE must precede bKGD, hIST, and tRNS"));
            }
            if length == 0 || length % 3 != 0 || length > 768 {
                return Err(malformed_png("PLTE length is invalid"));
            }
            if matches!(color_type, Some(0) | Some(4)) {
                return Err(malformed_png("PLTE is forbidden for grayscale PNG"));
            }
            if color_type == Some(3) {
                let max_entries = 1u32
                    .checked_shl(u32::from(bit_depth.unwrap_or_default()))
                    .unwrap_or(0);
                if length / 3 > max_entries {
                    return Err(malformed_png(
                        "PLTE has more entries than indexed bit depth permits",
                    ));
                }
            }
            saw_plte = true;
        }

        match &chunk_type {
            b"IHDR" => {
                if saw_ihdr {
                    return Err(malformed_png("duplicate IHDR chunk"));
                }
                if length != 13 {
                    return Err(malformed_png("IHDR length must be 13"));
                }
                let mut data = [0u8; 13];
                read_png_exact(reader, &mut data, "IHDR payload")?;
                crc.update(&data);
                let parsed_width = u32::from_be_bytes(data[0..4].try_into().unwrap());
                let parsed_height = u32::from_be_bytes(data[4..8].try_into().unwrap());
                if parsed_width == 0 || parsed_height == 0 {
                    return Err(malformed_png("IHDR dimensions must be non-zero"));
                }
                checked_rgba_bytes(parsed_width, parsed_height, limits)?;
                let parsed_bit_depth = data[8];
                let parsed_color_type = data[9];
                let valid_bit_depth = match parsed_color_type {
                    0 => matches!(parsed_bit_depth, 1 | 2 | 4 | 8 | 16),
                    2 | 4 | 6 => matches!(parsed_bit_depth, 8 | 16),
                    3 => matches!(parsed_bit_depth, 1 | 2 | 4 | 8),
                    _ => false,
                };
                if !valid_bit_depth || data[10] != 0 || data[11] != 0 || data[12] > 1 {
                    return Err(malformed_png("IHDR control fields are invalid"));
                }
                width = Some(parsed_width);
                height = Some(parsed_height);
                bit_depth = Some(parsed_bit_depth);
                color_type = Some(parsed_color_type);
                saw_ihdr = true;
            }
            b"acTL" => {
                if saw_animation_control {
                    return Err(malformed_png("duplicate acTL chunk"));
                }
                if saw_image_data {
                    return Err(malformed_png("acTL must precede IDAT"));
                }
                if length != 8 {
                    return Err(malformed_png("acTL length must be 8"));
                }
                let mut data = [0u8; 8];
                read_png_exact(reader, &mut data, "acTL payload")?;
                crc.update(&data);
                saw_animation_control = true;
                let parsed_frame_count = u32::from_be_bytes(data[0..4].try_into().unwrap());
                let parsed_play_count = u32::from_be_bytes(data[4..8].try_into().unwrap());
                if parsed_frame_count == 0 {
                    return Err(malformed_png("acTL frame count must be non-zero"));
                }
                if parsed_frame_count > limits.max_frame_count {
                    return Err(PipelineError::LimitExceeded {
                        resource: "frame-count",
                        limit: u64::from(limits.max_frame_count),
                        actual: u64::from(parsed_frame_count),
                    });
                }
                if parsed_play_count > i32::MAX as u32 {
                    return Err(malformed_png("acTL play count exceeds PNG integer range"));
                }
                frame_count = Some(u64::from(parsed_frame_count));
            }
            b"fcTL" => {
                if pending_frame_data.is_some() {
                    return Err(malformed_png("fcTL was not followed by frame data"));
                }
                if length != 26 {
                    return Err(malformed_png("fcTL length must be 26"));
                }
                let mut data = [0u8; 26];
                read_png_exact(reader, &mut data, "fcTL payload")?;
                crc.update(&data);
                let sequence = u32::from_be_bytes(data[0..4].try_into().unwrap());
                if sequence != expected_apng_sequence {
                    return Err(malformed_png("APNG sequence number is out of order"));
                }
                expected_apng_sequence = expected_apng_sequence
                    .checked_add(1)
                    .ok_or_else(|| malformed_png("APNG sequence number overflow"))?;
                let frame_width = u32::from_be_bytes(data[4..8].try_into().unwrap());
                let frame_height = u32::from_be_bytes(data[8..12].try_into().unwrap());
                let frame_x = u32::from_be_bytes(data[12..16].try_into().unwrap());
                let frame_y = u32::from_be_bytes(data[16..20].try_into().unwrap());
                let canvas_width = width.ok_or_else(|| malformed_png("missing IHDR width"))?;
                let canvas_height = height.ok_or_else(|| malformed_png("missing IHDR height"))?;
                let frame_right = frame_x
                    .checked_add(frame_width)
                    .ok_or_else(|| malformed_png("fcTL horizontal rectangle overflow"))?;
                let frame_bottom = frame_y
                    .checked_add(frame_height)
                    .ok_or_else(|| malformed_png("fcTL vertical rectangle overflow"))?;
                if frame_width == 0
                    || frame_height == 0
                    || frame_right > canvas_width
                    || frame_bottom > canvas_height
                {
                    return Err(malformed_png("fcTL frame rectangle is outside the canvas"));
                }
                if !saw_image_data
                    && frame_control_count == 0
                    && (frame_width != canvas_width
                        || frame_height != canvas_height
                        || frame_x != 0
                        || frame_y != 0)
                {
                    return Err(malformed_png(
                        "the first animated IDAT frame must cover the full canvas",
                    ));
                }
                if data[24] > 2 || data[25] > 1 {
                    return Err(malformed_png("fcTL dispose or blend operation is invalid"));
                }
                let next_frame_control_count = frame_control_count.saturating_add(1);
                if next_frame_control_count > u64::from(limits.max_frame_count) {
                    return Err(PipelineError::LimitExceeded {
                        resource: "frame-count",
                        limit: u64::from(limits.max_frame_count),
                        actual: next_frame_control_count,
                    });
                }
                frame_control_count = next_frame_control_count;
                let delay_num = u16::from_be_bytes([data[20], data[21]]);
                let delay_den = u16::from_be_bytes([data[22], data[23]]);
                let denominator = if delay_den == 0 { 100 } else { delay_den };
                let delay = if delay_num == 0 {
                    0.01
                } else {
                    f64::from(delay_num) / f64::from(denominator)
                };
                frame_durations.push(delay);
                pending_frame_data = Some(if saw_image_data { "fdAT" } else { "IDAT" });
                active_fdat_frame = false;
            }
            b"IDAT" => {
                if color_type == Some(3) && !saw_plte {
                    return Err(malformed_png("indexed PNG requires PLTE before IDAT"));
                }
                if saw_fdat {
                    return Err(malformed_png("IDAT cannot follow fdAT"));
                }
                if saw_image_data && !last_chunk_was_idat {
                    return Err(malformed_png("IDAT chunks must be consecutive"));
                }
                if pending_frame_data == Some("fdAT") {
                    return Err(malformed_png("frame requires fdAT data"));
                }
                if pending_frame_data == Some("IDAT") && length > 0 {
                    pending_frame_data = None;
                }
                saw_image_data = true;
                if length > 0 {
                    saw_nonempty_idat = true;
                }
                let mut remaining = usize::try_from(length)
                    .map_err(|_| malformed_png("IDAT length is not addressable"))?;
                let mut scratch = [0u8; SCRATCH_BYTES];
                while remaining > 0 {
                    checkpoint()?;
                    let count = remaining.min(scratch.len());
                    read_png_exact(reader, &mut scratch[..count], "IDAT payload")?;
                    crc.update(&scratch[..count]);
                    remaining -= count;
                }
            }
            b"fdAT" => {
                if !saw_animation_control {
                    return Err(malformed_png("fdAT requires a preceding acTL chunk"));
                }
                if !saw_image_data {
                    return Err(malformed_png("fdAT cannot precede IDAT"));
                }
                if length < 4 {
                    return Err(malformed_png("fdAT length must include a sequence number"));
                }
                if pending_frame_data == Some("IDAT") {
                    return Err(malformed_png("frame requires IDAT data"));
                }
                if pending_frame_data != Some("fdAT") && !active_fdat_frame {
                    return Err(malformed_png("fdAT requires a preceding fcTL chunk"));
                }
                let mut sequence_bytes = [0u8; 4];
                read_png_exact(reader, &mut sequence_bytes, "fdAT sequence number")?;
                crc.update(&sequence_bytes);
                let sequence = u32::from_be_bytes(sequence_bytes);
                if sequence != expected_apng_sequence {
                    return Err(malformed_png("APNG sequence number is out of order"));
                }
                expected_apng_sequence = expected_apng_sequence
                    .checked_add(1)
                    .ok_or_else(|| malformed_png("APNG sequence number overflow"))?;
                let mut remaining = usize::try_from(length - 4)
                    .map_err(|_| malformed_png("fdAT length is not addressable"))?;
                let mut scratch = [0u8; SCRATCH_BYTES];
                while remaining > 0 {
                    checkpoint()?;
                    let count = remaining.min(scratch.len());
                    read_png_exact(reader, &mut scratch[..count], "fdAT payload")?;
                    crc.update(&scratch[..count]);
                    remaining -= count;
                }
                if length > 4 {
                    pending_frame_data = None;
                }
                active_fdat_frame = true;
                saw_fdat = true;
            }
            b"IEND" => {
                if length != 0 {
                    return Err(malformed_png("IEND length must be zero"));
                }
                if !saw_image_data || !saw_nonempty_idat {
                    return Err(malformed_png("IEND requires non-empty IDAT data"));
                }
                if pending_frame_data.is_some() {
                    return Err(malformed_png("fcTL was not followed by frame data"));
                }
                saw_iend = true;
            }
            _ => {
                let mut remaining = usize::try_from(length)
                    .map_err(|_| malformed_png("PNG chunk length is not addressable"))?;
                let mut scratch = [0u8; SCRATCH_BYTES];
                while remaining > 0 {
                    checkpoint()?;
                    let count = remaining.min(scratch.len());
                    read_png_exact(reader, &mut scratch[..count], "PNG chunk payload")?;
                    crc.update(&scratch[..count]);
                    remaining -= count;
                }
            }
        }

        let mut stored_crc = [0u8; 4];
        read_png_exact(reader, &mut stored_crc, "PNG chunk CRC")?;
        let expected_crc = u32::from_be_bytes(stored_crc);
        let actual_crc = crc.finalize();
        if actual_crc != expected_crc {
            return Err(malformed_png(format!(
                "CRC mismatch for {} chunk",
                String::from_utf8_lossy(&chunk_type)
            )));
        }
        offset = offset
            .checked_add(payload_and_crc)
            .ok_or_else(|| malformed_png("PNG offset overflow"))?;
        last_chunk_was_idat = &chunk_type == b"IDAT";
    }

    if offset < file_length {
        warnings.push("png-trailing-bytes".into());
    }
    if !saw_animation_control && frame_control_count > 0 {
        return Err(malformed_png("fcTL requires an acTL chunk before IDAT"));
    }
    if !saw_animation_control {
        return Ok(PngFileMetadata {
            animation: None,
            warnings,
        });
    }
    if frame_count != Some(frame_control_count) {
        return Err(malformed_png("fcTL count does not match acTL frame count"));
    }

    Ok(PngFileMetadata {
        animation: Some(PngAnimationMetadata {
            width: width.ok_or_else(|| malformed_png("missing IHDR width"))?,
            height: height.ok_or_else(|| malformed_png("missing IHDR height"))?,
            frame_count,
            frame_durations,
            warnings: warnings.clone(),
        }),
        warnings,
    })
}

fn read_png_metadata(
    input_path: &Path,
    limits: MediaLimits,
    checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<PngFileMetadata, PipelineError> {
    run_decoder_boundary("png", || {
        let mut file = File::open(input_path).map_err(|error| PipelineError::Io {
            operation: "open PNG input",
            message: error.to_string(),
        })?;
        let metadata = file.metadata().map_err(|error| PipelineError::Io {
            operation: "read PNG input metadata",
            message: error.to_string(),
        })?;
        let file_length = validate_file_metadata(metadata, limits)?.len();
        let mut reader = BufReader::new(&mut file);
        read_png_metadata_from_reader(&mut reader, file_length, limits, checkpoint)
    })
}

fn read_png_animation_metadata(
    input_path: &Path,
    limits: MediaLimits,
    checkpoint: impl FnMut() -> Result<(), PipelineError>,
) -> Result<Option<PngAnimationMetadata>, PipelineError> {
    read_png_metadata(input_path, limits, checkpoint).map(|metadata| metadata.animation)
}

fn inspect_still_image_metadata_internal(input_path: &str, locale: UiLocale) -> MediaInspection {
    inspect_still_image_metadata_with_checkpoint(input_path, locale, &mut || Ok(()))
}

fn inspect_still_image_metadata_with_checkpoint(
    input_path: &str,
    locale: UiLocale,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> MediaInspection {
    if let Err(error) = checkpoint() {
        return inspection_pipeline_error(input_path, &error, locale);
    }
    let limits = MediaLimits::default();
    let (reader, metadata) = match open_limited_image_reader(input_path, limits) {
        Ok(result) => result,
        Err(error) => {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_image_detail(locale)),
                error.code(),
                pipeline_error_diagnostic(&error, locale),
            );
        }
    };

    let dimensions = run_decoder_boundary("image", || {
        checkpoint()?;
        let decoder = reader
            .into_decoder()
            .map_err(|error| image_decoder_error("image", error, limits))?;
        let dimensions = decoder.dimensions();
        checked_rgba_bytes(dimensions.0, dimensions.1, limits)?;
        checkpoint()?;
        Ok(dimensions)
    });
    let (width, height) = match dimensions {
        Ok(dimensions) => dimensions,
        Err(error) => {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_image_detail(locale)),
                error.code(),
                pipeline_error_diagnostic(&error, locale),
            );
        }
    };

    let format_name = lowercase_source_extension(input_path);

    MediaInspection {
        ok: true,
        input_path: input_path.to_string(),
        source_revision: None,
        tool_source: Some("native".into()),
        tool_command: None,
        tool_detail: Some(locale::native_image_detail(locale)),
        fallback_reason_code: None,
        format_name,
        duration_seconds: None,
        size_bytes: Some(metadata.len()),
        width: Some(width),
        height: Some(height),
        codec_name: None,
        pixel_format: None,
        avg_fps: None,
        frame_rate_label: None,
        estimated_frames: None,
        frame_durations_seconds: None,
        warnings: Vec::new(),
        is_static_image: true,
        can_convert_to_png: true,
        reason_code: None,
        error_code: None,
        error_message: None,
    }
}

fn inspect_gif_metadata_internal(input_path: &str, locale: UiLocale) -> MediaInspection {
    inspect_gif_metadata_with_checkpoint(input_path, locale, &mut || Ok(()))
}

fn inspect_gif_metadata_with_checkpoint(
    input_path: &str,
    locale: UiLocale,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> MediaInspection {
    if let Err(error) = checkpoint() {
        return inspection_pipeline_error(input_path, &error, locale);
    }
    let metadata = match fs::metadata(input_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            let error = pipeline_io_error("read GIF input metadata", error);
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_animation_detail(locale, "gif")),
                error.code(),
                pipeline_error_diagnostic(&error, locale),
            );
        }
    };

    let frames =
        match decode_gif_animation_frames(input_path, MediaLimits::default(), || checkpoint()) {
            Ok(frames) => frames,
            Err(error) => {
                return inspection_error(
                    input_path,
                    Some("native".into()),
                    None,
                    Some(locale::native_animation_detail(locale, "gif")),
                    error.code(),
                    pipeline_error_diagnostic(&error, locale),
                );
            }
        };

    let width = frames
        .first()
        .map(|frame| frame.pixels.width())
        .unwrap_or(0);
    let height = frames
        .first()
        .map(|frame| frame.pixels.height())
        .unwrap_or(0);
    let total_duration_us = match sticker_frame_duration_us(&frames) {
        Ok(duration_us) => duration_us,
        Err(error) => {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_animation_detail(locale, "gif")),
                "inspect-failed",
                error.into(),
            );
        }
    };
    let frame_durations = frames
        .iter()
        .map(|frame| duration_us_to_seconds(frame.duration_us))
        .collect::<Vec<_>>();
    let estimated_frames = frame_durations.len() as u64;
    let duration_seconds = duration_us_to_seconds(total_duration_us);
    let avg_fps = if duration_seconds > 0.0 {
        Some(estimated_frames as f64 / duration_seconds)
    } else {
        None
    };

    MediaInspection {
        ok: true,
        input_path: input_path.to_string(),
        source_revision: None,
        tool_source: Some("native".into()),
        tool_command: None,
        tool_detail: Some(locale::native_animation_detail(locale, "gif")),
        fallback_reason_code: None,
        format_name: Some("gif".into()),
        duration_seconds: Some(duration_seconds),
        size_bytes: Some(metadata.len()),
        width: Some(width),
        height: Some(height),
        codec_name: Some("gif".into()),
        pixel_format: None,
        avg_fps,
        frame_rate_label: avg_fps.map(|fps| format!("{fps:.2}")),
        estimated_frames: Some(estimated_frames),
        frame_durations_seconds: Some(frame_durations),
        warnings: Vec::new(),
        is_static_image: false,
        can_convert_to_png: false,
        reason_code: None,
        error_code: None,
        error_message: None,
    }
}

fn inspect_apng_metadata_internal(
    input_path: &str,
    locale: UiLocale,
    preparsed_metadata: Option<PngAnimationMetadata>,
) -> MediaInspection {
    inspect_apng_metadata_with_checkpoint(input_path, locale, preparsed_metadata, &mut || Ok(()))
}

fn inspect_apng_metadata_with_checkpoint(
    input_path: &str,
    locale: UiLocale,
    preparsed_metadata: Option<PngAnimationMetadata>,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> MediaInspection {
    if let Err(error) = checkpoint() {
        return inspection_pipeline_error(input_path, &error, locale);
    }
    let metadata = match fs::metadata(input_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            let error = pipeline_io_error("read APNG input metadata", error);
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_animation_detail(locale, "apng")),
                error.code(),
                pipeline_error_diagnostic(&error, locale),
            );
        }
    };

    let animation_metadata = match preparsed_metadata {
        Some(metadata) => metadata,
        None => {
            match read_png_animation_metadata(Path::new(input_path), MediaLimits::default(), || {
                checkpoint()
            }) {
                Ok(Some(metadata)) => metadata,
                Ok(None) => {
                    return inspection_error(
                        input_path,
                        Some("native".into()),
                        None,
                        Some(locale::native_animation_detail(locale, "apng")),
                        "malformed-media",
                        locale::invalid_apng_error(locale),
                    );
                }
                Err(error) => {
                    return inspection_error(
                        input_path,
                        Some("native".into()),
                        None,
                        Some(locale::native_animation_detail(locale, "apng")),
                        error.code(),
                        pipeline_error_diagnostic(&error, locale),
                    );
                }
            }
        }
    };

    let width = animation_metadata.width;
    let height = animation_metadata.height;
    if width == 0 {
        return inspection_error(
            input_path,
            Some("native".into()),
            None,
            Some(locale::native_animation_detail(locale, "apng")),
            "inspect-failed",
            locale::invalid_png_header_error(locale),
        );
    }
    if height == 0 {
        return inspection_error(
            input_path,
            Some("native".into()),
            None,
            Some(locale::native_animation_detail(locale, "apng")),
            "inspect-failed",
            locale::invalid_png_header_error(locale),
        );
    }

    if animation_metadata.frame_durations.is_empty() {
        return inspection_error(
            input_path,
            Some("native".into()),
            None,
            Some(locale::native_animation_detail(locale, "apng")),
            "inspect-failed",
            locale::invalid_apng_error(locale),
        );
    }

    let estimated_frames = animation_metadata
        .frame_count
        .unwrap_or(animation_metadata.frame_durations.len() as u64);
    let duration_seconds = animation_metadata.frame_durations.iter().sum::<f64>();
    let avg_fps = if duration_seconds > 0.0 {
        Some(estimated_frames as f64 / duration_seconds)
    } else {
        None
    };

    let tool_detail = if animation_metadata.warnings.is_empty() {
        locale::native_animation_detail(locale, "apng")
    } else {
        format!(
            "{} ({})",
            locale::native_animation_detail(locale, "apng"),
            animation_metadata.warnings.join(", ")
        )
    };

    MediaInspection {
        ok: true,
        input_path: input_path.to_string(),
        source_revision: None,
        tool_source: Some("native".into()),
        tool_command: None,
        tool_detail: Some(tool_detail),
        fallback_reason_code: None,
        format_name: Some("apng".into()),
        duration_seconds: Some(duration_seconds),
        size_bytes: Some(metadata.len()),
        width: Some(width),
        height: Some(height),
        codec_name: Some("apng".into()),
        pixel_format: None,
        avg_fps,
        frame_rate_label: avg_fps.map(|fps| format!("{fps:.2}")),
        estimated_frames: Some(estimated_frames),
        frame_durations_seconds: Some(animation_metadata.frame_durations),
        warnings: animation_metadata.warnings,
        is_static_image: false,
        can_convert_to_png: false,
        reason_code: None,
        error_code: None,
        error_message: None,
    }
}

fn parse_duration_hms_to_seconds(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.trim().parse::<f64>().ok()?;
    let minutes = parts.next()?.trim().parse::<f64>().ok()?;
    let seconds = parts.next()?.trim().parse::<f64>().ok()?;
    Some((hours * 3600.0) + (minutes * 60.0) + seconds)
}

fn should_try_media_foundation(extension: &str, is_windows: bool) -> bool {
    is_windows && matches!(extension, "mp4" | "m4v" | "mov")
}

fn inspect_mp4_family_with_fallback<N, F>(
    native: N,
    ffmpeg: F,
) -> Result<MediaInspection, PipelineError>
where
    N: FnOnce() -> Result<MediaInspection, PipelineError>,
    F: FnOnce(PipelineError) -> Result<MediaInspection, PipelineError>,
{
    match native() {
        Ok(inspection) => Ok(inspection),
        Err(native_error) => ffmpeg(native_error),
    }
}

#[cfg(target_os = "windows")]
fn try_inspect_mp4_family_with_media_foundation(
    input_path: &str,
    locale: UiLocale,
) -> Result<MediaInspection, PipelineError> {
    let format_name = lowercase_source_extension(input_path).unwrap_or_else(|| "video".into());
    unsafe {
        let mut should_uninitialize_com = false;
        let com_result = CoInitializeEx(None, COINIT_MULTITHREADED);
        if com_result.is_ok() {
            should_uninitialize_com = true;
        } else if com_result != RPC_E_CHANGED_MODE {
            return Err(pipeline_io_error(
                "initialize Media Foundation COM",
                com_result,
            ));
        }

        let mut media_foundation_started = false;
        let inspection = (|| -> Result<MediaInspection, PipelineError> {
            MFStartup(MF_VERSION, MFSTARTUP_FULL)
                .map_err(|error| pipeline_io_error("start Media Foundation", error))?;
            media_foundation_started = true;

            let wide_path: Vec<u16> = Path::new(input_path)
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let reader = MFCreateSourceReaderFromURL(PCWSTR(wide_path.as_ptr()), None)
                .map_err(|error| pipeline_io_error("open Media Foundation source", error))?;
            let media_type = reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                .map_err(|error| pipeline_io_error("read Media Foundation video type", error))?;

            let frame_size = media_type
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map_err(|error| pipeline_io_error("read Media Foundation frame size", error))?;
            let (width, height) = unpack_media_foundation_pair(frame_size);

            let avg_fps = media_type
                .GetUINT64(&MF_MT_FRAME_RATE)
                .ok()
                .and_then(|packed_rate| {
                    let (numerator, denominator) = unpack_media_foundation_pair(packed_rate);
                    if numerator > 0 && denominator > 0 {
                        Some(numerator as f64 / denominator as f64)
                    } else {
                        None
                    }
                });

            let duration_value = reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .map_err(|error| pipeline_io_error("read Media Foundation duration", error))?;
            let duration_100ns = u64::try_from(&duration_value)
                .map_err(|error| pipeline_io_error("decode Media Foundation duration", error))?;
            let duration_seconds =
                (duration_100ns > 0).then_some(duration_100ns as f64 / 10_000_000.0);
            let estimated_frames = duration_seconds
                .zip(avg_fps)
                .map(|(duration, fps)| (duration * fps).round().max(1.0) as u64);
            let size_bytes = fs::metadata(input_path).ok().map(|metadata| metadata.len());

            Ok(MediaInspection {
                ok: true,
                input_path: input_path.to_string(),
                source_revision: None,
                tool_source: Some("native".into()),
                tool_command: None,
                tool_detail: Some(locale::native_video_detail(locale, &format_name)),
                fallback_reason_code: None,
                format_name: Some(format_name.clone()),
                duration_seconds,
                size_bytes,
                width: Some(width),
                height: Some(height),
                codec_name: None,
                pixel_format: None,
                avg_fps,
                frame_rate_label: avg_fps.map(|fps| format!("{fps:.2}")),
                estimated_frames,
                frame_durations_seconds: None,
                warnings: Vec::new(),
                is_static_image: false,
                can_convert_to_png: false,
                reason_code: None,
                error_code: None,
                error_message: None,
            })
        })();

        if media_foundation_started {
            let _ = MFShutdown();
        }
        if should_uninitialize_com {
            CoUninitialize();
        }

        inspection
    }
}

#[cfg(target_os = "windows")]
fn inspect_mp4_family_with_fallback_and_checkpoint(
    input_path: &str,
    locale: UiLocale,
    context: Option<&OperationContext>,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> MediaInspection {
    if let Err(error) = checkpoint() {
        return inspection_pipeline_error(input_path, &error, locale);
    }

    let inspection = match inspect_mp4_family_with_fallback(
        || try_inspect_mp4_family_with_media_foundation(input_path, locale),
        |_native_error| {
            checkpoint()?;
            let fallback = try_inspect_video_with_ffmpeg_and_checkpoint(
                input_path, locale, context, checkpoint,
            )?;
            Ok(annotate_media_foundation_fallback(fallback, locale))
        },
    ) {
        Ok(inspection) => inspection,
        Err(error) => return inspection_pipeline_error(input_path, &error, locale),
    };

    if let Err(error) = checkpoint() {
        return inspection_pipeline_error_with_provenance(input_path, &inspection, &error, locale);
    }
    inspection
}

fn finish_ffmpeg_inspection_result(
    input_path: &str,
    locale: UiLocale,
    tool: ToolResolution,
    output_result: Result<CapturedProcess, PipelineError>,
) -> MediaInspection {
    let output = match output_result {
        Ok(output) => output,
        Err(error) => {
            return inspection_error(
                input_path,
                Some(tool.source.into()),
                Some(tool.command_display.clone()),
                tool.fallback_reason.clone(),
                error.code(),
                pipeline_error_diagnostic(&error, locale),
            );
        }
    };

    static FORMAT_REGEX: OnceLock<Regex> = OnceLock::new();
    static DURATION_REGEX: OnceLock<Regex> = OnceLock::new();
    static STREAM_REGEX: OnceLock<Regex> = OnceLock::new();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let format_regex = FORMAT_REGEX
        .get_or_init(|| Regex::new(r"Input #0, ([^,]+(?:,[^,]+)*), from").expect("format regex"));
    let duration_regex = DURATION_REGEX
        .get_or_init(|| Regex::new(r"Duration:\s*([0-9:.]+)").expect("duration regex"));
    let stream_regex = STREAM_REGEX.get_or_init(|| {
        Regex::new(r"Video:\s*([^,]+),\s*(.+?),\s*(\d+)x(\d+).*?([0-9]+(?:\.[0-9]+)?)\s+fps")
            .expect("stream regex")
    });

    let format_name = format_regex
        .captures(&stderr)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().trim().to_string())
        .or_else(|| lowercase_source_extension(input_path));
    let duration_seconds = duration_regex
        .captures(&stderr)
        .and_then(|captures| captures.get(1))
        .and_then(|value| parse_duration_hms_to_seconds(value.as_str()));
    let stream_captures = match stream_regex.captures(&stderr) {
        Some(captures) => captures,
        None => {
            let message = ffmpeg_inspection_failure_diagnostic(locale, true);

            return inspection_error(
                input_path,
                Some(tool.source.into()),
                Some(tool.command_display.clone()),
                tool.fallback_reason.clone(),
                "inspect-failed",
                message,
            );
        }
    };

    let codec_name = stream_captures
        .get(1)
        .map(|value| value.as_str().trim().to_string());
    let pixel_format = stream_captures
        .get(2)
        .map(|value| value.as_str().trim().to_string());
    let width = stream_captures
        .get(3)
        .and_then(|value| value.as_str().parse::<u32>().ok());
    let height = stream_captures
        .get(4)
        .and_then(|value| value.as_str().parse::<u32>().ok());
    let avg_fps = stream_captures
        .get(5)
        .and_then(|value| value.as_str().parse::<f64>().ok());
    let estimated_frames = duration_seconds
        .zip(avg_fps)
        .map(|(duration, fps)| (duration * fps).round().max(1.0) as u64);
    let size_bytes = fs::metadata(input_path).ok().map(|metadata| metadata.len());

    MediaInspection {
        ok: true,
        input_path: input_path.to_string(),
        source_revision: None,
        tool_source: Some(tool.source.into()),
        tool_command: Some(tool.command_display),
        tool_detail: tool.fallback_reason,
        fallback_reason_code: None,
        format_name,
        duration_seconds,
        size_bytes,
        width,
        height,
        codec_name,
        pixel_format,
        avg_fps,
        frame_rate_label: avg_fps.map(|fps| format!("{fps:.2}")),
        estimated_frames,
        frame_durations_seconds: None,
        warnings: Vec::new(),
        is_static_image: false,
        can_convert_to_png: false,
        reason_code: None,
        error_code: None,
        error_message: None,
    }
}

fn inspect_video_with_ffmpeg(input_path: &str, locale: UiLocale) -> MediaInspection {
    inspect_video_with_ffmpeg_and_checkpoint(input_path, locale, None, &mut || Ok(()))
}

fn try_inspect_video_with_ffmpeg_and_checkpoint(
    input_path: &str,
    locale: UiLocale,
    context: Option<&OperationContext>,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> Result<MediaInspection, PipelineError> {
    checkpoint()?;
    let tool = match resolve_tool("ffmpeg", locale) {
        Ok(tool) => tool,
        Err(_) => {
            return Ok(inspection_error(
                input_path,
                Some("missing".into()),
                None,
                None,
                "tool-unavailable",
                locale::media_pipeline_diagnostic(locale, "tool-missing"),
            ));
        }
    };

    let args = [
        OsString::from("-hide_banner"),
        OsString::from("-i"),
        OsString::from(input_path),
        OsString::from("-map"),
        OsString::from("0:v:0"),
        OsString::from("-frames:v"),
        OsString::from("1"),
        OsString::from("-f"),
        OsString::from("null"),
        OsString::from("-"),
    ];
    let detached = context
        .is_none()
        .then(|| OperationContext::detached(TOOL_PROCESS_TIMEOUT));
    let process_context =
        context.unwrap_or_else(|| detached.as_ref().expect("detached inspection context"));
    checkpoint()?;
    let output_result = run_captured(
        &tool.command,
        &args,
        ProcessLimits {
            timeout: TOOL_PROCESS_TIMEOUT,
            max_stdout_bytes: PROCESS_CAPTURE_LIMIT_BYTES,
            max_stderr_bytes: PROCESS_CAPTURE_LIMIT_BYTES,
        },
        process_context,
    );
    checkpoint()?;
    Ok(finish_ffmpeg_inspection_result(
        input_path,
        locale,
        tool,
        output_result,
    ))
}

fn inspect_video_with_ffmpeg_and_checkpoint(
    input_path: &str,
    locale: UiLocale,
    context: Option<&OperationContext>,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> MediaInspection {
    match try_inspect_video_with_ffmpeg_and_checkpoint(input_path, locale, context, checkpoint) {
        Ok(inspection) => inspection,
        Err(error) => inspection_pipeline_error(input_path, &error, locale),
    }
}

fn inspect_input_media_canonical_internal(input_path: &str, locale: UiLocale) -> MediaInspection {
    inspect_input_media_canonical_with_checkpoint(input_path, locale, None, &mut || Ok(()))
}

fn inspect_input_media_canonical_with_checkpoint(
    input_path: &str,
    locale: UiLocale,
    context: Option<&OperationContext>,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> MediaInspection {
    if let Err(error) = checkpoint() {
        return inspection_pipeline_error(input_path, &error, locale);
    }
    let Some(extension) = lowercase_source_extension(input_path) else {
        return inspection_error(
            input_path,
            None,
            None,
            None,
            "unsupported-source-format",
            locale::unsupported_still_image_error(locale),
        );
    };

    if extension == "png" {
        match read_png_metadata(Path::new(input_path), MediaLimits::default(), || {
            checkpoint()
        }) {
            Ok(metadata) => {
                if let Some(animation) = metadata.animation {
                    return inspect_apng_metadata_with_checkpoint(
                        input_path,
                        locale,
                        Some(animation),
                        checkpoint,
                    );
                }
                let mut inspection =
                    inspect_still_image_metadata_with_checkpoint(input_path, locale, checkpoint);
                inspection.warnings = metadata.warnings;
                return inspection;
            }
            Err(error) => {
                return inspection_error(
                    input_path,
                    Some("native".into()),
                    None,
                    Some(locale::native_image_detail(locale)),
                    error.code(),
                    pipeline_error_diagnostic(&error, locale),
                );
            }
        }
    }

    if is_supported_static_image_extension(&extension) {
        return inspect_still_image_metadata_with_checkpoint(input_path, locale, checkpoint);
    }

    if extension == "gif" {
        return inspect_gif_metadata_with_checkpoint(input_path, locale, checkpoint);
    }

    if extension == "apng" {
        return inspect_apng_metadata_with_checkpoint(input_path, locale, None, checkpoint);
    }

    #[cfg(target_os = "windows")]
    if should_try_media_foundation(&extension, true) {
        return inspect_mp4_family_with_fallback_and_checkpoint(
            input_path, locale, context, checkpoint,
        );
    }

    if is_supported_video_extension(&extension) {
        return inspect_video_with_ffmpeg_and_checkpoint(input_path, locale, context, checkpoint);
    }

    inspection_error(
        input_path,
        None,
        None,
        None,
        "unsupported-source-format",
        locale::unsupported_still_image_error(locale),
    )
}

fn inspect_input_media_internal(input_path: &str, locale: UiLocale) -> MediaInspection {
    inspect_input_media_with_callbacks(input_path, locale, None, || Ok(()), |_, _, _| {})
}

fn inspect_input_media_with_operation(
    input_path: &str,
    preflight: &SourcePreflight,
    locale: UiLocale,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> MediaInspection {
    if let Err(error) = context.checkpoint() {
        return inspection_pipeline_error(input_path, &error, locale);
    }
    publish_progress(progress, context, ProgressStage::Inspecting, 0, Some(1));
    let mut checkpoint = || context.checkpoint();
    let inspection = inspect_input_media_preflighted_with_callbacks(
        input_path,
        preflight,
        locale,
        Some(context),
        &mut checkpoint,
    );
    let tool_source = inspection.tool_source.clone();
    let tool_command = inspection.tool_command.clone();
    let tool_detail = inspection.tool_detail.clone();
    let fallback_reason_code = inspection.fallback_reason_code.clone();
    match finalize_source_publication(context, preflight.identity(), || {
        publish_progress(progress, context, ProgressStage::Finalizing, 1, Some(1));
        Ok(inspection)
    }) {
        Ok(inspection) => inspection,
        Err(error) => inspection_error_with_fallback_reason(
            input_path,
            tool_source,
            tool_command,
            tool_detail,
            fallback_reason_code,
            error.code(),
            pipeline_error_diagnostic(&error, locale),
        ),
    }
}

fn inspect_input_media_with_callbacks(
    input_path: &str,
    locale: UiLocale,
    context: Option<&OperationContext>,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
    mut progress: impl FnMut(ProgressStage, u32, Option<u32>),
) -> MediaInspection {
    if let Err(error) = checkpoint() {
        return inspection_pipeline_error(input_path, &error, locale);
    }
    let limits = MediaLimits::default();
    let preflight = match SourcePreflight::capture(Path::new(input_path), limits) {
        Ok(preflight) => preflight,
        Err(error) => {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                None,
                error.code(),
                pipeline_error_diagnostic(&error, locale),
            );
        }
    };
    let inspection = inspect_input_media_preflighted_with_callbacks(
        input_path,
        &preflight,
        locale,
        context,
        &mut checkpoint,
    );
    let tool_source = inspection.tool_source.clone();
    let tool_command = inspection.tool_command.clone();
    let tool_detail = inspection.tool_detail.clone();
    let fallback_reason_code = inspection.fallback_reason_code.clone();
    progress(ProgressStage::Finalizing, 1, Some(1));
    if let Err(error) = checkpoint() {
        return inspection_pipeline_error_with_provenance(input_path, &inspection, &error, locale);
    }
    finalize_source_checked(preflight.identity(), inspection, limits, |error| {
        inspection_error_with_fallback_reason(
            input_path,
            tool_source,
            tool_command,
            tool_detail,
            fallback_reason_code,
            error.code(),
            pipeline_error_diagnostic(&error, locale),
        )
    })
}

fn inspect_input_media_preflighted_with_callbacks(
    input_path: &str,
    preflight: &SourcePreflight,
    locale: UiLocale,
    context: Option<&OperationContext>,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> MediaInspection {
    let canonical_path = preflight.canonical_path().to_string_lossy().into_owned();
    let inspection =
        inspect_input_media_canonical_with_checkpoint(&canonical_path, locale, context, checkpoint);
    let inspection = apply_inspection_source_revision(preflight.identity(), input_path, inspection);
    enforce_desktop_inspection_frame_limit(inspection, locale)
}

fn check_media_tools_internal(locale: UiLocale) -> ToolHealthReport {
    let ffmpeg = check_tool("ffmpeg", locale);
    let ready = ffmpeg.available;

    ToolHealthReport {
        ready,
        checks: vec![ffmpeg],
        summary: locale::tool_health_summary(locale, ready),
    }
}

#[tauri::command]
async fn check_media_tools(locale: Option<String>) -> ToolHealthReport {
    let locale = parse_ui_locale(locale.as_deref());

    match run_blocking_task(move || check_media_tools_internal(locale)).await {
        Ok(report) => report,
        Err(_) => ToolHealthReport {
            ready: false,
            checks: Vec::new(),
            summary: format!(
                "{} {}",
                locale::tool_health_check_failed_summary(locale),
                locale::internal_task_error_message(locale)
            ),
        },
    }
}

#[tauri::command]
async fn inspect_input_media(
    input_path: String,
    locale: Option<String>,
    operation_id: String,
    on_progress: Channel<OperationProgress>,
    pipeline_state: State<'_, PipelineState>,
) -> MediaInspection {
    let locale = parse_ui_locale(locale.as_deref());
    if input_path.trim().is_empty() {
        return inspection_pipeline_error(
            &input_path,
            &PipelineError::InvalidRequestWithoutReason,
            locale,
        );
    }
    let kind = MediaOperationKind::Inspect;
    let input_path_for_error = input_path.clone();
    let state = pipeline_state.inner().clone();
    let progress = ValidatedProgressSink::new(kind, ChannelProgressSink::new(on_progress));
    let reservation = match state.reserve(&operation_id) {
        Ok(reservation) => reservation,
        Err(error) => return inspection_pipeline_error(&input_path, &error, locale),
    };
    let preflight_context = reservation.context().clone();
    let preflight_input_path = input_path.clone();
    let preflight = match run_blocking_task(move || {
        run_reserved_preflight(&preflight_context, || {
            SourcePreflight::capture(Path::new(&preflight_input_path), MediaLimits::default())
        })
    })
    .await
    {
        Ok(Ok(preflight)) => preflight,
        Ok(Err(error)) => return inspection_pipeline_error(&input_path, &error, locale),
        Err(message) => {
            return inspection_pipeline_error(
                &input_path,
                &PipelineError::Io {
                    operation: "join source preflight",
                    message,
                },
                locale,
            )
        }
    };
    let managed = match reservation.promote(kind, &progress).await {
        Ok(managed) => managed,
        Err(error) => return inspection_pipeline_error(&input_path, &error, locale),
    };
    let context = managed.context().clone();

    match run_managed_blocking(managed, move || {
        let response = if let Err(error) =
            checkpointed_source_check(&context, preflight.identity(), MediaLimits::default())
        {
            inspection_pipeline_error(&input_path, &error, locale)
        } else {
            inspect_input_media_with_operation(&input_path, &preflight, locale, &context, &progress)
        };
        match finalize_managed_response(&context, response) {
            Ok(response) => response,
            Err(error) => inspection_pipeline_error(&input_path, &error, locale),
        }
    })
    .await
    {
        Ok(result) => result,
        Err(message) => inspection_pipeline_error(
            &input_path_for_error,
            &PipelineError::Io {
                operation: "join media worker",
                message,
            },
            locale,
        ),
    }
}

#[tauri::command]
async fn build_optimizer_plan(
    request: OptimizerPlanRequest,
    operation_id: String,
    on_progress: Channel<OperationProgress>,
    pipeline_state: State<'_, PipelineState>,
) -> OptimizerPlanResponse {
    let locale = parse_ui_locale(request.locale.as_deref());
    let (_, fallback_fit_warning) = normalized_fit_mode(request.fit_mode.as_deref(), locale);
    let kind = MediaOperationKind::BuildPlan;
    let state = pipeline_state.inner().clone();
    let progress = ValidatedProgressSink::new(kind, ChannelProgressSink::new(on_progress));
    let reservation = match state.reserve(&operation_id) {
        Ok(reservation) => reservation,
        Err(error) => {
            return optimizer_plan_pipeline_error(
                locale,
                fallback_fit_warning.into_iter().collect(),
                &error,
            )
        }
    };
    let managed = match reservation.promote(kind, &progress).await {
        Ok(managed) => managed,
        Err(error) => {
            return optimizer_plan_pipeline_error(
                locale,
                fallback_fit_warning.into_iter().collect(),
                &error,
            )
        }
    };
    let context = managed.context().clone();
    let worker_fallback_fit_warning = fallback_fit_warning.clone();

    match run_managed_blocking(managed, move || {
        let response = prepare_optimizer_plan_with_operation(&request, locale, &context, &progress);
        match finalize_managed_response(&context, response) {
            Ok(response) => response,
            Err(error) => optimizer_plan_pipeline_error(
                locale,
                worker_fallback_fit_warning.into_iter().collect(),
                &error,
            ),
        }
    })
    .await
    {
        Ok(result) => result,
        Err(message) => optimizer_plan_pipeline_error(
            locale,
            fallback_fit_warning.into_iter().collect(),
            &PipelineError::Io {
                operation: "join media worker",
                message,
            },
        ),
    }
}

#[tauri::command]
async fn estimate_static_output_size(
    request: StaticSizeEstimateRequest,
    operation_id: String,
    on_progress: Channel<OperationProgress>,
    pipeline_state: State<'_, PipelineState>,
) -> Result<OutputSizeEstimate, OutputSizeEstimateError> {
    let locale = parse_ui_locale(Some(request.locale.as_str()));
    if request.input_path.trim().is_empty() || request.source_revision.trim().is_empty() {
        return Err(OutputSizeEstimateError::from_pipeline(
            &PipelineError::InvalidRequestWithoutReason,
            locale,
        ));
    }

    let kind = MediaOperationKind::StaticEstimate;
    let state = pipeline_state.inner().clone();
    let progress = ValidatedProgressSink::new(kind, ChannelProgressSink::new(on_progress));
    let reservation = state
        .reserve(&operation_id)
        .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
    let preflight_context = reservation.context().clone();
    let preflight_input_path = request.input_path.clone();
    let preflight_source_revision = request.source_revision.clone();
    let preflight = match run_blocking_task(move || {
        run_reserved_preflight(&preflight_context, || {
            SourcePreflight::validate_expected(
                Path::new(&preflight_input_path),
                Some(preflight_source_revision.as_str()),
                MediaLimits::default(),
            )
        })
    })
    .await
    {
        Ok(Ok(preflight)) => preflight,
        Ok(Err(error)) => return Err(OutputSizeEstimateError::from_pipeline(&error, locale)),
        Err(message) => {
            return Err(OutputSizeEstimateError::from_pipeline(
                &PipelineError::Io {
                    operation: "join source preflight",
                    message,
                },
                locale,
            ))
        }
    };
    let managed = reservation
        .promote(kind, &progress)
        .await
        .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
    let context = managed.context().clone();

    match run_managed_blocking(managed, move || {
        let estimate = (|| -> Result<OutputSizeEstimate, OutputSizeEstimateError> {
            checkpointed_source_check(&context, preflight.identity(), MediaLimits::default())
                .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
            publish_progress(&progress, &context, ProgressStage::Inspecting, 0, Some(1));
            let canonical_path = preflight.canonical_path().to_string_lossy().into_owned();
            let mut checkpoint = || context.checkpoint();
            let inspection = inspect_input_media_preflighted_with_callbacks(
                &canonical_path,
                &preflight,
                locale,
                Some(&context),
                &mut checkpoint,
            );
            checkpointed_source_check(&context, preflight.identity(), MediaLimits::default())
                .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
            publish_progress(&progress, &context, ProgressStage::Inspecting, 1, Some(1));
            if !inspection.ok {
                return Err(OutputSizeEstimateError::new(
                    inspection
                        .error_code
                        .unwrap_or(MediaOperationErrorCode::InternalTaskFailed),
                    inspection.reason_code,
                    inspection.error_message.unwrap_or_else(|| {
                        locale::media_pipeline_diagnostic(locale, "internal-task-failed")
                    }),
                ));
            }
            if !inspection.is_static_image {
                return Err(OutputSizeEstimateError::new(
                    MediaOperationErrorCode::InvalidRequest,
                    Some(MediaOperationReasonCode::UnsupportedSourceFormat),
                    locale::unsupported_still_image_error(locale),
                ));
            }
            if let Err(error_message) = resolve_crop_region(
                request.crop_region.as_ref(),
                inspection.width,
                inspection.height,
                locale,
            ) {
                return Err(OutputSizeEstimateError::new(
                    MediaOperationErrorCode::InvalidRequest,
                    Some(MediaOperationReasonCode::InvalidCrop),
                    error_message,
                ));
            }

            let estimate = estimate_static_png_with_operation(
                preflight.canonical_path(),
                request.crop_region.as_ref(),
                preflight.identity(),
                &context,
                MediaLimits::default(),
                &progress,
            )
            .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
            finalize_source_publication(&context, preflight.identity(), || {
                publish_progress(&progress, &context, ProgressStage::Finalizing, 1, Some(1));
                Ok(estimate)
            })
            .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))
        })();
        match finalize_managed_response(&context, estimate) {
            Ok(estimate) => estimate,
            Err(error) => Err(OutputSizeEstimateError::from_pipeline(&error, locale)),
        }
    })
    .await
    {
        Ok(result) => result,
        Err(message) => Err(OutputSizeEstimateError::from_pipeline(
            &PipelineError::Io {
                operation: "join media worker",
                message,
            },
            locale,
        )),
    }
}

#[tauri::command]
async fn estimate_optimizer_candidates(
    request: OptimizerSizeEstimateRequest,
    operation_id: String,
    on_progress: Channel<OperationProgress>,
    pipeline_state: State<'_, PipelineState>,
) -> Result<Vec<OutputSizeEstimate>, OutputSizeEstimateError> {
    let locale = parse_ui_locale(request.plan.locale.as_deref());
    if request.input_path.trim().is_empty()
        || request.source_revision.trim().is_empty()
        || validate_candidate_ids(&request.candidate_ids).is_err()
        || parse_sample_seed(&request.sample_seed).is_err()
    {
        return Err(OutputSizeEstimateError::from_pipeline(
            &PipelineError::InvalidRequestWithoutReason,
            locale,
        ));
    }
    let kind = MediaOperationKind::OptimizerEstimate;
    let state = pipeline_state.inner().clone();
    let progress = ValidatedProgressSink::new(kind, ChannelProgressSink::new(on_progress));
    let reservation = state
        .reserve(&operation_id)
        .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
    let preflight_context = reservation.context().clone();
    let preflight_input_path = request.input_path.clone();
    let preflight_source_revision = request.source_revision.clone();
    let preflight = match run_blocking_task(move || {
        run_reserved_preflight(&preflight_context, || {
            SourcePreflight::validate_expected(
                Path::new(&preflight_input_path),
                Some(preflight_source_revision.as_str()),
                MediaLimits::default(),
            )
        })
    })
    .await
    {
        Ok(Ok(preflight)) => preflight,
        Ok(Err(error)) => return Err(OutputSizeEstimateError::from_pipeline(&error, locale)),
        Err(message) => {
            return Err(OutputSizeEstimateError::from_pipeline(
                &PipelineError::Io {
                    operation: "join source preflight",
                    message,
                },
                locale,
            ))
        }
    };
    let managed = reservation
        .promote(kind, &progress)
        .await
        .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
    let context = managed.context().clone();

    match run_managed_blocking(managed, move || {
        let result = (|| -> Result<Vec<OutputSizeEstimate>, OutputSizeEstimateError> {
            checkpointed_source_check(&context, preflight.identity(), MediaLimits::default())
                .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
            let loader = DefaultFrameSourceLoader;
            let estimates = estimate_optimizer_candidates_with_loader(
                &request, &preflight, locale, &loader, &context, &progress,
            )
            .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
            finalize_source_publication(&context, preflight.identity(), || {
                publish_progress(&progress, &context, ProgressStage::Finalizing, 1, Some(1));
                Ok(estimates)
            })
            .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))
        })();
        match finalize_managed_response(&context, result) {
            Ok(result) => result,
            Err(error) => Err(OutputSizeEstimateError::from_pipeline(&error, locale)),
        }
    })
    .await
    {
        Ok(result) => result,
        Err(message) => Err(OutputSizeEstimateError::from_pipeline(
            &PipelineError::Io {
                operation: "join media worker",
                message,
            },
            locale,
        )),
    }
}

#[tauri::command]
async fn probe_optimizer_candidate_size(
    request: CandidateSizeProbeRequest,
    operation_id: String,
    on_progress: Channel<OperationProgress>,
    pipeline_state: State<'_, PipelineState>,
) -> Result<OutputSizeEstimate, OutputSizeEstimateError> {
    let locale = parse_ui_locale(request.plan.locale.as_deref());
    if request.input_path.trim().is_empty()
        || request.source_revision.trim().is_empty()
        || validate_candidate_ids(std::slice::from_ref(&request.candidate_id)).is_err()
    {
        return Err(OutputSizeEstimateError::from_pipeline(
            &PipelineError::InvalidRequestWithoutReason,
            locale,
        ));
    }
    let kind = MediaOperationKind::OptimizerProbe;
    let state = pipeline_state.inner().clone();
    let progress = ValidatedProgressSink::new(kind, ChannelProgressSink::new(on_progress));
    let reservation = state
        .reserve(&operation_id)
        .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
    let preflight_context = reservation.context().clone();
    let preflight_input_path = request.input_path.clone();
    let preflight_source_revision = request.source_revision.clone();
    let preflight = match run_blocking_task(move || {
        run_reserved_preflight(&preflight_context, || {
            SourcePreflight::validate_expected(
                Path::new(&preflight_input_path),
                Some(preflight_source_revision.as_str()),
                MediaLimits::default(),
            )
        })
    })
    .await
    {
        Ok(Ok(preflight)) => preflight,
        Ok(Err(error)) => return Err(OutputSizeEstimateError::from_pipeline(&error, locale)),
        Err(message) => {
            return Err(OutputSizeEstimateError::from_pipeline(
                &PipelineError::Io {
                    operation: "join source preflight",
                    message,
                },
                locale,
            ))
        }
    };
    let managed = reservation
        .promote(kind, &progress)
        .await
        .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
    let context = managed.context().clone();

    match run_managed_blocking(managed, move || {
        let result = (|| -> Result<OutputSizeEstimate, OutputSizeEstimateError> {
            checkpointed_source_check(&context, preflight.identity(), MediaLimits::default())
                .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
            let loader = DefaultFrameSourceLoader;
            let estimate = probe_optimizer_candidate_with_loader(
                &request, &preflight, locale, &loader, &context, &progress,
            )
            .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))?;
            finalize_source_publication(&context, preflight.identity(), || {
                publish_progress(&progress, &context, ProgressStage::Finalizing, 1, Some(1));
                Ok(estimate)
            })
            .map_err(|error| OutputSizeEstimateError::from_pipeline(&error, locale))
        })();
        match finalize_managed_response(&context, result) {
            Ok(result) => result,
            Err(error) => Err(OutputSizeEstimateError::from_pipeline(&error, locale)),
        }
    })
    .await
    {
        Ok(result) => result,
        Err(message) => Err(OutputSizeEstimateError::from_pipeline(
            &PipelineError::Io {
                operation: "join media worker",
                message,
            },
            locale,
        )),
    }
}

#[tauri::command]
async fn convert_static_image_to_png(
    request: StaticImageConversionRequest,
    operation_id: String,
    on_progress: Channel<OperationProgress>,
    pipeline_state: State<'_, PipelineState>,
) -> StaticImageConversionResult {
    let locale = parse_ui_locale(request.locale.as_deref());
    if request.input_path.trim().is_empty()
        || request
            .source_revision
            .as_deref()
            .filter(|revision| !revision.trim().is_empty())
            .is_none()
    {
        return static_conversion_pipeline_error(
            &PipelineError::InvalidRequestWithoutReason,
            locale,
        );
    }
    let kind = MediaOperationKind::StaticConversion;
    let state = pipeline_state.inner().clone();
    let progress = ValidatedProgressSink::new(kind, ChannelProgressSink::new(on_progress));
    let reservation = match state.reserve(&operation_id) {
        Ok(reservation) => reservation,
        Err(error) => return static_conversion_pipeline_error(&error, locale),
    };
    let preflight_context = reservation.context().clone();
    let preflight_input_path = request.input_path.clone();
    let preflight_source_revision = request.source_revision.clone();
    let preflight = match run_blocking_task(move || {
        run_reserved_preflight(&preflight_context, || {
            SourcePreflight::validate_expected(
                Path::new(&preflight_input_path),
                preflight_source_revision.as_deref(),
                MediaLimits::default(),
            )
        })
    })
    .await
    {
        Ok(Ok(preflight)) => preflight,
        Ok(Err(error)) => return static_conversion_pipeline_error(&error, locale),
        Err(message) => {
            return static_conversion_pipeline_error(
                &PipelineError::Io {
                    operation: "join source preflight",
                    message,
                },
                locale,
            )
        }
    };
    let managed = match reservation.promote(kind, &progress).await {
        Ok(managed) => managed,
        Err(error) => return static_conversion_pipeline_error(&error, locale),
    };
    let context = managed.context().clone();

    match run_managed_blocking(managed, move || {
        let response = if let Err(error) =
            checkpointed_source_check(&context, preflight.identity(), MediaLimits::default())
        {
            static_conversion_pipeline_error(&error, locale)
        } else {
            let canonical_path = preflight.canonical_path().to_string_lossy().into_owned();
            convert_static_image_to_png_with_operation(
                &canonical_path,
                request.output_directory.as_deref(),
                request.crop_region.as_ref(),
                locale,
                &preflight,
                &context,
                &progress,
            )
        };
        match finalize_managed_response(&context, response) {
            Ok(response) => response,
            Err(error) => static_conversion_pipeline_error(&error, locale),
        }
    })
    .await
    {
        Ok(result) => result,
        Err(message) => static_conversion_pipeline_error(
            &PipelineError::Io {
                operation: "join media worker",
                message,
            },
            locale,
        ),
    }
}

fn optimizer_search_pipeline_error(
    locale: UiLocale,
    warnings: Vec<String>,
    error: PipelineError,
) -> OptimizerSearchResponse {
    optimizer_search_pipeline_error_with_duration(locale, warnings, error, None)
}

fn optimizer_search_pipeline_error_with_duration(
    locale: UiLocale,
    warnings: Vec<String>,
    error: PipelineError,
    selected_duration_seconds: Option<f64>,
) -> OptimizerSearchResponse {
    OptimizerSearchResponse {
        ok: false,
        fit_mode: CANONICAL_FIT_MODE.into(),
        selected_duration_seconds,
        limit_bytes: DISCORD_MAX_STICKER_BYTES,
        search_budget: MAX_SEARCH_BUDGET,
        real_attempt_count: 0,
        stop_reason: Some(error.code().into()),
        selection_reason: "no_fit_found".into(),
        summary: locale::optimizer_search_summary(locale, "no_fit_found"),
        warnings,
        attempts: Vec::new(),
        winning_candidate_id: None,
        closest_candidate_id: None,
        best_output_path: None,
        best_size_bytes: None,
        best_within_limit: false,
        reason_code: error.media_reason_code(),
        error_code: Some(error.error_code()),
        error_message: Some(pipeline_error_diagnostic(&error, locale)),
    }
}

#[derive(Clone, Copy)]
enum OptimizerRequestDomainOrigin {
    Direct,
    Plan,
}

struct OptimizerRequestDomainError {
    origin: OptimizerRequestDomainOrigin,
    reason: &'static str,
    message: String,
    selected_duration_seconds: Option<f64>,
    warnings: Vec<String>,
}

fn optimizer_request_domain_error(
    locale: UiLocale,
    origin: OptimizerRequestDomainOrigin,
    reason: &'static str,
    selected_duration_seconds: Option<f64>,
) -> OptimizerRequestDomainError {
    let message = match reason {
        "no-frames-selected" => locale::frame_selection_required_error(locale),
        "invalid-frame-duration" => locale::invalid_frame_duration_error(locale),
        "duration-too-long" => locale::selected_duration_limit_error(locale),
        _ => locale::invalid_frame_selection_error(locale),
    };
    OptimizerRequestDomainError {
        origin,
        reason,
        message,
        selected_duration_seconds,
        warnings: Vec::new(),
    }
}

fn with_optimizer_domain_warnings(
    mut error: OptimizerRequestDomainError,
    warnings: &[String],
) -> OptimizerRequestDomainError {
    error.warnings.extend_from_slice(warnings);
    error
}

fn validate_optimizer_search_request_domain(
    request: &OptimizerSearchRequest,
    locale: UiLocale,
) -> Result<(), OptimizerRequestDomainError> {
    let optimizer_goal = normalized_optimizer_goal(
        request.optimizer_goal.as_deref(),
        request.preset_strategy.as_deref(),
    );
    let quality_frame_drop_interval =
        normalized_quality_frame_drop_interval(request.quality_frame_drop_interval);
    let timeline_frames =
        resolve_timeline_frames(request.timeline_frames.as_ref(), request.base_frame_count)
            .map_err(|reason| {
                optimizer_request_domain_error(
                    locale,
                    OptimizerRequestDomainOrigin::Direct,
                    reason,
                    None,
                )
            })?;
    let (timeline_frames, frame_selection) = match timeline_frames {
        Some(timeline_frames) => {
            let timeline_frames = if optimizer_goal == "quality" {
                apply_quality_frame_drop_to_timeline_frames(
                    timeline_frames,
                    quality_frame_drop_interval,
                )
                .map_err(|reason| {
                    optimizer_request_domain_error(
                        locale,
                        OptimizerRequestDomainOrigin::Direct,
                        reason,
                        None,
                    )
                })?
            } else {
                timeline_frames
            };
            (Some(timeline_frames), None)
        }
        None => {
            let frame_selection =
                resolve_frame_selection(request.selected_frames.as_ref(), request.base_frame_count)
                    .map_err(|reason| {
                        optimizer_request_domain_error(
                            locale,
                            OptimizerRequestDomainOrigin::Direct,
                            reason,
                            None,
                        )
                    })?;
            let frame_selection = if optimizer_goal == "quality" {
                apply_quality_frame_drop_to_selection(frame_selection, quality_frame_drop_interval)
                    .map_err(|reason| {
                        optimizer_request_domain_error(
                            locale,
                            OptimizerRequestDomainOrigin::Direct,
                            reason,
                            None,
                        )
                    })?
            } else {
                frame_selection
            };
            (None, Some(frame_selection))
        }
    };

    let resolved_crop_region = resolve_crop_region(
        request.crop_region.as_ref(),
        request.input_width,
        request.input_height,
        locale,
    )
    .map_err(|message| OptimizerRequestDomainError {
        origin: OptimizerRequestDomainOrigin::Plan,
        reason: "invalid-crop",
        message,
        selected_duration_seconds: None,
        warnings: Vec::new(),
    })?;
    let plan_warnings = resolved_crop_region
        .is_some()
        .then(|| locale::crop_applied_before_scale_warning(locale))
        .into_iter()
        .collect::<Vec<_>>();

    if let Some(timeline_frames) = timeline_frames {
        let duration_us = timeline_duration_us(&timeline_frames).map_err(|reason| {
            with_optimizer_domain_warnings(
                optimizer_request_domain_error(
                    locale,
                    OptimizerRequestDomainOrigin::Plan,
                    reason,
                    None,
                ),
                &plan_warnings,
            )
        })?;
        if duration_us > DISCORD_MAX_DURATION_US {
            let duration_seconds = duration_us_to_seconds(duration_us);
            return Err(with_optimizer_domain_warnings(
                optimizer_request_domain_error(
                    locale,
                    OptimizerRequestDomainOrigin::Plan,
                    "duration-too-long",
                    Some(duration_seconds),
                ),
                &plan_warnings,
            ));
        }
        return Ok(());
    }

    let frame_selection = frame_selection.expect("selection exists when timeline is absent");
    let source_fps = derive_source_fps(
        request.avg_fps,
        request.source_duration_seconds,
        frame_selection.base_frame_count,
    );
    let duration_seconds = projected_selection_duration_seconds(
        frame_selection.selected_frame_count,
        frame_selection.base_frame_count,
        request.source_duration_seconds,
        source_fps,
    );
    if duration_seconds > duration_us_to_seconds(DISCORD_MAX_DURATION_US) {
        return Err(with_optimizer_domain_warnings(
            optimizer_request_domain_error(
                locale,
                OptimizerRequestDomainOrigin::Plan,
                "duration-too-long",
                Some(duration_seconds),
            ),
            &plan_warnings,
        ));
    }
    Ok(())
}

fn optimizer_search_domain_error(
    locale: UiLocale,
    mut warnings: Vec<String>,
    error: OptimizerRequestDomainError,
) -> OptimizerSearchResponse {
    let OptimizerRequestDomainError {
        origin,
        reason,
        message,
        selected_duration_seconds,
        warnings: domain_warnings,
    } = error;
    warnings.extend(domain_warnings);
    let mut response = optimizer_search_pipeline_error_with_duration(
        locale,
        warnings,
        PipelineError::InvalidRequest { reason },
        selected_duration_seconds,
    );
    match origin {
        OptimizerRequestDomainOrigin::Direct => {
            response.stop_reason = Some(reason.into());
            response.summary = locale::plan_failed_message(locale);
        }
        OptimizerRequestDomainOrigin::Plan => {
            response.stop_reason = Some("plan-invalid".into());
            response.summary = message.clone();
        }
    }
    response.error_message = Some(message);
    response
}

fn run_optimizer_search_internal(
    mut request: OptimizerSearchRequest,
    locale: UiLocale,
) -> OptimizerSearchResponse {
    run_optimizer_search_with_callbacks(request, locale, None, || Ok(()), |_, _, _| {})
}

fn run_optimizer_search_with_operation(
    request: OptimizerSearchRequest,
    locale: UiLocale,
    preflight: &SourcePreflight,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> OptimizerSearchResponse {
    let loader = DefaultFrameSourceLoader;
    run_optimizer_search_with_loader_core(
        request,
        locale,
        Some(context),
        &loader,
        Some(preflight),
        || context.checkpoint(),
        |stage, completed, total| {
            publish_progress(progress, context, stage, completed, total);
        },
    )
}

fn run_optimizer_search_with_callbacks(
    request: OptimizerSearchRequest,
    locale: UiLocale,
    context: Option<&OperationContext>,
    checkpoint: impl FnMut() -> Result<(), PipelineError>,
    progress: impl FnMut(ProgressStage, u32, Option<u32>),
) -> OptimizerSearchResponse {
    let loader = DefaultFrameSourceLoader;
    run_optimizer_search_with_loader(request, locale, context, &loader, checkpoint, progress)
}

fn publish_optimizer_finalizing_progress(
    progress: &mut impl FnMut(ProgressStage, u32, Option<u32>),
    completed: u32,
    total: u32,
) {
    progress(ProgressStage::Finalizing, completed, Some(total));
}

fn run_optimizer_search_with_loader(
    mut request: OptimizerSearchRequest,
    locale: UiLocale,
    context: Option<&OperationContext>,
    loader: &impl FrameSourceLoader,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
    mut progress: impl FnMut(ProgressStage, u32, Option<u32>),
) -> OptimizerSearchResponse {
    run_optimizer_search_with_loader_core(
        request,
        locale,
        context,
        loader,
        None,
        &mut checkpoint,
        &mut progress,
    )
}

fn run_optimizer_search_with_loader_core(
    mut request: OptimizerSearchRequest,
    locale: UiLocale,
    context: Option<&OperationContext>,
    loader: &impl FrameSourceLoader,
    source_preflight: Option<&SourcePreflight>,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
    mut progress: impl FnMut(ProgressStage, u32, Option<u32>),
) -> OptimizerSearchResponse {
    let detached_context = context
        .is_none()
        .then(|| OperationContext::detached(MediaOperationKind::OptimizerSearch.timeout()));
    let operation_context =
        context.unwrap_or_else(|| detached_context.as_ref().expect("detached search context"));
    let (_, legacy_fit_warning) = normalized_fit_mode(request.fit_mode.as_deref(), locale);
    let legacy_fit_warnings = legacy_fit_warning.into_iter().collect::<Vec<_>>();
    if let Err(error) = checkpoint() {
        return optimizer_search_pipeline_error(locale, legacy_fit_warnings, error);
    }
    let source_identity = match source_preflight {
        Some(preflight) => preflight.identity().clone(),
        None => match validate_source_revision(
            Path::new(&request.input_path),
            request.source_revision.as_deref(),
            MediaLimits::default(),
        ) {
            Ok(identity) => identity,
            Err(error) => {
                return optimizer_search_pipeline_error(locale, legacy_fit_warnings, error);
            }
        },
    };
    request.input_path = source_identity
        .canonical_path
        .to_string_lossy()
        .into_owned();
    let response = (|| {
        let optimizer_goal = normalized_optimizer_goal(
            request.optimizer_goal.as_deref(),
            request.preset_strategy.as_deref(),
        );
        let preset_strategy = normalized_preset_strategy(request.preset_strategy.as_deref());
        let quality_frame_drop_interval =
            normalized_quality_frame_drop_interval(request.quality_frame_drop_interval);
        let resolved_timeline_frames = match resolve_timeline_frames(
            request.timeline_frames.as_ref(),
            request.base_frame_count,
        ) {
            Ok(timeline_frames) => match (timeline_frames, optimizer_goal) {
                (Some(timeline_frames), "quality") => {
                    match apply_quality_frame_drop_to_timeline_frames(
                        timeline_frames,
                        quality_frame_drop_interval,
                    ) {
                        Ok(timeline_frames) => Some(timeline_frames),
                        Err(error) => {
                            return OptimizerSearchResponse {
                                ok: false,
                                fit_mode: CANONICAL_FIT_MODE.into(),
                                selected_duration_seconds: None,
                                limit_bytes: DISCORD_MAX_STICKER_BYTES,
                                search_budget: MAX_SEARCH_BUDGET,
                                real_attempt_count: 0,
                                stop_reason: Some(error.into()),
                                selection_reason: "no_fit_found".into(),
                                summary: locale::plan_failed_message(locale),
                                warnings: legacy_fit_warnings.clone(),
                                attempts: Vec::new(),
                                winning_candidate_id: None,
                                closest_candidate_id: None,
                                best_output_path: None,
                                best_size_bytes: None,
                                best_within_limit: false,
                                reason_code: MediaOperationReasonCode::from_code(error),
                                error_code: Some(
                                    MediaOperationErrorCode::for_invalid_request_reason(error),
                                ),
                                error_message: Some(locale::frame_selection_required_error(locale)),
                            };
                        }
                    }
                }
                (timeline_frames, _) => timeline_frames,
            },
            Err("no-frames-selected") => {
                return OptimizerSearchResponse {
                    ok: false,
                    fit_mode: CANONICAL_FIT_MODE.into(),
                    selected_duration_seconds: None,
                    limit_bytes: DISCORD_MAX_STICKER_BYTES,
                    search_budget: MAX_SEARCH_BUDGET,
                    real_attempt_count: 0,
                    stop_reason: Some("no-frames-selected".into()),
                    selection_reason: "no_fit_found".into(),
                    summary: locale::plan_failed_message(locale),
                    warnings: legacy_fit_warnings.clone(),
                    attempts: Vec::new(),
                    winning_candidate_id: None,
                    closest_candidate_id: None,
                    best_output_path: None,
                    best_size_bytes: None,
                    best_within_limit: false,
                    reason_code: Some(MediaOperationReasonCode::NoFramesSelected),
                    error_code: Some(MediaOperationErrorCode::InvalidRequest),
                    error_message: Some(locale::frame_selection_required_error(locale)),
                };
            }
            Err(reason) => {
                let error_message = if reason == "invalid-frame-duration" {
                    locale::invalid_frame_duration_error(locale)
                } else {
                    locale::invalid_frame_selection_error(locale)
                };
                return OptimizerSearchResponse {
                    ok: false,
                    fit_mode: CANONICAL_FIT_MODE.into(),
                    selected_duration_seconds: None,
                    limit_bytes: DISCORD_MAX_STICKER_BYTES,
                    search_budget: MAX_SEARCH_BUDGET,
                    real_attempt_count: 0,
                    stop_reason: Some(reason.into()),
                    selection_reason: "no_fit_found".into(),
                    summary: locale::plan_failed_message(locale),
                    warnings: legacy_fit_warnings.clone(),
                    attempts: Vec::new(),
                    winning_candidate_id: None,
                    closest_candidate_id: None,
                    best_output_path: None,
                    best_size_bytes: None,
                    best_within_limit: false,
                    reason_code: MediaOperationReasonCode::from_code(reason),
                    error_code: Some(MediaOperationErrorCode::for_invalid_request_reason(reason)),
                    error_message: Some(error_message),
                };
            }
        };

        let legacy_selected_frames = if resolved_timeline_frames.is_none() {
            Some(
                match resolve_frame_selection(
                    request.selected_frames.as_ref(),
                    request.base_frame_count,
                ) {
                    Ok(selection) => {
                        let selection = if optimizer_goal == "quality" {
                            match apply_quality_frame_drop_to_selection(
                                selection,
                                quality_frame_drop_interval,
                            ) {
                                Ok(selection) => selection,
                                Err(error) => {
                                    return OptimizerSearchResponse {
                                        ok: false,
                                        fit_mode: CANONICAL_FIT_MODE.into(),
                                        selected_duration_seconds: None,
                                        limit_bytes: DISCORD_MAX_STICKER_BYTES,
                                        search_budget: MAX_SEARCH_BUDGET,
                                        real_attempt_count: 0,
                                        stop_reason: Some(error.into()),
                                        selection_reason: "no_fit_found".into(),
                                        summary: locale::plan_failed_message(locale),
                                        warnings: legacy_fit_warnings.clone(),
                                        attempts: Vec::new(),
                                        winning_candidate_id: None,
                                        closest_candidate_id: None,
                                        best_output_path: None,
                                        best_size_bytes: None,
                                        best_within_limit: false,
                                        reason_code: MediaOperationReasonCode::from_code(error),
                                        error_code: Some(
                                            MediaOperationErrorCode::for_invalid_request_reason(
                                                error,
                                            ),
                                        ),
                                        error_message: Some(
                                            locale::frame_selection_required_error(locale),
                                        ),
                                    };
                                }
                            }
                        } else {
                            selection
                        };

                        selection.selected_frames
                    }
                    Err("no-frames-selected") => {
                        return OptimizerSearchResponse {
                            ok: false,
                            fit_mode: CANONICAL_FIT_MODE.into(),
                            selected_duration_seconds: None,
                            limit_bytes: DISCORD_MAX_STICKER_BYTES,
                            search_budget: MAX_SEARCH_BUDGET,
                            real_attempt_count: 0,
                            stop_reason: Some("no-frames-selected".into()),
                            selection_reason: "no_fit_found".into(),
                            summary: locale::plan_failed_message(locale),
                            warnings: legacy_fit_warnings.clone(),
                            attempts: Vec::new(),
                            winning_candidate_id: None,
                            closest_candidate_id: None,
                            best_output_path: None,
                            best_size_bytes: None,
                            best_within_limit: false,
                            reason_code: Some(MediaOperationReasonCode::NoFramesSelected),
                            error_code: Some(MediaOperationErrorCode::InvalidRequest),
                            error_message: Some(locale::frame_selection_required_error(locale)),
                        };
                    }
                    Err(_) => {
                        return OptimizerSearchResponse {
                            ok: false,
                            fit_mode: CANONICAL_FIT_MODE.into(),
                            selected_duration_seconds: None,
                            limit_bytes: DISCORD_MAX_STICKER_BYTES,
                            search_budget: MAX_SEARCH_BUDGET,
                            real_attempt_count: 0,
                            stop_reason: Some("invalid-frame-selection".into()),
                            selection_reason: "no_fit_found".into(),
                            summary: locale::plan_failed_message(locale),
                            warnings: legacy_fit_warnings.clone(),
                            attempts: Vec::new(),
                            winning_candidate_id: None,
                            closest_candidate_id: None,
                            best_output_path: None,
                            best_size_bytes: None,
                            best_within_limit: false,
                            reason_code: Some(MediaOperationReasonCode::InvalidFrameSelection),
                            error_code: Some(MediaOperationErrorCode::InvalidRequest),
                            error_message: Some(locale::invalid_frame_selection_error(locale)),
                        };
                    }
                },
            )
        } else {
            None
        };

        progress(ProgressStage::Estimating, 0, None);
        let mut plan = prepare_optimizer_plan_with_checkpoint(
            &OptimizerPlanRequest {
                locale: request.locale.clone(),
                source_duration_seconds: request.source_duration_seconds,
                input_width: request.input_width,
                input_height: request.input_height,
                avg_fps: request.avg_fps,
                fit_mode: request.fit_mode.clone(),
                preset_strategy: request.preset_strategy.clone(),
                optimizer_goal: request.optimizer_goal.clone(),
                quality_frame_drop_interval: request.quality_frame_drop_interval,
                search_depth: request.search_depth.clone(),
                crop_region: request.crop_region.clone(),
                selected_frames: request.selected_frames.clone(),
                base_frame_count: request.base_frame_count,
                timeline_frames: request.timeline_frames.clone(),
            },
            locale,
            &mut checkpoint,
        );

        if !plan.ok {
            return OptimizerSearchResponse {
                ok: false,
                fit_mode: plan.fit_mode,
                selected_duration_seconds: plan.selected_duration_seconds,
                limit_bytes: DISCORD_MAX_STICKER_BYTES,
                search_budget: MAX_SEARCH_BUDGET,
                real_attempt_count: 0,
                stop_reason: Some("plan-invalid".into()),
                selection_reason: "no_fit_found".into(),
                summary: plan
                    .error_message
                    .clone()
                    .unwrap_or_else(|| locale::plan_failed_message(locale)),
                warnings: plan.warnings,
                attempts: Vec::new(),
                winning_candidate_id: None,
                closest_candidate_id: None,
                best_output_path: None,
                best_size_bytes: None,
                best_within_limit: false,
                reason_code: plan.reason_code,
                error_code: plan.error_code,
                error_message: plan.error_message,
            };
        }

        progress(ProgressStage::Decoding, 0, Some(1));
        let validated_revision = source_identity.revision();
        let prepared_source = match loader.prepare(
            FramePreparationRequest {
                input_path: Path::new(&request.input_path),
                source_revision: &validated_revision,
                crop_region: request.crop_region.as_ref(),
                input_width: request.input_width,
                input_height: request.input_height,
                base_frame_count: request.base_frame_count,
                timeline_frames: request.timeline_frames.as_deref(),
                resolved_timeline_frames: resolved_timeline_frames.as_deref(),
                selected_frame_indexes: legacy_selected_frames
                    .as_ref()
                    .and_then(|frames| frames.as_deref()),
                source_duration_seconds: request.source_duration_seconds,
                avg_fps: request.avg_fps,
                locale,
                optimizer_goal,
            },
            &plan,
            operation_context,
            MediaLimits::default(),
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                return optimizer_search_pipeline_error_with_duration(
                    locale,
                    plan.warnings.clone(),
                    error,
                    plan.selected_duration_seconds,
                )
            }
        };
        if let Err(error) = checkpoint().and_then(|_| {
            checkpointed_source_check(operation_context, &source_identity, MediaLimits::default())
        }) {
            return optimizer_search_pipeline_error_with_duration(
                locale,
                plan.warnings.clone(),
                error,
                plan.selected_duration_seconds,
            );
        }
        if let Err(error) = synchronize_candidates_with_prepared_duration(
            &mut plan,
            &prepared_source,
            request.input_width,
            request.input_height,
            preset_strategy,
            optimizer_goal,
            locale,
            operation_context,
        ) {
            return optimizer_search_pipeline_error_with_duration(
                locale,
                plan.warnings.clone(),
                error,
                plan.selected_duration_seconds,
            );
        }
        progress(ProgressStage::Decoding, 1, Some(1));

        let mut attempts = Vec::new();
        let mut best_within_limit_output: Option<PendingSelectedEncodeOutput> = None;
        let mut smallest_oversize_output: Option<PendingSelectedEncodeOutput> = None;
        let mut stopped_after_best_within_limit = false;
        let total_candidates = u32::try_from(plan.candidates.len()).unwrap_or(u32::MAX);
        let mut completed_candidates = 0_u32;
        progress(ProgressStage::Encoding, 0, Some(total_candidates));
        for candidate in &plan.candidates {
            if let Err(error) = checkpoint() {
                return optimizer_search_pipeline_error_with_duration(
                    locale,
                    plan.warnings.clone(),
                    error,
                    plan.selected_duration_seconds,
                );
            }
            if let Err(error) = checkpointed_source_check(
                operation_context,
                &source_identity,
                MediaLimits::default(),
            ) {
                return optimizer_search_pipeline_error_with_duration(
                    locale,
                    plan.warnings.clone(),
                    error,
                    plan.selected_duration_seconds,
                );
            }
            if remaining_candidate_cannot_beat_within_limit(
                best_within_limit_output
                    .as_ref()
                    .map(|output| &output.selected),
                candidate,
            ) {
                stopped_after_best_within_limit = true;
                break;
            }

            let sequence = match build_candidate_output_sequence(
                &prepared_source,
                candidate,
                operation_context,
            ) {
                Ok(sequence) => sequence,
                Err(error) => {
                    return optimizer_search_pipeline_error_with_duration(
                        locale,
                        plan.warnings.clone(),
                        error,
                        plan.selected_duration_seconds,
                    )
                }
            };
            let candidate_duration_seconds = duration_us_to_seconds(sequence.duration_us);
            let encode_result = encode_prepared_candidate_with_checkpoint(
                Path::new(&request.input_path),
                request.output_directory.as_deref(),
                locale,
                &sequence,
                Some(&source_identity),
                operation_context,
            );

            match encode_result {
                Ok(result) => {
                    let EncodeResult {
                        pending_output,
                        size_bytes,
                        elapsed_ms,
                        tool_source,
                        tool_command,
                        tool_detail,
                    } = result;
                    let within_limit = size_bytes <= DISCORD_MAX_STICKER_BYTES;
                    let attempt = SearchAttemptResult {
                        candidate_id: candidate.id.clone(),
                        canonical_candidate_id: candidate.id.clone(),
                        equivalent_to_candidate_id: None,
                        rank: candidate.rank,
                        duration_seconds: candidate_duration_seconds,
                        fps: candidate.fps,
                        content_scale: candidate.content_scale,
                        preset: candidate.preset.clone(),
                        fit_mode: candidate.fit_mode.clone(),
                        score: candidate.score,
                        source_similarity_score: candidate.source_similarity_score,
                        summary: candidate.summary.clone(),
                        skipped: false,
                        within_limit,
                        output_path: None,
                        size_bytes: Some(size_bytes),
                        elapsed_ms: Some(elapsed_ms),
                        tool_source: Some(tool_source),
                        tool_command,
                        tool_detail,
                        warnings: Vec::new(),
                        reason_code: None,
                        error_code: None,
                        error_message: None,
                    };
                    attempts.push(attempt);
                    let contender = PendingSelectedEncodeOutput {
                        selected: SelectedEncodeOutput {
                            candidate_id: candidate.id.clone(),
                            rank: candidate.rank,
                            duration_seconds: candidate_duration_seconds,
                            size_bytes,
                            source_similarity_score: candidate.source_similarity_score,
                        },
                        pending_output,
                    };

                    if within_limit {
                        let replace_current = best_within_limit_output
                            .as_ref()
                            .map(|current| {
                                is_better_within_limit_candidate(
                                    &current.selected,
                                    &contender.selected,
                                )
                            })
                            .unwrap_or(true);

                        if replace_current {
                            drop(best_within_limit_output.replace(contender));
                        } else {
                            drop(contender);
                        }
                    } else {
                        let replace_current = smallest_oversize_output
                            .as_ref()
                            .map(|current| {
                                is_better_oversize_candidate(&current.selected, &contender.selected)
                            })
                            .unwrap_or(true);

                        if replace_current {
                            drop(smallest_oversize_output.replace(contender));
                        } else {
                            drop(contender);
                        }
                    }
                    completed_candidates = completed_candidates.saturating_add(1);
                    progress(
                        ProgressStage::Encoding,
                        completed_candidates,
                        Some(total_candidates),
                    );
                }
                Err(error) => {
                    return optimizer_search_pipeline_error_with_duration(
                        locale,
                        plan.warnings.clone(),
                        error,
                        plan.selected_duration_seconds,
                    );
                }
            }
        }

        let stop_reason = if stopped_after_best_within_limit {
            "found-best-ranked-within-limit"
        } else if attempts
            .iter()
            .any(|attempt| !attempt.skipped && attempt.size_bytes.is_some())
        {
            "exhausted-ranked-candidates"
        } else {
            "no-successful-encodes"
        };
        let selection_reason = if best_within_limit_output.is_some() {
            "best_within_limit"
        } else if smallest_oversize_output.is_some() {
            "smallest_oversize"
        } else {
            "no_fit_found"
        };
        let best_within_limit = best_within_limit_output.is_some();
        let published_output = if source_preflight.is_some() {
            finalize_source_publication(operation_context, &source_identity, || {
                progress(
                    ProgressStage::Finalizing,
                    completed_candidates,
                    Some(total_candidates),
                );
                publish_optimizer_selection(
                    &source_identity,
                    true,
                    best_within_limit_output,
                    smallest_oversize_output,
                    &mut attempts,
                )
            })
        } else {
            if let Err(error) = checkpoint() {
                return optimizer_search_pipeline_error_with_duration(
                    locale,
                    plan.warnings.clone(),
                    error,
                    plan.selected_duration_seconds,
                );
            }
            publish_optimizer_finalizing_progress(
                &mut progress,
                completed_candidates,
                total_candidates,
            );
            publish_optimizer_selection(
                &source_identity,
                false,
                best_within_limit_output,
                smallest_oversize_output,
                &mut attempts,
            )
        };
        let published_output = match published_output {
            Ok(output) => output,
            Err(error) => {
                return optimizer_search_pipeline_error_with_duration(
                    locale,
                    plan.warnings.clone(),
                    error,
                    plan.selected_duration_seconds,
                )
            }
        };
        let selected_output = published_output.as_ref();
        let closest_candidate_id =
            selected_output.map(|output| output.selected.candidate_id.clone());
        let winning_candidate_id = if best_within_limit {
            closest_candidate_id.clone()
        } else {
            None
        };
        let best_output_path = selected_output.map(|output| output.output_path.clone());
        let best_size_bytes = selected_output.map(|output| output.selected.size_bytes);
        let summary = locale::optimizer_search_summary(locale, selection_reason);
        let final_duration_seconds = selected_output
            .map(|output| output.selected.duration_seconds)
            .or(plan.selected_duration_seconds);

        OptimizerSearchResponse {
            ok: best_within_limit,
            fit_mode: plan.fit_mode,
            selected_duration_seconds: final_duration_seconds,
            limit_bytes: DISCORD_MAX_STICKER_BYTES,
            search_budget: plan.search_budget,
            real_attempt_count: attempts.len(),
            stop_reason: Some(stop_reason.into()),
            selection_reason: selection_reason.into(),
            summary,
            warnings: plan.warnings,
            attempts,
            winning_candidate_id,
            closest_candidate_id,
            best_output_path,
            best_size_bytes,
            best_within_limit,
            reason_code: None,
            error_code: None,
            error_message: None,
        }
    })();
    if source_preflight.is_some() {
        response
    } else {
        finalize_optimizer_search_source_unless_published(&source_identity, response, locale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_error::PipelineError;
    use crate::media_limits::MediaLimits;
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct IgnoringProgressSink;

    impl ProgressSink for IgnoringProgressSink {
        fn send(&self, _progress: OperationProgress) {}
    }

    struct CountingFrameSourceLoader {
        prepare_count: AtomicUsize,
    }

    impl CountingFrameSourceLoader {
        fn new() -> Self {
            Self {
                prepare_count: AtomicUsize::new(0),
            }
        }
    }

    #[test]
    fn candidate_estimate_preparation_calls_loader_once_and_stays_under_prepared_budget() {
        let loader = CountingFrameSourceLoader::new();
        let context = OperationContext::detached(MediaOperationKind::OptimizerEstimate.timeout());
        let request = OptimizerPlanRequest {
            locale: Some("en".into()),
            source_duration_seconds: Some(0.1),
            input_width: Some(320),
            input_height: Some(320),
            avg_fps: Some(20.0),
            fit_mode: Some("contain".into()),
            preset_strategy: Some("standard".into()),
            optimizer_goal: Some("balanced".into()),
            quality_frame_drop_interval: None,
            search_depth: Some("quick".into()),
            crop_region: None,
            selected_frames: Some(vec![1, 2]),
            base_frame_count: Some(2),
            timeline_frames: None,
        };

        let (_, prepared) = prepare_candidate_estimation_source(
            &request,
            &[],
            Path::new("ignored.gif"),
            "revision",
            UiLocale::En,
            &loader,
            &context,
            &IgnoringProgressSink,
        )
        .expect("candidate estimate preparation");

        assert_eq!(loader.prepare_count.load(AtomicOrdering::SeqCst), 1);
        assert!(prepared.decoded_bytes <= MAX_PREPARED_SEARCH_BYTES);
    }

    impl FrameSourceLoader for CountingFrameSourceLoader {
        fn prepare(
            &self,
            _request: FramePreparationRequest<'_>,
            _plan: &OptimizerPlanResponse,
            _context: &OperationContext,
            _limits: MediaLimits,
        ) -> Result<PreparedSearchSource, PipelineError> {
            self.prepare_count.fetch_add(1, AtomicOrdering::SeqCst);
            let noisy_frame = |seed: u32| {
                Arc::new(RgbaImage::from_fn(320, 320, |x, y| {
                    let mut value = x ^ y.rotate_left(16) ^ seed;
                    value ^= value >> 16;
                    value = value.wrapping_mul(0x7feb_352d);
                    value ^= value >> 15;
                    value = value.wrapping_mul(0x846c_a68b);
                    value ^= value >> 16;
                    Rgba([value as u8, (value >> 8) as u8, (value >> 16) as u8, 255])
                }))
            };
            Ok(PreparedSearchSource {
                frames: vec![
                    PreparedFrame {
                        source_frame_id: 1,
                        pixels: noisy_frame(1),
                        duration_us: 50_000,
                    },
                    PreparedFrame {
                        source_frame_id: 2,
                        pixels: noisy_frame(2),
                        duration_us: 50_000,
                    },
                ],
                base_sequence: vec![
                    ResolvedTimelineFrame {
                        source_frame_index: 0,
                        duration_us: 50_000,
                    },
                    ResolvedTimelineFrame {
                        source_frame_index: 1,
                        duration_us: 50_000,
                    },
                ],
                timing_authority: TimelineTimingAuthority::Inspected,
                base_fps: 20,
                decoded_bytes: 2 * 320 * 320 * 4,
                tool_source: "counting-loader".into(),
                tool_command: None,
                tool_detail: None,
            })
        }
    }

    struct MutatingFrameSourceLoader;

    impl FrameSourceLoader for MutatingFrameSourceLoader {
        fn prepare(
            &self,
            request: FramePreparationRequest<'_>,
            _plan: &OptimizerPlanResponse,
            _context: &OperationContext,
            _limits: MediaLimits,
        ) -> Result<PreparedSearchSource, PipelineError> {
            fs::write(request.input_path, b"source-mutated-after-preparation").map_err(
                |error| PipelineError::Io {
                    operation: "mutate source fixture",
                    message: error.to_string(),
                },
            )?;
            Ok(PreparedSearchSource {
                frames: vec![PreparedFrame {
                    source_frame_id: 1,
                    pixels: Arc::new(RgbaImage::new(1, 1)),
                    duration_us: 100_000,
                }],
                base_sequence: vec![ResolvedTimelineFrame {
                    source_frame_index: 0,
                    duration_us: 100_000,
                }],
                timing_authority: TimelineTimingAuthority::Inspected,
                base_fps: 10,
                decoded_bytes: 4,
                tool_source: "mutating-loader".into(),
                tool_command: None,
                tool_detail: None,
            })
        }
    }

    fn source_section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        let start_offset = source.find(start).expect("source section start");
        let tail = &source[start_offset..];
        let end_offset = tail.find(end).expect("source section end");
        &tail[..end_offset]
    }

    fn ffmpeg_inspection_resolution() -> ToolResolution {
        ToolResolution {
            source: "sidecar",
            command: OsString::from("ffmpeg.exe"),
            command_display: "ffmpeg.exe".into(),
            attempted_sidecar_paths: vec!["ffmpeg.exe".into()],
            fallback_reason: None,
        }
    }

    #[test]
    fn media_foundation_success_never_invokes_ffmpeg_fallback() {
        let native_calls = Cell::new(0);
        let ffmpeg_calls = Cell::new(0);
        let mut native = video_inspection_fixture(30);
        native.tool_source = Some("native".into());
        native.tool_command = None;
        native.tool_detail = Some(locale::native_video_detail(UiLocale::En, "mp4"));

        let inspection = inspect_mp4_family_with_fallback(
            || {
                native_calls.set(native_calls.get() + 1);
                Ok(native)
            },
            |_| {
                ffmpeg_calls.set(ffmpeg_calls.get() + 1);
                panic!("native success must not invoke ffmpeg")
            },
        )
        .expect("native success");

        assert_eq!(native_calls.get(), 1);
        assert_eq!(ffmpeg_calls.get(), 0);
        assert_eq!(inspection.tool_source.as_deref(), Some("native"));
        assert_eq!(inspection.fallback_reason_code, None);
    }

    #[test]
    fn media_foundation_failure_invokes_ffmpeg_once_with_the_exact_error() {
        let native_error = PipelineError::Io {
            operation: "read Media Foundation metadata",
            message: "native fixture failure".into(),
        };
        let ffmpeg_calls = Cell::new(0);
        let mut received_native_error = None;

        let inspection = inspect_mp4_family_with_fallback(
            || Err(native_error.clone()),
            |error| {
                ffmpeg_calls.set(ffmpeg_calls.get() + 1);
                received_native_error = Some(error);
                Ok(video_inspection_fixture(30))
            },
        )
        .expect("ffmpeg fallback success");

        assert!(inspection.ok);
        assert_eq!(ffmpeg_calls.get(), 1);
        assert_eq!(received_native_error, Some(native_error));
    }

    #[test]
    fn fallback_control_errors_propagate_without_inspection_annotation() {
        for expected in [
            PipelineError::Cancelled,
            PipelineError::TimedOut {
                stage: "child-process",
            },
        ] {
            let result = inspect_mp4_family_with_fallback(
                || {
                    Err(PipelineError::Io {
                        operation: "read Media Foundation metadata",
                        message: "native fixture failure".into(),
                    })
                },
                |_| Err(expected.clone()),
            );

            let Err(actual) = result else {
                panic!("fallback control error must remain an Err")
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn missing_ffmpeg_fallback_records_reason_without_claiming_decoder_use() {
        let missing = inspection_error(
            "input.mp4",
            Some("missing".into()),
            None,
            None,
            "tool-unavailable",
            locale::media_pipeline_diagnostic(UiLocale::En, "tool-missing"),
        );

        let missing = annotate_media_foundation_fallback(missing, UiLocale::En);

        assert!(!missing.ok);
        assert_eq!(missing.tool_source.as_deref(), Some("missing"));
        assert_eq!(
            missing.fallback_reason_code.as_deref(),
            Some("media-foundation-failed")
        );
        assert_eq!(missing.tool_detail, None);
    }

    #[test]
    fn fallback_source_propagates_attempt_errors_before_annotation() {
        let source = include_str!("lib.rs");
        let fallback = source_section(
            source,
            "fn inspect_mp4_family_with_fallback_and_checkpoint(",
            "fn finish_ffmpeg_inspection_result(",
        );
        let attempt = fallback
            .find("try_inspect_video_with_ffmpeg_and_checkpoint(")
            .expect("Result-returning ffmpeg attempt");
        let propagate = fallback[attempt..]
            .find("?;")
            .map(|offset| attempt + offset)
            .expect("fallback attempt error propagation");
        let annotate = fallback[propagate..]
            .find("annotate_media_foundation_fallback(")
            .map(|offset| propagate + offset)
            .expect("fallback annotation after successful attempt");
        assert!(attempt < propagate && propagate < annotate);

        let direct = source_section(
            source,
            "fn inspect_video_with_ffmpeg_and_checkpoint(",
            "fn inspect_input_media_canonical_internal(",
        );
        assert!(direct.contains("try_inspect_video_with_ffmpeg_and_checkpoint("));
        assert!(direct.contains("inspection_pipeline_error("));
    }

    #[test]
    fn media_foundation_fallback_preserves_sidecar_success_and_failure_provenance() {
        let mut success = video_inspection_fixture(30);
        success.tool_source = Some("sidecar".into());
        success.tool_command = Some("ffmpeg.exe".into());
        let success = annotate_media_foundation_fallback(success, UiLocale::En);

        assert!(success.ok);
        assert_eq!(success.tool_source.as_deref(), Some("sidecar"));
        assert_eq!(success.tool_command.as_deref(), Some("ffmpeg.exe"));
        assert_eq!(
            success.fallback_reason_code.as_deref(),
            Some("media-foundation-failed")
        );
        assert_eq!(
            success.tool_detail,
            Some(locale::media_foundation_fallback_warning(UiLocale::En))
        );

        let process_error = PipelineError::ProcessFailed {
            command: "ffmpeg.exe".into(),
            exit_code: Some(17),
            stderr: "Video: h264, yuv420p, 320x240, 30 fps".into(),
        };
        let failure = finish_ffmpeg_inspection_result(
            "input.mp4",
            UiLocale::En,
            ffmpeg_inspection_resolution(),
            Err(process_error),
        );
        let failure = annotate_media_foundation_fallback(failure, UiLocale::En);

        assert!(!failure.ok);
        assert_eq!(failure.tool_source.as_deref(), Some("sidecar"));
        assert_eq!(failure.tool_command.as_deref(), Some("ffmpeg.exe"));
        assert_eq!(
            failure.fallback_reason_code.as_deref(),
            Some("media-foundation-failed")
        );
        assert_eq!(failure.error_code.as_deref(), Some("process-failed"));
        assert_eq!(failure.reason_code, None);
        assert_eq!(
            failure.tool_detail,
            Some(locale::media_foundation_fallback_attempt_detail(
                UiLocale::En
            ))
        );
        assert_ne!(
            failure.tool_detail,
            Some(locale::media_foundation_fallback_warning(UiLocale::En))
        );
        assert_eq!(failure.width, None);
        assert_eq!(failure.height, None);
        assert_eq!(failure.codec_name, None);
    }

    #[test]
    fn only_windows_mp4_family_extensions_try_media_foundation_first() {
        for extension in ["mp4", "m4v", "mov"] {
            assert!(should_try_media_foundation(extension, true));
            assert!(!should_try_media_foundation(extension, false));
        }
        assert!(!should_try_media_foundation("webm", true));
        assert!(!should_try_media_foundation("webm", false));
    }

    #[test]
    fn direct_inspection_sources_never_claim_media_foundation_fallback() {
        let source = include_str!("lib.rs");
        for section in [
            source_section(
                source,
                "fn inspect_still_image_metadata_with_checkpoint(",
                "fn inspect_gif_metadata_internal(",
            ),
            source_section(
                source,
                "fn inspect_gif_metadata_with_checkpoint(",
                "fn inspect_apng_metadata_internal(",
            ),
            source_section(
                source,
                "fn inspect_apng_metadata_with_checkpoint(",
                "fn parse_duration_hms_to_seconds(",
            ),
            source_section(
                source,
                "fn try_inspect_mp4_family_with_media_foundation(",
                "fn inspect_mp4_family_with_fallback_and_checkpoint(",
            ),
            source_section(
                source,
                "fn finish_ffmpeg_inspection_result(",
                "fn inspect_video_with_ffmpeg(",
            ),
        ] {
            assert!(section.contains("fallback_reason_code: None"));
        }
    }

    #[test]
    fn nonzero_ffmpeg_result_is_not_parsed_even_when_stderr_looks_valid() {
        let inspection = finish_ffmpeg_inspection_result(
            "input.mp4",
            UiLocale::En,
            ffmpeg_inspection_resolution(),
            Err(PipelineError::ProcessFailed {
                command: "ffmpeg.exe".into(),
                exit_code: Some(17),
                stderr: "Duration: 00:00:01.00 Video: h264, yuv420p, 320x240, 30 fps".into(),
            }),
        );

        assert!(!inspection.ok);
        assert_eq!(inspection.error_code.as_deref(), Some("process-failed"));
        assert_eq!(inspection.format_name, None);
        assert_eq!(inspection.duration_seconds, None);
        assert_eq!(inspection.width, None);
        assert_eq!(inspection.height, None);
        assert_eq!(inspection.avg_fps, None);
    }

    #[test]
    fn successful_zero_frame_and_selected_underproduction_are_malformed() {
        assert!(matches!(
            validate_resampled_frame_stream(0),
            Err(PipelineError::MalformedProcessOutput { .. })
        ));
        assert!(matches!(
            validate_exact_selected_frame_stream(3, 2),
            Err(PipelineError::MalformedProcessOutput { .. })
        ));
        assert_eq!(validate_resampled_frame_stream(1), Ok(()));
        assert_eq!(validate_exact_selected_frame_stream(3, 3), Ok(()));
    }

    #[test]
    fn process_runner_source_maps_all_managed_callers_and_only_folder_spawns_remain() {
        let lib_source = include_str!("lib.rs");
        let runner_source = include_str!("process_runner.rs");

        let sidecar = source_section(lib_source, "fn run_resolved_command(", "fn check_tool(");
        assert!(sidecar.contains("run_captured("));
        let health = source_section(
            lib_source,
            "fn check_tool(",
            "fn lowercase_source_extension(",
        );
        assert!(health.contains("OperationContext::detached"));
        assert!(health.contains("run_sidecar_tool("));

        let inspection = source_section(
            lib_source,
            "fn try_inspect_video_with_ffmpeg_and_checkpoint(",
            "fn inspect_input_media_canonical_internal(",
        );
        assert!(inspection.contains("run_captured("));
        assert!(inspection.contains("context.unwrap_or"));

        let prepared_video = source_section(
            lib_source,
            "fn prepare_video_search_source(",
            "impl FrameSourceLoader for DefaultFrameSourceLoader",
        );
        assert_eq!(
            prepared_video.matches("stream_fixed_rgba_frames(").count(),
            1
        );
        assert!(prepared_video.contains("validate_exact_selected_frame_stream("));
        assert!(prepared_video.contains("context,"));

        let inspect_direct = source_section(
            lib_source,
            "fn inspect_input_media_internal(",
            "fn inspect_input_media_with_operation(",
        );
        let inspect_managed = source_section(
            lib_source,
            "fn inspect_input_media_with_operation(",
            "fn inspect_input_media_with_callbacks(",
        );
        assert!(inspect_direct.contains("None,"));
        assert!(inspect_managed.contains("Some(context),"));

        let static_direct = source_section(
            lib_source,
            "fn convert_static_image_to_png_internal(",
            "fn convert_static_image_to_png_with_operation(",
        );
        let static_managed = source_section(
            lib_source,
            "fn convert_static_image_to_png_with_operation(",
            "fn convert_static_image_to_png_with_callbacks(",
        );
        assert!(static_direct.contains("None,"));
        assert!(static_managed.contains("Some(context),"));

        let search_direct = source_section(
            lib_source,
            "fn run_optimizer_search_internal(",
            "fn run_optimizer_search_with_operation(",
        );
        let search_managed = source_section(
            lib_source,
            "fn run_optimizer_search_with_operation(",
            "fn run_optimizer_search_with_callbacks(",
        );
        assert!(search_direct.contains("None,"));
        assert!(search_managed.contains("Some(context),"));

        let encode_direct = source_section(
            lib_source,
            "fn encode_candidate_internal(",
            "fn convert_static_image_to_png_internal(",
        );
        let encode_propagation = source_section(
            lib_source,
            "fn encode_prepared_candidate_with_checkpoint(",
            "fn encode_candidate_internal(",
        );
        assert!(encode_direct.contains("loader.prepare("));
        assert!(encode_direct.contains("build_candidate_output_sequence("));
        assert!(encode_direct.contains("encode_prepared_candidate_with_checkpoint("));
        assert!(encode_propagation.contains("sequence: &PreparedCandidateSequence"));

        let command_new = ["Command", "::new("].concat();
        let spawn = [".", "spawn()"].concat();
        let kill = ["Child", "::kill(self)"].concat();
        let wait = ["Child", "::wait(self)"].concat();
        let output = [".", "output()"].concat();
        assert_eq!(lib_source.matches(&command_new).count(), 3);
        assert_eq!(lib_source.matches(&spawn).count(), 3);
        assert_eq!(lib_source.matches(&output).count(), 0);
        let folder_open = source_section(lib_source, "fn open_folder_path(", "pub fn run()");
        assert_eq!(folder_open.matches(&command_new).count(), 3);
        assert_eq!(folder_open.matches(&spawn).count(), 3);
        assert_eq!(runner_source.matches(&command_new).count(), 1);
        assert_eq!(runner_source.matches(&spawn).count(), 1);
        assert_eq!(runner_source.matches(&kill).count(), 1);
        assert_eq!(runner_source.matches(&wait).count(), 1);
        assert_eq!(runner_source.matches(&output).count(), 0);
        assert!(runner_source.contains("Stdio::null()"));
        assert!(runner_source.contains("sync_channel::<FrameEvent>(0)"));
    }

    #[derive(Debug)]
    struct PanicOnFrame301;

    impl<'de> Deserialize<'de> for PanicOnFrame301 {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            let value = u32::deserialize(deserializer)?;
            assert_ne!(
                value, 301,
                "the bounded visitor must not deserialize the 301st T"
            );
            Ok(Self)
        }
    }

    struct UnknownLengthFrameSequence {
        next: u32,
        size_hint: Option<usize>,
    }

    impl<'de> SeqAccess<'de> for UnknownLengthFrameSequence {
        type Error = serde::de::value::Error;

        fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
        where
            T: serde::de::DeserializeSeed<'de>,
        {
            if self.next > 301 {
                return Ok(None);
            }

            let value = self.next;
            self.next += 1;
            seed.deserialize(serde::de::value::U32Deserializer::<Self::Error>::new(value))
                .map(Some)
        }

        fn size_hint(&self) -> Option<usize> {
            self.size_hint
        }
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(prefix: &str) -> Self {
            let suffix = TEST_DIR_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "stickerfit-{prefix}-{}-{suffix}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("test temp directory should be created");
            Self { path }
        }

        fn ffmpeg_path(&self, file_name: &str) -> String {
            self.path
                .join(file_name)
                .to_string_lossy()
                .replace('\\', "/")
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_native_png_file(path: &Path, pixels: &RgbaImage) -> Result<(), PipelineError> {
        let file = File::create(path)
            .map_err(|error| pipeline_io_error("create test PNG output", error))?;
        write_native_png(file, pixels)
    }

    fn write_native_apng_file(
        path: &Path,
        frames: &[StickerFrame],
        preset: &str,
    ) -> Result<(), PipelineError> {
        let file = File::create(path)
            .map_err(|error| pipeline_io_error("create test APNG output", error))?;
        write_native_apng(file, frames, preset)
    }

    fn revisioned_placeholder(prefix: &str) -> (TestDir, String, String) {
        let test_dir = TestDir::new(prefix);
        let input_path = test_dir.path.join("input.png");
        fs::write(&input_path, b"placeholder").expect("revision placeholder must be written");
        let identity = SourceIdentity::from_path(&input_path, MediaLimits::default())
            .expect("revision placeholder identity must be created");
        (
            test_dir,
            input_path.to_string_lossy().into_owned(),
            identity.revision(),
        )
    }

    fn build_test_candidate(id: &str, fps: u32) -> CandidatePreview {
        CandidatePreview {
            id: id.into(),
            rank: 1,
            duration_seconds: 1.0,
            fps,
            content_scale: 1.0,
            preset: "standard".into(),
            fit_mode: "contain".into(),
            score: 1.0,
            source_similarity_score: 1.0,
            relative_size_factor: relative_size_factor_for(1, 1.0, "standard"),
            summary: "test candidate".into(),
            frame_sample_step: 1,
        }
    }

    fn build_selected_output(
        candidate_id: &str,
        rank: usize,
        size_bytes: u64,
        source_similarity_score: f64,
    ) -> SelectedEncodeOutput {
        SelectedEncodeOutput {
            candidate_id: candidate_id.into(),
            rank,
            duration_seconds: 1.0,
            size_bytes,
            source_similarity_score,
        }
    }

    fn build_test_attempt(candidate_id: &str, within_limit: bool) -> SearchAttemptResult {
        SearchAttemptResult {
            candidate_id: candidate_id.into(),
            canonical_candidate_id: candidate_id.into(),
            equivalent_to_candidate_id: None,
            rank: 1,
            duration_seconds: 1.0,
            fps: 12,
            content_scale: 1.0,
            preset: "standard".into(),
            fit_mode: "contain".into(),
            score: 1.0,
            source_similarity_score: 1.0,
            summary: "test candidate".into(),
            skipped: false,
            within_limit,
            output_path: None,
            size_bytes: Some(7),
            elapsed_ms: Some(1),
            tool_source: Some("native".into()),
            tool_command: None,
            tool_detail: None,
            warnings: Vec::new(),
            error_code: None,
            reason_code: None,
            error_message: None,
        }
    }

    fn build_pending_selection(
        directory: &Path,
        source_path: &Path,
        candidate_id: &str,
        size_bytes: u64,
    ) -> PendingSelectedEncodeOutput {
        let mut pending_output = PendingOutput::new(directory, source_path, candidate_id, "png")
            .expect("pending candidate must be created");
        pending_output
            .writer()
            .write_all(b"encoded")
            .expect("candidate bytes must be written");
        PendingSelectedEncodeOutput {
            selected: SelectedEncodeOutput {
                candidate_id: candidate_id.into(),
                rank: 1,
                duration_seconds: 1.0,
                size_bytes,
                source_similarity_score: 1.0,
            },
            pending_output,
        }
    }

    #[test]
    fn native_png_and_apng_writers_accept_in_memory_targets() {
        let pixels = RgbaImage::from_pixel(2, 2, Rgba([12, 34, 56, 255]));
        let mut png_bytes = Vec::new();
        write_native_png(&mut png_bytes, &pixels).expect("PNG must encode into memory");
        assert!(png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));

        let frames = vec![
            StickerFrame {
                pixels: pixels.clone(),
                duration_us: 100_000,
            },
            StickerFrame {
                pixels,
                duration_us: 100_000,
            },
        ];
        let mut apng_bytes = Vec::new();
        write_native_apng(&mut apng_bytes, &frames, "standard")
            .expect("APNG must encode into memory");
        assert!(apng_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn native_png_writer_propagates_write_failures() {
        struct FailingWriter;

        impl Write for FailingWriter {
            fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected write failure"))
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("injected flush failure"))
            }
        }

        let pixels = RgbaImage::from_pixel(1, 1, Rgba([12, 34, 56, 255]));
        let error = write_native_png(FailingWriter, &pixels)
            .expect_err("writer failure must abort native PNG encoding");

        assert!(matches!(error, PipelineError::Io { .. }));
    }

    #[test]
    fn static_publication_source_change_before_commit_leaves_no_output_or_temp_file() {
        let directory = tempfile::tempdir().expect("temp directory must be created");
        let source_path = directory.path().join("source.png");
        fs::write(&source_path, b"source").expect("source fixture must be written");
        let source_identity = SourceIdentity::from_path(&source_path, MediaLimits::default())
            .expect("source identity must be created");
        let mut output = PendingOutput::new(directory.path(), &source_path, "candidate", "png")
            .expect("pending output must be created");
        output
            .writer()
            .write_all(b"encoded")
            .expect("candidate bytes must be written");
        fs::remove_file(&source_path).expect("source fixture must be removed");

        let error = commit_output_after_source_validation(output, Some(&source_identity))
            .expect_err("source change must prevent publication");

        assert_eq!(error, PipelineError::SourceChanged);
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("output directory must be readable")
                .count(),
            0
        );
    }

    #[test]
    fn optimizer_publication_commits_only_winner_and_only_its_attempt_path() {
        let directory = tempfile::tempdir().expect("temp directory must be created");
        let source_path = directory.path().join("source.png");
        fs::write(&source_path, b"source").expect("source fixture must be written");
        let source_identity = SourceIdentity::from_path(&source_path, MediaLimits::default())
            .expect("source identity must be created");
        let winner = build_pending_selection(directory.path(), &source_path, "winner", 7);
        let oversize = build_pending_selection(directory.path(), &source_path, "oversize", 999_999);
        let mut attempts = vec![
            build_test_attempt("winner", true),
            build_test_attempt("oversize", false),
        ];

        let published = publish_optimizer_selection(
            &source_identity,
            false,
            Some(winner),
            Some(oversize),
            &mut attempts,
        )
        .expect("winner must publish")
        .expect("winner must be selected");

        assert_eq!(published.selected.candidate_id, "winner");
        assert_eq!(
            attempts[0].output_path.as_deref(),
            Some(published.output_path.as_str())
        );
        assert_eq!(attempts[1].output_path, None);
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("output directory must be readable")
                .count(),
            2,
            "only the source and selected final output may remain"
        );
    }

    #[test]
    fn optimizer_publication_commits_smallest_oversize_when_no_winner_exists() {
        let directory = tempfile::tempdir().expect("temp directory must be created");
        let source_path = directory.path().join("source.png");
        fs::write(&source_path, b"source").expect("source fixture must be written");
        let source_identity = SourceIdentity::from_path(&source_path, MediaLimits::default())
            .expect("source identity must be created");
        let oversize = build_pending_selection(directory.path(), &source_path, "oversize", 999_999);
        let mut attempts = vec![build_test_attempt("oversize", false)];

        let published = publish_optimizer_selection(
            &source_identity,
            false,
            None,
            Some(oversize),
            &mut attempts,
        )
        .expect("oversize selection must publish")
        .expect("oversize candidate must be selected");

        assert_eq!(published.selected.candidate_id, "oversize");
        assert_eq!(
            attempts[0].output_path.as_deref(),
            Some(published.output_path.as_str())
        );
    }

    #[test]
    fn optimizer_source_change_before_selected_commit_leaves_no_output_or_temp_file() {
        let directory = tempfile::tempdir().expect("temp directory must be created");
        let source_path = directory.path().join("source.png");
        fs::write(&source_path, b"source").expect("source fixture must be written");
        let source_identity = SourceIdentity::from_path(&source_path, MediaLimits::default())
            .expect("source identity must be created");
        let winner = build_pending_selection(directory.path(), &source_path, "winner", 7);
        let mut attempts = vec![build_test_attempt("winner", true)];
        fs::remove_file(&source_path).expect("source fixture must be removed");

        let error =
            publish_optimizer_selection(&source_identity, false, Some(winner), None, &mut attempts)
                .expect_err("source change must prevent selected publication");

        assert_eq!(error, PipelineError::SourceChanged);
        assert_eq!(attempts[0].output_path, None);
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("output directory must be readable")
                .count(),
            0
        );
    }

    #[test]
    fn later_candidate_error_drops_all_retained_optimizer_outputs_without_artifacts() {
        fn retain_candidates_then_fail(
            directory: &Path,
            source_path: &Path,
        ) -> Result<(), PipelineError> {
            let _best_within_limit =
                Some(build_pending_selection(directory, source_path, "winner", 7));
            let _smallest_oversize = Some(build_pending_selection(
                directory,
                source_path,
                "oversize",
                999_999,
            ));
            Err(PipelineError::InvalidRequest {
                reason: "encode-failed",
            })
        }

        let directory = tempfile::tempdir().expect("temp directory must be created");
        let source_path = directory.path().join("source.png");
        fs::write(&source_path, b"source").expect("source fixture must be written");

        let error = retain_candidates_then_fail(directory.path(), &source_path)
            .expect_err("later candidate error must abort the optimizer scope");

        assert_eq!(
            error,
            PipelineError::InvalidRequest {
                reason: "encode-failed"
            }
        );
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("output directory must be readable")
                .count(),
            1,
            "only the untouched source may remain after retained guards drop"
        );
    }

    #[test]
    fn postcheck_does_not_override_a_precommit_validated_publication() {
        let directory = tempfile::tempdir().expect("temp directory must be created");
        let source_path = directory.path().join("source.png");
        fs::write(&source_path, b"source").expect("source fixture must be written");
        let source_identity = SourceIdentity::from_path(&source_path, MediaLimits::default())
            .expect("source identity must be created");
        fs::remove_file(&source_path).expect("source fixture must be removed");

        let mut conversion = static_conversion_pipeline_error(
            &PipelineError::InvalidRequest {
                reason: "encode-failed",
            },
            UiLocale::En,
        );
        conversion.ok = true;
        conversion.output_path = Some("published.png".into());
        let conversion = finalize_static_conversion_source_unless_published(
            &source_identity,
            conversion,
            UiLocale::En,
        );
        assert!(conversion.ok);
        assert_eq!(conversion.output_path.as_deref(), Some("published.png"));

        let mut search = optimizer_search_pipeline_error(
            UiLocale::En,
            Vec::new(),
            PipelineError::InvalidRequest {
                reason: "encode-failed",
            },
        );
        search.best_output_path = Some("published.png".into());
        let search = finalize_optimizer_search_source_unless_published(
            &source_identity,
            search,
            UiLocale::En,
        );
        assert_eq!(search.best_output_path.as_deref(), Some("published.png"));
        assert_ne!(search.error_code.as_deref(), Some("source-changed"));
    }

    #[test]
    fn decoder_boundary_maps_panics_to_malformed_media() {
        let error = run_decoder_boundary("png", || -> Result<(), PipelineError> {
            panic!("decoder panic detail must not cross the wire")
        })
        .expect_err("decoder panics must be contained");

        assert!(matches!(
            error,
            PipelineError::MalformedInput {
                format: "png",
                ref reason,
            } if reason == "decoder panicked"
        ));
    }

    #[test]
    fn animation_decode_preflights_aggregate_bytes_before_advancing_iterator() {
        let next_calls = Cell::new(0usize);
        let frames = std::iter::from_fn(|| {
            next_calls.set(next_calls.get() + 1);
            Some(Ok(image::Frame::new(RgbaImage::new(1, 1))))
        });
        let limits = MediaLimits {
            max_total_decoded_bytes: 3,
            ..MediaLimits::default()
        };

        let error = collect_decoded_animation_frames("gif", frames, 4, 1, limits, || Ok(()))
            .expect_err("aggregate budget must be checked before decoding a frame");

        assert_eq!(next_calls.get(), 0, "iterator.next() must not be called");
        assert!(matches!(
            error,
            PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: 3,
                actual: 4,
            }
        ));
    }

    #[test]
    fn animation_decode_accepts_exact_aggregate_budget_without_false_eof_failure() {
        let frames = vec![Ok(image::Frame::new(RgbaImage::new(1, 1)))];
        let limits = MediaLimits {
            max_total_decoded_bytes: 4,
            ..MediaLimits::default()
        };

        let decoded =
            collect_decoded_animation_frames("gif", frames.into_iter(), 4, 1, limits, || Ok(()))
                .expect("one 4-byte frame must fit an exact 4-byte aggregate budget");

        assert_eq!(decoded.len(), 1);
    }

    #[test]
    fn wire_diagnostics_expose_only_safe_stable_detail_tags() {
        let parser_error = malformed_png("CRC mismatch for \u{1b}[31msecret chunk");
        let panic_error = PipelineError::MalformedInput {
            format: "png",
            reason: "decoder panicked".into(),
        };
        let secret_path = r"C:\Users\private\source.mp4";
        let process_error = PipelineError::ProcessFailed {
            command: secret_path.into(),
            exit_code: Some(1),
            stderr: format!("failed while reading {secret_path}"),
        };

        let parser_diagnostic = pipeline_error_diagnostic(&parser_error, UiLocale::En);
        let panic_diagnostic = pipeline_error_diagnostic(&panic_error, UiLocale::En);
        let process_diagnostic = pipeline_error_diagnostic(&process_error, UiLocale::En);

        assert!(parser_diagnostic.contains("[png-parser-crc]"));
        assert!(panic_diagnostic.contains("[decoder-panic]"));
        assert_ne!(parser_diagnostic, panic_diagnostic);
        for diagnostic in [parser_diagnostic, panic_diagnostic, process_diagnostic] {
            assert!(!diagnostic.contains(secret_path));
            assert!(!diagnostic.contains('\u{1b}'));
        }
        assert_eq!(
            ffmpeg_inspection_failure_diagnostic(UiLocale::En, false),
            locale::media_pipeline_diagnostic(UiLocale::En, "malformed-media")
        );
    }

    #[test]
    fn decode_still_rgba_image_applies_dimension_limits() {
        let test_dir = TestDir::new("still-decode-limits");
        let input_path = test_dir.path.join("input.png");
        DynamicImage::ImageRgba8(RgbaImage::new(2, 1))
            .save_with_format(&input_path, ImageFormat::Png)
            .expect("still fixture must be encoded");
        let limits = MediaLimits {
            max_dimension: 1,
            ..MediaLimits::default()
        };

        let error =
            decode_still_rgba_image(input_path.to_string_lossy().as_ref(), limits, || Ok(()))
                .expect_err("still decoder must reject oversized dimensions");

        assert!(matches!(
            error,
            PipelineError::LimitExceeded {
                resource: "image-dimensions",
                limit: 1,
                actual: 2,
            }
        ));
    }

    #[test]
    fn decode_still_rgba_image_rejects_pixel_budget_from_header_before_pixel_decode() {
        let test_dir = TestDir::new("still-pixel-preflight");
        let input_path = test_dir.path.join("input.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr(10_000, 4_001));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
        fs::write(&input_path, bytes).expect("oversized PNG header fixture must be written");

        let error = decode_still_rgba_image(
            input_path.to_string_lossy().as_ref(),
            MediaLimits::default(),
            || Ok(()),
        )
        .expect_err("pixel budget must be rejected from dimensions alone");

        assert!(matches!(
            error,
            PipelineError::LimitExceeded {
                resource: "image-pixels",
                limit: 40_000_000,
                actual: 40_010_000,
            }
        ));
    }

    #[test]
    fn decode_gif_animation_frames_enforces_frame_limit() {
        let test_dir = TestDir::new("gif-frame-limit");
        let input_path = test_dir.path.join("input.gif");
        let file = File::create(&input_path).expect("GIF fixture must be created");
        image::codecs::gif::GifEncoder::new(file)
            .encode_frames(
                vec![
                    image::Frame::new(RgbaImage::new(1, 1)),
                    image::Frame::new(RgbaImage::new(1, 1)),
                ]
                .into_iter(),
            )
            .expect("GIF fixture must be encoded");
        let limits = MediaLimits {
            max_frame_count: 1,
            ..MediaLimits::default()
        };

        let error =
            decode_gif_animation_frames(input_path.to_string_lossy().as_ref(), limits, || Ok(()))
                .expect_err("GIF decoder must enforce the frame limit");

        assert!(matches!(
            error,
            PipelineError::LimitExceeded {
                resource: "frame-count",
                limit: 1,
                actual: 2,
            }
        ));
    }

    #[test]
    fn decode_apng_animation_frames_enforces_decoded_byte_budget() {
        let test_dir = TestDir::new("apng-decoded-byte-limit");
        let input_path = test_dir.path.join("input.png");
        let frames = vec![
            StickerFrame {
                pixels: RgbaImage::new(1, 1),
                duration_us: 100_000,
            },
            StickerFrame {
                pixels: RgbaImage::new(1, 1),
                duration_us: 100_000,
            },
        ];
        write_native_apng_file(&input_path, &frames, "standard")
            .expect("APNG fixture must be encoded");
        let limits = MediaLimits {
            max_total_decoded_bytes: 7,
            ..MediaLimits::default()
        };

        let error =
            decode_apng_animation_frames(input_path.to_string_lossy().as_ref(), limits, || Ok(()))
                .expect_err("APNG decoder must enforce the aggregate decoded byte budget");

        assert!(matches!(
            error,
            PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: 7,
                actual: 8,
            }
        ));
    }

    #[test]
    fn animation_decode_checkpoint_cancellation_is_preserved() {
        let frames = vec![
            Ok(image::Frame::new(RgbaImage::new(1, 1))),
            Ok(image::Frame::new(RgbaImage::new(1, 1))),
        ];
        let mut checkpoints = 0;

        let error = collect_decoded_animation_frames(
            "gif",
            frames.into_iter(),
            4,
            2,
            MediaLimits::default(),
            || {
                checkpoints += 1;
                if checkpoints == 2 {
                    Err(PipelineError::Cancelled)
                } else {
                    Ok(())
                }
            },
        )
        .expect_err("checkpoint cancellation must stop frame collection");

        assert_eq!(error, PipelineError::Cancelled);
    }

    #[test]
    fn raw_rgba_output_limits_reject_frame_count_and_total_bytes() {
        let frame_error = validate_raw_rgba_output(
            1,
            1,
            8,
            None,
            MediaLimits {
                max_frame_count: 1,
                ..MediaLimits::default()
            },
        )
        .expect_err("raw output must enforce frame count");
        assert!(matches!(
            frame_error,
            PipelineError::LimitExceeded {
                resource: "frame-count",
                limit: 1,
                actual: 2,
            }
        ));

        let byte_error = validate_raw_rgba_output(
            1,
            1,
            8,
            None,
            MediaLimits {
                max_total_decoded_bytes: 7,
                ..MediaLimits::default()
            },
        )
        .expect_err("raw output must enforce aggregate decoded bytes");
        assert!(matches!(
            byte_error,
            PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: 7,
                actual: 8,
            }
        ));
    }

    #[test]
    fn native_animation_crop_uses_decoded_dimensions_not_request_dimensions() {
        let test_dir = TestDir::new("native-decoded-crop-dimensions");
        let input_path = test_dir.path.join("input.apng");
        write_native_apng_file(
            &input_path,
            &[StickerFrame {
                pixels: RgbaImage::new(4, 2),
                duration_us: 100_000,
            }],
            "standard",
        )
        .expect("native crop fixture must be encoded");
        let candidate = build_test_candidate("decoded-crop", 10);
        let crop = CropRegion {
            x: 0.5,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        };

        let result = encode_candidate_internal(
            input_path.to_string_lossy().as_ref(),
            Some(test_dir.path.to_string_lossy().as_ref()),
            UiLocale::En,
            Some(&crop),
            Some(400),
            Some(400),
            &candidate,
            None,
            None,
            None,
        )
        .expect("native crop encode must succeed");
        let output_path = result
            .pending_output
            .commit()
            .expect("native crop output must commit");
        let frames = decode_apng_animation_frames(
            output_path.to_string_lossy().as_ref(),
            MediaLimits::default(),
            || Ok(()),
        )
        .expect("native crop output must decode");

        assert_eq!(frames[0].pixels.dimensions(), (2, 2));
    }

    #[test]
    fn validate_source_revision_rejects_missing_and_stale_revisions() {
        let test_dir = TestDir::new("source-revision-validation");
        let input_path = test_dir.path.join("input.png");
        fs::write(&input_path, b"first").expect("source fixture must be written");

        let missing = validate_source_revision(&input_path, None, MediaLimits::default())
            .expect_err("missing source revision must be rejected");
        assert_eq!(missing, PipelineError::InvalidRequestWithoutReason);
        assert_eq!(missing.code(), "invalid-request");
        assert_eq!(missing.reason_code(), None);

        let assert_response_contract = |error: &PipelineError, expected_code: &str| {
            let conversion = static_conversion_pipeline_error(error, UiLocale::En);
            assert_eq!(conversion.error_code.as_deref(), Some(expected_code));
            assert_eq!(conversion.reason_code, None);

            let single = frame_preview_pipeline_error(error, UiLocale::En);
            assert_eq!(single.error_code.as_deref(), Some(expected_code));
            assert_eq!(single.reason_code, None);

            let bulk = frame_previews_pipeline_error(error, UiLocale::En);
            assert_eq!(bulk.error_code.as_deref(), Some(expected_code));
            assert_eq!(bulk.reason_code, None);

            let search = optimizer_search_pipeline_error(UiLocale::En, Vec::new(), error.clone());
            assert_eq!(search.error_code.as_deref(), Some(expected_code));
            assert_eq!(search.reason_code, None);
        };
        assert_response_contract(&missing, "invalid-request");
        let missing_search: OptimizerSearchRequest = serde_json::from_value(serde_json::json!({
            "inputPath": input_path.to_string_lossy()
        }))
        .expect("missing-revision search fixture must deserialize");
        let missing_search_response = run_optimizer_search_internal(missing_search, UiLocale::En);
        assert_eq!(
            missing_search_response.error_code.as_deref(),
            Some("invalid-request")
        );
        assert_eq!(missing_search_response.reason_code, None);

        let identity =
            crate::media_limits::SourceIdentity::from_path(&input_path, MediaLimits::default())
                .expect("source identity must be created");
        validate_source_revision(
            &input_path,
            Some(identity.revision().as_str()),
            MediaLimits::default(),
        )
        .expect("matching source revision must be accepted");

        fs::write(&input_path, b"changed-source").expect("source fixture must be changed");
        let stale = validate_source_revision(
            &input_path,
            Some(identity.revision().as_str()),
            MediaLimits::default(),
        )
        .expect_err("stale source revision must be rejected");
        assert_eq!(stale, PipelineError::SourceChanged);
        assert_eq!(stale.code(), "source-changed");
        assert_eq!(stale.reason_code(), None);
        assert_response_contract(&stale, "source-changed");
        let stale_search: OptimizerSearchRequest = serde_json::from_value(serde_json::json!({
            "inputPath": input_path.to_string_lossy(),
            "sourceRevision": identity.revision()
        }))
        .expect("stale-revision search fixture must deserialize");
        let stale_search_response = run_optimizer_search_internal(stale_search, UiLocale::En);
        assert_eq!(
            stale_search_response.error_code.as_deref(),
            Some("source-changed")
        );
        assert_eq!(stale_search_response.reason_code, None);
    }

    #[test]
    fn source_postcheck_treats_disappearance_as_changed_and_overrides_work_failure() {
        let test_dir = TestDir::new("source-postcheck-precedence");
        let input_path = test_dir.path.join("input.png");
        fs::write(&input_path, b"source").expect("source fixture must be written");
        let identity = SourceIdentity::from_path(&input_path, MediaLimits::default())
            .expect("source identity must be created");
        fs::remove_file(&input_path).expect("source fixture must be removed");

        let direct_error = ensure_source_unchanged(&identity, MediaLimits::default())
            .expect_err("a disappeared source must be reported as changed");
        assert_eq!(direct_error, PipelineError::SourceChanged);

        let result =
            finalize_source_checked(&identity, "work-failed", MediaLimits::default(), |error| {
                error.code()
            });
        assert_eq!(result, "source-changed");

        let stale_revision = validate_source_revision(
            &input_path,
            Some(identity.revision().as_str()),
            MediaLimits::default(),
        )
        .expect_err("a supplied revision whose source disappeared must be stale");
        assert_eq!(stale_revision, PipelineError::SourceChanged);

        let work_error = PipelineError::MalformedInput {
            format: "test",
            reason: "work failed".into(),
        };
        for work_reported_success in [false, true] {
            let mut conversion = static_conversion_pipeline_error(&work_error, UiLocale::En);
            conversion.ok = work_reported_success;
            assert_eq!(
                finalize_static_conversion_source(&identity, conversion, UiLocale::En)
                    .error_code
                    .as_deref(),
                Some("source-changed")
            );

            let mut search =
                optimizer_search_pipeline_error(UiLocale::En, Vec::new(), work_error.clone());
            search.ok = work_reported_success;
            assert_eq!(
                finalize_optimizer_search_source(&identity, search, UiLocale::En)
                    .error_code
                    .as_deref(),
                Some("source-changed")
            );

            let mut single = frame_preview_pipeline_error(&work_error, UiLocale::En);
            single.ok = work_reported_success;
            assert_eq!(
                finalize_frame_preview_source(&identity, single, UiLocale::En)
                    .error_code
                    .as_deref(),
                Some("source-changed")
            );

            let mut batch = frame_previews_pipeline_error(&work_error, UiLocale::En);
            batch.ok = work_reported_success;
            assert_eq!(
                finalize_frame_previews_source(&identity, batch, UiLocale::En)
                    .error_code
                    .as_deref(),
                Some("source-changed")
            );
        }
    }

    #[test]
    fn request_frame_counts_are_capped_before_collection_or_range_allocation() {
        let selected = vec![1u32; 301];
        assert!(matches!(
            resolve_frame_selection(Some(&selected), Some(300)),
            Err("invalid-frame-selection")
        ));
        assert!(matches!(
            resolve_frame_selection(None, Some(u32::MAX)),
            Err("invalid-frame-selection")
        ));

        let timeline = (0..301)
            .map(|_| EditedTimelineFrame {
                source_frame_id: 1,
                duration_us: 100_000,
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            resolve_timeline_frames(Some(&timeline), Some(300)),
            Err("invalid-frame-selection")
        ));

        let preview =
            extract_frame_previews_internal("missing.gif", &vec![1u32; 301], UiLocale::En);
        assert!(!preview.ok);
        assert_eq!(preview.error_code.as_deref(), Some("media-frame-limit"));
        assert_eq!(preview.reason_code, None);

        let zero_id = extract_frame_previews_internal("missing.gif", &[0, 1], UiLocale::En);
        assert!(!zero_id.ok);
        assert_eq!(zero_id.error_code.as_deref(), Some("invalid-request"));
        assert_eq!(
            zero_id.reason_code.as_deref(),
            Some("invalid-frame-selection")
        );
    }

    fn video_inspection_fixture(estimated_frames: u64) -> MediaInspection {
        MediaInspection {
            ok: true,
            input_path: "canonical-input.mp4".into(),
            source_revision: None,
            tool_source: Some("sidecar".into()),
            tool_command: Some("ffmpeg".into()),
            tool_detail: Some("fixture".into()),
            fallback_reason_code: None,
            format_name: Some("mp4".into()),
            duration_seconds: Some(10.0),
            size_bytes: Some(10),
            width: Some(320),
            height: Some(320),
            codec_name: Some("h264".into()),
            pixel_format: Some("yuv420p".into()),
            avg_fps: Some(30.0),
            frame_rate_label: Some("30.00".into()),
            estimated_frames: Some(estimated_frames),
            frame_durations_seconds: None,
            warnings: Vec::new(),
            is_static_image: false,
            can_convert_to_png: false,
            error_code: None,
            reason_code: None,
            error_message: None,
        }
    }

    #[test]
    fn desktop_inspection_accepts_300_frames_and_rejects_301_before_revision_export() {
        let test_dir = TestDir::new("desktop-inspection-frame-limit");
        let input_path = test_dir.path.join("input.mp4");
        fs::write(&input_path, b"fixture").expect("inspection identity fixture must be written");
        let identity = SourceIdentity::from_path(&input_path, MediaLimits::default())
            .expect("inspection identity must be created");
        let display_path = input_path.to_string_lossy();
        let expected_revision = identity.revision();

        let accepted = enforce_desktop_inspection_frame_limit(
            apply_inspection_source_revision(
                &identity,
                &display_path,
                video_inspection_fixture(300),
            ),
            UiLocale::En,
        );
        assert!(accepted.ok);
        assert_eq!(accepted.estimated_frames, Some(300));
        assert_eq!(
            accepted.source_revision.as_deref(),
            Some(expected_revision.as_str())
        );

        let rejected = enforce_desktop_inspection_frame_limit(
            apply_inspection_source_revision(
                &identity,
                &display_path,
                video_inspection_fixture(301),
            ),
            UiLocale::En,
        );
        assert!(!rejected.ok);
        assert_eq!(rejected.error_code.as_deref(), Some("media-frame-limit"));
        assert_eq!(rejected.reason_code, None);
        assert_eq!(rejected.source_revision, None);
        assert!(rejected
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains("limit:frame-count:301>300")));
    }

    #[test]
    fn media_inspection_sets_revision_only_for_success() {
        let test_dir = TestDir::new("inspection-source-revision");
        let valid_path = test_dir.path.join("input.png");
        DynamicImage::ImageRgba8(RgbaImage::new(2, 2))
            .save_with_format(&valid_path, ImageFormat::Png)
            .expect("inspection fixture must be encoded");

        let success =
            inspect_input_media_internal(valid_path.to_string_lossy().as_ref(), UiLocale::En);
        assert!(success.ok);
        assert!(success
            .source_revision
            .as_deref()
            .is_some_and(|value| !value.is_empty()));

        let failed_path = test_dir.path.join("unsupported.bin");
        fs::write(&failed_path, b"unsupported").expect("failed fixture must be written");
        let failure =
            inspect_input_media_internal(failed_path.to_string_lossy().as_ref(), UiLocale::En);
        assert!(!failure.ok);
        assert_eq!(failure.source_revision, None);
    }

    #[test]
    fn path_backed_request_deserialization_keeps_missing_revision_for_validation() {
        let conversion: StaticImageConversionRequest = serde_json::from_value(serde_json::json!({
            "inputPath": "input.png"
        }))
        .expect("legacy request must reach command validation");
        assert_eq!(conversion.source_revision, None);

        let search: OptimizerSearchRequest = serde_json::from_value(serde_json::json!({
            "inputPath": "input.png"
        }))
        .expect("legacy search request must reach command validation");
        assert_eq!(search.source_revision, None);
    }

    #[test]
    fn request_deserialization_stops_frame_sequences_at_the_hard_cap() {
        let selected_frames = vec![1u32; 301];
        let search = serde_json::json!({
            "inputPath": "input.gif",
            "selectedFrames": selected_frames,
            "baseFrameCount": 300
        });
        assert!(
            serde_json::from_value::<OptimizerSearchRequest>(search).is_err(),
            "search selectedFrames must fail while deserializing the 301st item"
        );

        let timeline_frames = (0..301)
            .map(|_| {
                serde_json::json!({
                    "sourceFrameId": 1,
                    "durationUs": 100_000
                })
            })
            .collect::<Vec<_>>();
        let plan = serde_json::json!({
            "timelineFrames": timeline_frames,
            "baseFrameCount": 300
        });
        assert!(
            serde_json::from_value::<OptimizerPlanRequest>(plan).is_err(),
            "plan timelineFrames must fail while deserializing the 301st item"
        );

        assert!(
            serde_json::from_value::<BoundedFrameIds>(serde_json::json!(vec![1u32; 301])).is_err()
        );

        let selected_at_limit = vec![1u32; 300];
        assert!(
            serde_json::from_value::<OptimizerSearchRequest>(serde_json::json!({
                "inputPath": "input.gif",
                "selectedFrames": selected_at_limit,
                "baseFrameCount": 300
            }))
            .is_ok()
        );
        let timeline_at_limit = (0..300)
            .map(|_| {
                serde_json::json!({
                    "sourceFrameId": 1,
                    "durationUs": 100_000
                })
            })
            .collect::<Vec<_>>();
        assert!(
            serde_json::from_value::<OptimizerPlanRequest>(serde_json::json!({
                "timelineFrames": timeline_at_limit,
                "baseFrameCount": 300
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<BoundedFrameIds>(serde_json::json!(vec![1u32; 300])).is_ok()
        );

        for base_frame_count in [300u32] {
            assert!(
                serde_json::from_value::<OptimizerPlanRequest>(serde_json::json!({
                    "baseFrameCount": base_frame_count
                }))
                .is_ok()
            );
            assert!(
                serde_json::from_value::<OptimizerSearchRequest>(serde_json::json!({
                    "inputPath": "input.gif",
                    "baseFrameCount": base_frame_count
                }))
                .is_ok()
            );
        }
        assert!(
            serde_json::from_value::<OptimizerPlanRequest>(serde_json::json!({
                "baseFrameCount": 301
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<OptimizerSearchRequest>(serde_json::json!({
                "inputPath": "input.gif",
                "baseFrameCount": 301
            }))
            .is_err()
        );
    }

    #[test]
    fn bounded_sequence_probes_the_301st_item_without_deserializing_t() {
        for size_hint in [None, Some(0)] {
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                BoundedVecVisitor::<PanicOnFrame301>(PhantomData)
                    .visit_seq(UnknownLengthFrameSequence { next: 1, size_hint })
            }));

            let result = outcome.expect("the 301st item must not deserialize PanicOnFrame301");
            assert!(
                result.is_err(),
                "an unknown or false size hint must still reject item 301"
            );
        }
    }

    #[test]
    fn still_decoder_preflight_rejects_native_and_conversion_allocation_peaks() {
        let limits = MediaLimits::default();
        let native_high_depth = validate_still_decoder_allocation(
            10_000,
            4_000,
            image::ColorType::Rgba16,
            320_000_000,
            limits,
        )
        .expect_err("40M-pixel RGBA16 must be rejected before native allocation");
        assert!(matches!(
            native_high_depth,
            PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: 167_772_160,
                actual: 320_000_000,
            }
        ));

        let conversion_peak = validate_still_decoder_allocation(
            10_000,
            2_500,
            image::ColorType::Rgb8,
            75_000_000,
            limits,
        )
        .expect_err("native RGB plus RGBA conversion must share the decoded-byte budget");
        assert!(matches!(
            conversion_peak,
            PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: 167_772_160,
                actual: 175_000_000,
            }
        ));

        validate_still_decoder_allocation(
            10_000,
            4_000,
            image::ColorType::Rgba8,
            160_000_000,
            limits,
        )
        .expect("an existing RGBA8 output must not be double-counted");
    }

    #[cfg(target_os = "windows")]
    fn named_rgba(color: &str) -> Rgba<u8> {
        match color {
            "red" => Rgba([255, 0, 0, 255]),
            "green" => Rgba([0, 255, 0, 255]),
            "blue" => Rgba([0, 0, 255, 255]),
            "yellow" => Rgba([255, 255, 0, 255]),
            "magenta" => Rgba([255, 0, 255, 255]),
            "cyan" => Rgba([0, 255, 255, 255]),
            "white" => Rgba([255, 255, 255, 255]),
            _ => Rgba([0, 0, 0, 255]),
        }
    }

    #[cfg(target_os = "windows")]
    fn create_animation(test_dir: &TestDir, file_name: &str, colors: &[&str]) -> String {
        let input_path = test_dir.ffmpeg_path(file_name);
        let frames = colors
            .iter()
            .map(|color| StickerFrame {
                pixels: RgbaImage::from_pixel(48, 48, named_rgba(color)),
                duration_us: 100_000,
            })
            .collect::<Vec<_>>();
        write_native_apng_file(Path::new(&input_path), &frames, "standard")
            .expect("native APNG writer should create the animation source");

        input_path
    }

    #[cfg(target_os = "windows")]
    fn create_seven_frame_animation(test_dir: &TestDir) -> String {
        create_animation(
            test_dir,
            "input.apng",
            &["red", "green", "blue", "yellow", "magenta", "cyan", "white"],
        )
    }

    #[cfg(target_os = "windows")]
    fn create_three_frame_animation(test_dir: &TestDir) -> String {
        create_animation(
            test_dir,
            "three-frame-input.apng",
            &["red", "green", "blue"],
        )
    }

    #[cfg(target_os = "windows")]
    fn create_static_image_with_size(
        test_dir: &TestDir,
        file_name: &str,
        color: &str,
        width: u32,
        height: u32,
    ) -> String {
        let output = test_dir.ffmpeg_path(file_name);
        let image = RgbaImage::from_pixel(width, height, named_rgba(color));

        match lowercase_source_extension(file_name).as_deref() {
            Some("jpg") | Some("jpeg") => {
                DynamicImage::ImageRgba8(image)
                    .save_with_format(&output, ImageFormat::Jpeg)
                    .expect("native image encoder should create a jpg test source");
            }
            _ => {
                write_native_png_file(Path::new(&output), &image)
                    .expect("native image encoder should create a png test source");
            }
        }

        output
    }

    #[cfg(target_os = "windows")]
    fn create_static_image(test_dir: &TestDir, file_name: &str, color: &str) -> String {
        create_static_image_with_size(test_dir, file_name, color, 48, 48)
    }

    #[cfg(target_os = "windows")]
    fn create_variable_duration_animation(test_dir: &TestDir) -> String {
        let output_path = test_dir.ffmpeg_path("variable-duration.apng");
        let frames = vec![
            StickerFrame {
                pixels: RgbaImage::from_pixel(48, 48, named_rgba("red")),
                duration_us: 120_000,
            },
            StickerFrame {
                pixels: RgbaImage::from_pixel(48, 48, named_rgba("green")),
                duration_us: 240_000,
            },
            StickerFrame {
                pixels: RgbaImage::from_pixel(48, 48, named_rgba("blue")),
                duration_us: 360_000,
            },
        ];
        write_native_apng_file(Path::new(&output_path), &frames, "standard")
            .expect("native APNG writer should create a variable-duration animation");

        output_path
    }

    #[cfg(target_os = "windows")]
    fn sparse_sprite_frame(
        width: u32,
        height: u32,
        sprite_origin: Option<(u32, u32)>,
    ) -> RgbaImage {
        let mut pixels = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));
        if let Some((origin_x, origin_y)) = sprite_origin {
            for y in origin_y..origin_y + 3 {
                for x in origin_x..origin_x + 3 {
                    pixels.put_pixel(x, y, Rgba([255, 32, 64, 255]));
                }
            }
        }
        pixels
    }

    fn approx_eq(left: f64, right: f64) -> bool {
        (left - right).abs() <= 0.03
    }

    #[test]
    fn source_similarity_score_prefers_closer_to_source_candidates() {
        let preserved = source_similarity_score(24.0, 24, 1.0, "standard", 1.0, 1.0);
        let degraded = source_similarity_score(24.0, 12, 0.84, "compactPlus", 1.0, 1.0);

        assert!(preserved > degraded);
    }

    #[test]
    fn within_limit_selection_prefers_similarity_before_size() {
        let current = build_selected_output("current", 2, 120_000, 0.82);
        let contender = build_selected_output("contender", 3, 140_000, 0.93);

        assert!(is_better_within_limit_candidate(&current, &contender));
        assert!(!is_better_within_limit_candidate(&contender, &current));
    }

    #[test]
    fn oversize_selection_prefers_smaller_size_before_similarity() {
        let current = build_selected_output("current", 2, 580_000, 0.96);
        let contender = build_selected_output("contender", 3, 540_000, 0.82);

        assert!(is_better_oversize_candidate(&current, &contender));
        assert!(!is_better_oversize_candidate(&contender, &current));
    }

    #[test]
    fn search_stops_after_within_limit_when_remaining_similarity_is_lower() {
        let best = build_selected_output("best", 1, 500_000, 0.93);
        let mut candidate = build_test_candidate("lower", 12);
        candidate.source_similarity_score = 0.92;

        assert!(remaining_candidate_cannot_beat_within_limit(
            Some(&best),
            &candidate
        ));

        candidate.source_similarity_score = 0.93;
        assert!(!remaining_candidate_cannot_beat_within_limit(
            Some(&best),
            &candidate
        ));
        assert!(!remaining_candidate_cannot_beat_within_limit(
            None, &candidate
        ));
    }

    #[test]
    fn normalize_selected_frame_indexes_preserves_ui_order_and_duplicates() {
        assert_eq!(
            normalize_selected_frame_indexes(Some(&vec![7, 1, 3, 7])),
            Some(vec![6, 0, 2, 6])
        );
    }

    #[test]
    fn reordered_full_selection_is_not_collapsed_to_unedited_sequence() {
        let selection = resolve_frame_selection(Some(&vec![3, 1, 2]), Some(3))
            .expect("reordered full selection must resolve");

        assert_eq!(selection.selected_frames, Some(vec![2, 0, 1]));
        assert_eq!(selection.selected_frame_count, 3);
    }

    #[test]
    fn legacy_fit_modes_are_canonicalized_to_contain() {
        for raw in ["cover", "fill", "unexpected"] {
            let (mode, warning) = normalized_fit_mode(Some(raw), UiLocale::En);
            assert_eq!(mode, "contain");
            assert!(warning.is_some());
        }

        assert_eq!(
            normalized_fit_mode(Some("contain"), UiLocale::En),
            ("contain", None),
        );
        assert_eq!(normalized_fit_mode(None, UiLocale::En), ("contain", None));
    }

    #[test]
    fn optimizer_plan_request_accepts_missing_fit_mode() {
        let plan_request = serde_json::from_str::<OptimizerPlanRequest>("{}");
        let search_request =
            serde_json::from_str::<OptimizerSearchRequest>(r#"{"inputPath":"ignored.png"}"#);

        assert!(
            plan_request.is_ok(),
            "plan fitMode should remain optional on the wire"
        );
        assert!(
            search_request.is_ok(),
            "search fitMode should remain optional on the wire"
        );
    }

    #[test]
    fn legacy_fit_mode_plan_metadata_stays_canonical() {
        let response = prepare_optimizer_plan(
            &OptimizerPlanRequest {
                locale: Some("en".into()),
                source_duration_seconds: Some(1.0),
                input_width: Some(640),
                input_height: Some(320),
                avg_fps: Some(1.0),
                fit_mode: Some("cover".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: None,
                base_frame_count: Some(1),
                timeline_frames: None,
            },
            UiLocale::En,
        );

        assert!(response.ok);
        assert_eq!(response.fit_mode, "contain");
        assert!(!response.warnings.is_empty());
        assert!(response.candidates.iter().all(|candidate| {
            candidate.fit_mode == "contain" && candidate.id.starts_with("contain-")
        }));
    }

    #[test]
    fn legacy_fit_mode_search_error_stays_canonical_and_warns() {
        let (_test_dir, input_path, source_revision) =
            revisioned_placeholder("legacy-fit-search-error");
        let response = run_optimizer_search_internal(
            OptimizerSearchRequest {
                input_path,
                source_revision: Some(source_revision),
                output_directory: None,
                locale: Some("en".into()),
                source_duration_seconds: Some(1.0),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(7.0),
                fit_mode: Some("fill".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: Some(Vec::new()),
                base_frame_count: Some(7),
                timeline_frames: None,
            },
            UiLocale::En,
        );

        assert!(!response.ok);
        assert_eq!(response.fit_mode, "contain");
        assert!(!response.warnings.is_empty());
    }

    #[test]
    fn build_filter_graph_retimes_selected_frames_before_fps_resampling() {
        let graph = build_filter_graph(12, 1.0, Some(320), Some(320), None, Some(&[0, 6]));

        assert!(graph.contains("select='eq(n,0)+eq(n,6)',setpts=N/(12*TB),fps=12"));
        assert!(!graph.contains("select='eq(n,0)+eq(n,6)',fps=12"));
    }

    #[test]
    fn build_source_frame_select_filter_orders_unique_frame_indexes() {
        let frame_indexes = BTreeSet::from([4, 0, 2]);

        assert_eq!(
            build_source_frame_select_filter(&frame_indexes),
            "select='eq(n,0)+eq(n,2)+eq(n,4)',format=rgba"
        );
    }

    #[test]
    fn scaled_output_dimensions_preserve_smaller_sources() {
        assert_eq!(
            scaled_output_dimensions(Some(200), Some(120), 1.0),
            (200, 120)
        );
        assert_eq!(
            scaled_output_dimensions(Some(200), Some(120), 0.5),
            (100, 60)
        );
    }

    #[test]
    fn scaled_output_dimensions_cap_larger_sources_to_max_320() {
        assert_eq!(
            scaled_output_dimensions(Some(640), Some(320), 1.0),
            (320, 160)
        );
        assert_eq!(
            scaled_output_dimensions(Some(200), Some(500), 1.0),
            (128, 320)
        );
    }

    #[test]
    fn apng_preset_encoder_settings_vary_by_preset() {
        assert_eq!(apng_compression_level_for_preset("standard"), "4");
        assert_eq!(apng_compression_level_for_preset("compact"), "7");
        assert_eq!(apng_compression_level_for_preset("compactPlus"), "9");

        assert_eq!(apng_prediction_for_preset("standard"), "paeth");
        assert_eq!(apng_prediction_for_preset("compact"), "mixed");
        assert_eq!(apng_prediction_for_preset("compactPlus"), "mixed");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn sidecar_candidate_paths_include_packaged_windows_binary_name() {
        let exe_path = Path::new("C:/StickerFit/StickerFit.exe");
        let paths = sidecar_candidate_paths_for_exe("ffmpeg", Some(exe_path));
        let normalized = paths
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>();

        assert!(
            normalized.iter().any(|path| path.ends_with("/ffmpeg.exe")),
            "packaged Tauri sidecars are copied beside the executable as ffmpeg.exe"
        );
        assert!(
            normalized
                .iter()
                .any(|path| path.ends_with("/ffmpeg-x86_64-pc-windows-msvc.exe")),
            "development sidecar name should remain available"
        );
    }

    #[test]
    fn tool_health_wire_labels_do_not_expose_absolute_paths_or_raw_version_output() {
        let label = safe_tool_path_label(
            Path::new(r"C:\Users\private\ffmpeg-x86_64-pc-windows-msvc.exe"),
            "ffmpeg",
        );
        assert_eq!(label, "ffmpeg-x86_64-pc-windows-msvc.exe");
        assert!(!label.contains("C:\\Users"));

        let version =
            safe_tool_version_line("ffmpeg", br"ffmpeg version 7.1 C:\Users\private\build", &[])
                .expect("safe version label must be available");
        assert_eq!(version, "ffmpeg 7.1");
        assert!(!version.contains("private"));
    }

    fn png_chunk(chunk_type: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
        bytes.extend_from_slice(chunk_type);
        bytes.extend_from_slice(data);
        let mut crc = crc32fast::Hasher::new();
        crc.update(chunk_type);
        crc.update(data);
        bytes.extend_from_slice(&crc.finalize().to_be_bytes());
        bytes
    }

    fn png_ihdr_with_format(width: u32, height: u32, bit_depth: u8, color_type: u8) -> Vec<u8> {
        let mut data = [0u8; 13];
        data[0..4].copy_from_slice(&width.to_be_bytes());
        data[4..8].copy_from_slice(&height.to_be_bytes());
        data[8] = bit_depth;
        data[9] = color_type;
        png_chunk(b"IHDR", &data)
    }

    fn png_ihdr(width: u32, height: u32) -> Vec<u8> {
        png_ihdr_with_format(width, height, 8, 6)
    }

    fn png_suggested_palette(name: &str) -> Vec<u8> {
        let mut data = name.as_bytes().to_vec();
        data.push(0);
        data.push(8);
        data.extend_from_slice(&[0, 0, 0, 255, 0, 1]);
        data
    }

    fn png_actl(frame_count: u32) -> Vec<u8> {
        png_actl_with_plays(frame_count, 0)
    }

    fn png_actl_with_plays(frame_count: u32, play_count: u32) -> Vec<u8> {
        let mut data = [0u8; 8];
        data[0..4].copy_from_slice(&frame_count.to_be_bytes());
        data[4..8].copy_from_slice(&play_count.to_be_bytes());
        png_chunk(b"acTL", &data)
    }

    fn png_fctl(sequence: u32, width: u32, height: u32, x: u32, y: u32) -> Vec<u8> {
        png_fctl_with_ops(sequence, width, height, x, y, 0, 0)
    }

    fn png_fctl_with_ops(
        sequence: u32,
        width: u32,
        height: u32,
        x: u32,
        y: u32,
        dispose: u8,
        blend: u8,
    ) -> Vec<u8> {
        let mut data = [0u8; 26];
        data[0..4].copy_from_slice(&sequence.to_be_bytes());
        data[4..8].copy_from_slice(&width.to_be_bytes());
        data[8..12].copy_from_slice(&height.to_be_bytes());
        data[12..16].copy_from_slice(&x.to_be_bytes());
        data[16..20].copy_from_slice(&y.to_be_bytes());
        data[20..22].copy_from_slice(&1u16.to_be_bytes());
        data[22..24].copy_from_slice(&10u16.to_be_bytes());
        data[24] = dispose;
        data[25] = blend;
        png_chunk(b"fcTL", &data)
    }

    fn png_fdat(sequence: u32, frame_data: &[u8]) -> Vec<u8> {
        let mut data = Vec::with_capacity(4 + frame_data.len());
        data.extend_from_slice(&sequence.to_be_bytes());
        data.extend_from_slice(frame_data);
        png_chunk(b"fdAT", &data)
    }

    fn write_png_parser_fixture(name: &str, bytes: &[u8]) -> (TestDir, PathBuf) {
        let test_dir = TestDir::new(name);
        let path = test_dir.path.join("fixture.png");
        fs::write(&path, bytes).expect("PNG parser fixture must be written");
        (test_dir, path)
    }

    fn parse_png_error(name: &str, bytes: &[u8]) -> PipelineError {
        let (_test_dir, path) = write_png_parser_fixture(name, bytes);
        read_png_animation_metadata(&path, MediaLimits::default(), || Ok(()))
            .expect_err("malformed PNG must be rejected")
    }

    #[test]
    fn read_png_animation_metadata_rejects_non_first_ihdr() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_actl(1));
        bytes.extend_from_slice(&png_ihdr(32, 32));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));

        assert!(matches!(
            parse_png_error("png-ihdr-order", &bytes),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "IHDR must be the first chunk"
        ));
    }

    #[test]
    fn read_png_animation_metadata_allows_fctl_before_actl_when_both_precede_idat() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr(8, 8));
        bytes.extend_from_slice(&png_fctl(0, 8, 8, 0, 0));
        bytes.extend_from_slice(&png_actl(1));
        bytes.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
        let (_test_dir, path) = write_png_parser_fixture("png-fctl-before-actl", &bytes);

        let metadata = read_png_animation_metadata(&path, MediaLimits::default(), || Ok(()))
            .expect("fcTL may precede acTL before the first IDAT")
            .expect("acTL must produce animation metadata");
        assert_eq!(metadata.frame_count, Some(1));
    }

    #[test]
    fn read_png_animation_metadata_rejects_invalid_fixed_chunk_lengths() {
        for (name, chunk_type, payload, expected_reason) in [
            ("ihdr", b"IHDR", vec![0u8; 12], "IHDR length must be 13"),
            ("actl", b"acTL", vec![0u8; 7], "acTL length must be 8"),
            ("fctl", b"fcTL", vec![0u8; 25], "fcTL length must be 26"),
        ] {
            let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
            if name != "ihdr" {
                bytes.extend_from_slice(&png_ihdr(32, 32));
            }
            if name == "fctl" {
                bytes.extend_from_slice(&png_actl(1));
            }
            bytes.extend_from_slice(&png_chunk(chunk_type, &payload));
            bytes.extend_from_slice(&png_chunk(b"IEND", &[]));

            assert!(matches!(
                parse_png_error(&format!("png-{name}-length"), &bytes),
                PipelineError::MalformedInput { format: "png", reason }
                    if reason == expected_reason
            ));
        }
    }

    #[test]
    fn read_png_metadata_rejects_invalid_ihdr_and_critical_chunk_contracts() {
        let mut invalid_ihdr_data = [0u8; 13];
        invalid_ihdr_data[0..4].copy_from_slice(&8u32.to_be_bytes());
        invalid_ihdr_data[4..8].copy_from_slice(&8u32.to_be_bytes());
        invalid_ihdr_data[8] = 8;
        invalid_ihdr_data[9] = 6;
        invalid_ihdr_data[10] = 1;
        let mut invalid_ihdr = b"\x89PNG\r\n\x1a\n".to_vec();
        invalid_ihdr.extend_from_slice(&png_chunk(b"IHDR", &invalid_ihdr_data));
        match parse_png_error("png-invalid-ihdr-control", &invalid_ihdr) {
            PipelineError::MalformedInput { reason, .. } => {
                assert!(reason.contains("IHDR control fields"));
            }
            error => panic!("expected IHDR control error, got {error:?}"),
        }

        for (name, chunk_type, expected_reason) in [
            ("reserved", *b"abcd", "chunk type is invalid"),
            ("critical", *b"ABCD", "unknown critical"),
        ] {
            let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
            bytes.extend_from_slice(&png_ihdr(8, 8));
            bytes.extend_from_slice(&png_chunk(&chunk_type, &[]));
            match parse_png_error(&format!("png-{name}-chunk"), &bytes) {
                PipelineError::MalformedInput { reason, .. } => {
                    assert!(reason.contains(expected_reason));
                }
                error => panic!("expected chunk type error, got {error:?}"),
            }
        }
    }

    #[test]
    fn read_png_metadata_enforces_plte_presence_order_and_uniqueness() {
        let mut indexed_ihdr = [0u8; 13];
        indexed_ihdr[0..4].copy_from_slice(&8u32.to_be_bytes());
        indexed_ihdr[4..8].copy_from_slice(&8u32.to_be_bytes());
        indexed_ihdr[8] = 8;
        indexed_ihdr[9] = 3;

        let mut missing = b"\x89PNG\r\n\x1a\n".to_vec();
        missing.extend_from_slice(&png_chunk(b"IHDR", &indexed_ihdr));
        missing.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        assert!(matches!(
            parse_png_error("png-indexed-missing-plte", &missing),
            PipelineError::MalformedInput { reason, .. }
                if reason.contains("requires PLTE")
        ));

        let mut duplicate = b"\x89PNG\r\n\x1a\n".to_vec();
        duplicate.extend_from_slice(&png_chunk(b"IHDR", &indexed_ihdr));
        duplicate.extend_from_slice(&png_chunk(b"PLTE", &[0, 0, 0]));
        duplicate.extend_from_slice(&png_chunk(b"PLTE", &[1, 1, 1]));
        assert!(matches!(
            parse_png_error("png-duplicate-plte", &duplicate),
            PipelineError::MalformedInput { reason, .. }
                if reason.contains("duplicate PLTE")
        ));

        let mut one_bit_ihdr = indexed_ihdr;
        one_bit_ihdr[8] = 1;
        let mut exact_palette = b"\x89PNG\r\n\x1a\n".to_vec();
        exact_palette.extend_from_slice(&png_chunk(b"IHDR", &one_bit_ihdr));
        exact_palette.extend_from_slice(&png_chunk(b"PLTE", &[0, 0, 0, 255, 255, 255]));
        exact_palette.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        exact_palette.extend_from_slice(&png_chunk(b"IEND", &[]));
        let (_test_dir, exact_path) =
            write_png_parser_fixture("png-one-bit-exact-palette", &exact_palette);
        assert!(
            read_png_metadata(&exact_path, MediaLimits::default(), || Ok(()))
                .expect("two entries must fit one-bit indexed PNG")
                .animation
                .is_none()
        );

        let mut oversized_palette = b"\x89PNG\r\n\x1a\n".to_vec();
        oversized_palette.extend_from_slice(&png_chunk(b"IHDR", &one_bit_ihdr));
        oversized_palette.extend_from_slice(&png_chunk(b"PLTE", &[0; 9]));
        assert!(matches!(
            parse_png_error("png-one-bit-oversized-palette", &oversized_palette),
            PipelineError::MalformedInput { reason, .. }
                if reason.contains("indexed bit depth")
        ));

        let mut rgb_ihdr = indexed_ihdr;
        rgb_ihdr[9] = 2;
        let mut late_palette = b"\x89PNG\r\n\x1a\n".to_vec();
        late_palette.extend_from_slice(&png_chunk(b"IHDR", &rgb_ihdr));
        late_palette.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        late_palette.extend_from_slice(&png_chunk(b"PLTE", &[0, 0, 0]));
        assert!(matches!(
            parse_png_error("png-late-plte", &late_palette),
            PipelineError::MalformedInput { reason, .. }
                if reason.contains("PLTE must precede IDAT")
        ));
    }

    #[test]
    fn read_png_animation_metadata_rejects_invalid_animation_frame_counts() {
        let mut zero = b"\x89PNG\r\n\x1a\n".to_vec();
        zero.extend_from_slice(&png_ihdr(32, 32));
        zero.extend_from_slice(&png_actl(0));
        zero.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-frame-count-0", &zero),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "acTL frame count must be non-zero"
        ));

        let mut excessive = b"\x89PNG\r\n\x1a\n".to_vec();
        excessive.extend_from_slice(&png_ihdr(32, 32));
        excessive.extend_from_slice(&png_actl(301));
        excessive.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-frame-count-301", &excessive),
            PipelineError::LimitExceeded {
                resource: "frame-count",
                limit: 300,
                actual: 301,
            }
        ));

        let mut excessive_plays = b"\x89PNG\r\n\x1a\n".to_vec();
        excessive_plays.extend_from_slice(&png_ihdr(8, 8));
        excessive_plays.extend_from_slice(&png_actl_with_plays(1, 0x8000_0000));
        assert!(matches!(
            parse_png_error("png-play-count-high-bit", &excessive_plays),
            PipelineError::MalformedInput { reason, .. }
                if reason == "acTL play count exceeds PNG integer range"
        ));

        let mut max_plays = b"\x89PNG\r\n\x1a\n".to_vec();
        max_plays.extend_from_slice(&png_ihdr(8, 8));
        max_plays.extend_from_slice(&png_actl_with_plays(1, i32::MAX as u32));
        max_plays.extend_from_slice(&png_fctl(0, 8, 8, 0, 0));
        max_plays.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        max_plays.extend_from_slice(&png_chunk(b"IEND", &[]));
        let (_test_dir, max_plays_path) =
            write_png_parser_fixture("png-max-play-count", &max_plays);
        assert!(
            read_png_animation_metadata(&max_plays_path, MediaLimits::default(), || Ok(()),)
                .expect("maximum PNG play count must parse")
                .is_some()
        );
    }

    #[test]
    fn read_png_animation_metadata_rejects_frame_rectangle_outside_canvas() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr(10, 10));
        bytes.extend_from_slice(&png_actl(1));
        bytes.extend_from_slice(&png_fctl(0, 8, 8, 3, 3));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));

        assert!(matches!(
            parse_png_error("png-frame-rectangle", &bytes),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "fcTL frame rectangle is outside the canvas"
        ));
    }

    #[test]
    fn read_png_animation_metadata_rejects_apng_sequence_and_fdat_length() {
        let mut sequence_gap = b"\x89PNG\r\n\x1a\n".to_vec();
        sequence_gap.extend_from_slice(&png_ihdr(8, 8));
        sequence_gap.extend_from_slice(&png_actl(1));
        sequence_gap.extend_from_slice(&png_fctl(1, 8, 8, 0, 0));
        sequence_gap.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        sequence_gap.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-sequence-gap", &sequence_gap),
            PipelineError::MalformedInput { format: "png", .. }
        ));

        let mut short_fdat = b"\x89PNG\r\n\x1a\n".to_vec();
        short_fdat.extend_from_slice(&png_ihdr(8, 8));
        short_fdat.extend_from_slice(&png_actl(1));
        short_fdat.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        short_fdat.extend_from_slice(&png_fctl(0, 8, 8, 0, 0));
        short_fdat.extend_from_slice(&png_chunk(b"fdAT", &[0, 0, 0]));
        short_fdat.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-short-fdat", &short_fdat),
            PipelineError::MalformedInput { format: "png", .. }
        ));

        let mut sequence_only_fdat = b"\x89PNG\r\n\x1a\n".to_vec();
        sequence_only_fdat.extend_from_slice(&png_ihdr(8, 8));
        sequence_only_fdat.extend_from_slice(&png_actl(1));
        sequence_only_fdat.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        sequence_only_fdat.extend_from_slice(&png_fctl(0, 8, 8, 0, 0));
        sequence_only_fdat.extend_from_slice(&png_fdat(1, &[]));
        sequence_only_fdat.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-empty-fdat", &sequence_only_fdat),
            PipelineError::MalformedInput { format: "png", .. }
        ));
    }

    #[test]
    fn read_png_animation_metadata_allows_empty_fdat_before_frame_payload() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr(8, 8));
        bytes.extend_from_slice(&png_actl(2));
        bytes.extend_from_slice(&png_fctl(0, 8, 8, 0, 0));
        bytes.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        bytes.extend_from_slice(&png_fctl(1, 8, 8, 0, 0));
        bytes.extend_from_slice(&png_fdat(2, &[]));
        bytes.extend_from_slice(&png_fdat(3, &[0]));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
        let (_test_dir, path) = write_png_parser_fixture("png-empty-fdat-prefix", &bytes);

        let metadata = read_png_animation_metadata(&path, MediaLimits::default(), || Ok(()))
            .expect("empty fdAT prefix followed by payload must parse")
            .expect("fixture must contain animation metadata");
        assert_eq!(metadata.frame_count, Some(2));
    }

    #[test]
    fn read_png_animation_metadata_rejects_invalid_dispose_and_blend_ops() {
        for (name, dispose, blend) in [("dispose", 3, 0), ("blend", 0, 2)] {
            let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
            bytes.extend_from_slice(&png_ihdr(8, 8));
            bytes.extend_from_slice(&png_actl(1));
            bytes.extend_from_slice(&png_fctl_with_ops(0, 8, 8, 0, 0, dispose, blend));
            bytes.extend_from_slice(&png_chunk(b"IDAT", &[0]));
            bytes.extend_from_slice(&png_chunk(b"IEND", &[]));

            assert!(matches!(
                parse_png_error(&format!("png-invalid-{name}"), &bytes),
                PipelineError::MalformedInput { format: "png", .. }
            ));
        }
    }

    #[test]
    fn read_png_animation_metadata_rejects_missing_or_misordered_frame_data() {
        let mut missing_idat = b"\x89PNG\r\n\x1a\n".to_vec();
        missing_idat.extend_from_slice(&png_ihdr(8, 8));
        missing_idat.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-missing-idat", &missing_idat),
            PipelineError::MalformedInput { format: "png", .. }
        ));

        let mut empty_idat = b"\x89PNG\r\n\x1a\n".to_vec();
        empty_idat.extend_from_slice(&png_ihdr(8, 8));
        empty_idat.extend_from_slice(&png_chunk(b"IDAT", &[]));
        empty_idat.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-empty-idat-stream", &empty_idat),
            PipelineError::MalformedInput { format: "png", .. }
        ));

        let mut empty_then_data = b"\x89PNG\r\n\x1a\n".to_vec();
        empty_then_data.extend_from_slice(&png_ihdr(8, 8));
        empty_then_data.extend_from_slice(&png_chunk(b"IDAT", &[]));
        empty_then_data.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        empty_then_data.extend_from_slice(&png_chunk(b"IEND", &[]));
        let (_test_dir, path) = write_png_parser_fixture("png-empty-idat-prefix", &empty_then_data);
        assert!(
            read_png_animation_metadata(&path, MediaLimits::default(), || Ok(()))
                .expect("empty IDAT prefix followed by data must parse")
                .is_none()
        );

        let mut pending_frame = b"\x89PNG\r\n\x1a\n".to_vec();
        pending_frame.extend_from_slice(&png_ihdr(8, 8));
        pending_frame.extend_from_slice(&png_actl(1));
        pending_frame.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        pending_frame.extend_from_slice(&png_fctl(0, 8, 8, 0, 0));
        pending_frame.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-fctl-without-data", &pending_frame),
            PipelineError::MalformedInput { format: "png", .. }
        ));

        let mut noncontiguous_idat = b"\x89PNG\r\n\x1a\n".to_vec();
        noncontiguous_idat.extend_from_slice(&png_ihdr(8, 8));
        noncontiguous_idat.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        noncontiguous_idat.extend_from_slice(&png_chunk(b"tEXt", &[]));
        noncontiguous_idat.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        noncontiguous_idat.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-noncontiguous-idat", &noncontiguous_idat),
            PipelineError::MalformedInput { format: "png", .. }
        ));

        let mut fdat_without_frame = b"\x89PNG\r\n\x1a\n".to_vec();
        fdat_without_frame.extend_from_slice(&png_ihdr(8, 8));
        fdat_without_frame.extend_from_slice(&png_actl(1));
        fdat_without_frame.extend_from_slice(&png_fctl(0, 8, 8, 0, 0));
        fdat_without_frame.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        fdat_without_frame.extend_from_slice(&png_fdat(1, &[0]));
        fdat_without_frame.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-fdat-without-frame", &fdat_without_frame),
            PipelineError::MalformedInput { format: "png", .. }
        ));
    }

    #[test]
    fn read_png_animation_metadata_requires_full_canvas_for_first_animated_idat_frame() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr(10, 10));
        bytes.extend_from_slice(&png_actl(1));
        bytes.extend_from_slice(&png_fctl(0, 8, 8, 1, 1));
        bytes.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));

        assert!(matches!(
            parse_png_error("png-first-frame-subrect", &bytes),
            PipelineError::MalformedInput { format: "png", .. }
        ));
    }

    #[test]
    fn read_png_animation_metadata_caps_observed_frame_controls() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr(1, 1));
        bytes.extend_from_slice(&png_actl(1));
        bytes.extend_from_slice(&png_fctl(0, 1, 1, 0, 0));
        bytes.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        for frame in 1..=300u32 {
            let sequence = frame * 2 - 1;
            bytes.extend_from_slice(&png_fctl(sequence, 1, 1, 0, 0));
            bytes.extend_from_slice(&png_fdat(sequence + 1, &[0]));
        }
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));

        assert!(matches!(
            parse_png_error("png-observed-frame-cap", &bytes),
            PipelineError::LimitExceeded {
                resource: "frame-count",
                limit: 300,
                actual: 301,
            }
        ));
    }

    #[test]
    fn read_png_animation_metadata_rejects_crc_mismatch() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = png_ihdr(32, 32);
        let last = ihdr.len() - 1;
        ihdr[last] ^= 0xff;
        bytes.extend_from_slice(&ihdr);

        assert!(matches!(
            parse_png_error("png-crc", &bytes),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "CRC mismatch for IHDR chunk"
        ));
    }

    #[test]
    fn inspect_input_media_internal_reports_malformed_png_crc_without_static_fallback() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = png_ihdr(32, 32);
        let last_crc_byte = ihdr.last_mut().expect("IHDR fixture must include a CRC");
        *last_crc_byte ^= 0xff;
        bytes.extend_from_slice(&ihdr);
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
        let (_test_dir, path) = write_png_parser_fixture("png-dispatch-crc", &bytes);

        let inspection =
            inspect_input_media_internal(path.to_string_lossy().as_ref(), UiLocale::En);

        assert!(!inspection.ok);
        assert_eq!(inspection.error_code.as_deref(), Some("malformed-media"));
        assert!(!inspection.is_static_image);
    }

    #[test]
    fn read_png_animation_metadata_rejects_oversized_chunk_before_allocation() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr(32, 32));
        bytes.extend_from_slice(&(MediaLimits::default().max_png_chunk_bytes + 1).to_be_bytes());
        bytes.extend_from_slice(b"iTXt");

        assert!(matches!(
            parse_png_error("png-huge-chunk", &bytes),
            PipelineError::LimitExceeded {
                resource: "png-chunk-bytes",
                ..
            }
        ));
    }

    #[test]
    fn read_png_animation_metadata_rejects_chunk_lengths_above_the_png_integer_range() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr(32, 32));
        bytes.extend_from_slice(&0x8000_0000u32.to_be_bytes());
        bytes.extend_from_slice(b"IDAT");

        assert!(matches!(
            parse_png_error("png-31-bit-chunk-length", &bytes),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "PNG chunk length exceeds the 31-bit maximum"
        ));
    }

    #[test]
    fn read_png_animation_metadata_rejects_duplicate_and_late_gama_with_exact_reasons() {
        let gamma = 45_455u32.to_be_bytes();

        let mut duplicate = b"\x89PNG\r\n\x1a\n".to_vec();
        duplicate.extend_from_slice(&png_ihdr_with_format(32, 32, 8, 2));
        duplicate.extend_from_slice(&png_chunk(b"gAMA", &gamma));
        duplicate.extend_from_slice(&png_chunk(b"gAMA", &gamma));
        duplicate.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        duplicate.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-duplicate-gama", &duplicate),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "duplicate gAMA chunk"
        ));

        let mut after_idat = b"\x89PNG\r\n\x1a\n".to_vec();
        after_idat.extend_from_slice(&png_ihdr_with_format(32, 32, 8, 2));
        after_idat.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        after_idat.extend_from_slice(&png_chunk(b"gAMA", &gamma));
        after_idat.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-gama-after-idat", &after_idat),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "gAMA must precede PLTE and IDAT"
        ));
    }

    #[test]
    fn read_png_animation_metadata_enforces_plte_and_idat_ancillary_boundaries() {
        let gamma = 45_455u32.to_be_bytes();

        let mut gamma_after_plte = b"\x89PNG\r\n\x1a\n".to_vec();
        gamma_after_plte.extend_from_slice(&png_ihdr_with_format(32, 32, 8, 2));
        gamma_after_plte.extend_from_slice(&png_chunk(b"PLTE", &[0, 0, 0]));
        gamma_after_plte.extend_from_slice(&png_chunk(b"gAMA", &gamma));
        gamma_after_plte.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        gamma_after_plte.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-gama-after-plte", &gamma_after_plte),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "gAMA must precede PLTE and IDAT"
        ));

        let mut background_before_plte = b"\x89PNG\r\n\x1a\n".to_vec();
        background_before_plte.extend_from_slice(&png_ihdr_with_format(32, 32, 8, 3));
        background_before_plte.extend_from_slice(&png_chunk(b"bKGD", &[0]));
        background_before_plte.extend_from_slice(&png_chunk(b"PLTE", &[0, 0, 0]));
        background_before_plte.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        background_before_plte.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-bkgd-before-plte", &background_before_plte),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "PLTE must precede bKGD, hIST, and tRNS"
        ));

        let mut palette_after_idat = b"\x89PNG\r\n\x1a\n".to_vec();
        palette_after_idat.extend_from_slice(&png_ihdr_with_format(32, 32, 8, 2));
        palette_after_idat.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        palette_after_idat.extend_from_slice(&png_chunk(b"sPLT", &png_suggested_palette("late")));
        palette_after_idat.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-splt-after-idat", &palette_after_idat),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "sPLT must precede IDAT"
        ));
    }

    #[test]
    fn read_png_animation_metadata_requires_plte_for_hist_and_keeps_time_order_free_but_singleton()
    {
        let mut histogram_without_plte = b"\x89PNG\r\n\x1a\n".to_vec();
        histogram_without_plte.extend_from_slice(&png_ihdr_with_format(32, 32, 8, 2));
        histogram_without_plte.extend_from_slice(&png_chunk(b"hIST", &[0, 1]));
        histogram_without_plte.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        histogram_without_plte.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-hist-without-plte", &histogram_without_plte),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "hIST requires a preceding PLTE chunk"
        ));

        let timestamp = [0x07, 0xea, 7, 11, 0, 0, 0];
        let mut duplicate_time_after_idat = b"\x89PNG\r\n\x1a\n".to_vec();
        duplicate_time_after_idat.extend_from_slice(&png_ihdr_with_format(32, 32, 8, 2));
        duplicate_time_after_idat.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        duplicate_time_after_idat.extend_from_slice(&png_chunk(b"tIME", &timestamp));
        duplicate_time_after_idat.extend_from_slice(&png_chunk(b"tIME", &timestamp));
        duplicate_time_after_idat.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert!(matches!(
            parse_png_error("png-duplicate-time-after-idat", &duplicate_time_after_idat),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "duplicate tIME chunk"
        ));
    }

    #[test]
    fn read_png_animation_metadata_preserves_repeatable_text_and_splt_chunks() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr_with_format(32, 32, 8, 2));
        bytes.extend_from_slice(&png_chunk(b"sPLT", &png_suggested_palette("first")));
        bytes.extend_from_slice(&png_chunk(b"sPLT", &png_suggested_palette("second")));
        bytes.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        bytes.extend_from_slice(&png_chunk(b"tEXt", b"Comment\0one"));
        bytes.extend_from_slice(&png_chunk(b"tEXt", b"Comment\0two"));
        bytes.extend_from_slice(&png_chunk(b"iTXt", b"Title\0\0\0\0\0one"));
        bytes.extend_from_slice(&png_chunk(b"iTXt", b"Title\0\0\0\0\0two"));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
        let (_test_dir, path) = write_png_parser_fixture("png-repeatable-ancillary", &bytes);

        read_png_animation_metadata(&path, MediaLimits::default(), || Ok(()))
            .expect("repeatable text and sPLT chunks must remain legal");
    }

    #[test]
    fn read_png_animation_metadata_rejects_truncated_payload_or_crc() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr(32, 32));
        bytes.extend_from_slice(&4u32.to_be_bytes());
        bytes.extend_from_slice(b"tEXt");
        bytes.extend_from_slice(&[1, 2]);

        assert!(matches!(
            parse_png_error("png-truncated", &bytes),
            PipelineError::MalformedInput { format: "png", .. }
        ));
    }

    #[test]
    fn read_png_animation_metadata_requires_zero_length_iend() {
        let mut missing = b"\x89PNG\r\n\x1a\n".to_vec();
        missing.extend_from_slice(&png_ihdr(32, 32));
        assert!(matches!(
            parse_png_error("png-missing-iend", &missing),
            PipelineError::MalformedInput { format: "png", .. }
        ));

        let mut nonzero = b"\x89PNG\r\n\x1a\n".to_vec();
        nonzero.extend_from_slice(&png_ihdr(32, 32));
        nonzero.extend_from_slice(&png_chunk(b"IEND", &[0]));
        assert!(matches!(
            parse_png_error("png-nonzero-iend", &nonzero),
            PipelineError::MalformedInput { format: "png", reason }
                if reason == "IEND length must be zero"
        ));
    }

    #[test]
    fn read_png_animation_metadata_rejects_fctl_count_mismatch() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr(32, 32));
        bytes.extend_from_slice(&png_actl(2));
        bytes.extend_from_slice(&png_fctl(0, 32, 32, 0, 0));
        bytes.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));

        match parse_png_error("png-frame-count-mismatch", &bytes) {
            PipelineError::MalformedInput {
                format: "png",
                reason,
            } => assert!(reason.contains("fcTL count does not match")),
            error => panic!("expected frame-count mismatch, got {error:?}"),
        }
    }

    #[test]
    fn read_png_animation_metadata_stops_on_stream_checkpoint_cancellation() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr(32, 32));
        bytes.extend_from_slice(&png_chunk(b"iTXt", &vec![0u8; 128 * 1024]));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
        let (_test_dir, path) = write_png_parser_fixture("png-checkpoint", &bytes);
        let mut checkpoints = 0;

        let error = read_png_animation_metadata(&path, MediaLimits::default(), || {
            checkpoints += 1;
            if checkpoints >= 4 {
                Err(PipelineError::Cancelled)
            } else {
                Ok(())
            }
        })
        .expect_err("stream checkpoint cancellation must stop parsing");

        assert_eq!(error, PipelineError::Cancelled);
    }

    #[test]
    fn read_png_animation_metadata_warns_about_trailing_bytes() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&png_ihdr(32, 32));
        bytes.extend_from_slice(&png_actl(1));
        bytes.extend_from_slice(&png_fctl(0, 32, 32, 0, 0));
        bytes.extend_from_slice(&png_chunk(b"IDAT", &[0]));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
        bytes.extend_from_slice(b"trailing");
        let (_test_dir, path) = write_png_parser_fixture("png-trailing", &bytes);

        let metadata = read_png_animation_metadata(&path, MediaLimits::default(), || Ok(()))
            .expect("valid APNG should parse")
            .expect("acTL should produce animation metadata");
        assert_eq!(metadata.warnings, vec!["png-trailing-bytes"]);
    }

    #[test]
    fn static_png_inspection_preserves_trailing_bytes_warning() {
        let test_dir = TestDir::new("static-png-trailing-warning");
        let input_path = test_dir.path.join("static.png");
        DynamicImage::ImageRgba8(RgbaImage::new(2, 2))
            .save_with_format(&input_path, ImageFormat::Png)
            .expect("static PNG fixture must be encoded");
        let mut bytes = fs::read(&input_path).expect("static PNG fixture must be read");
        bytes.extend_from_slice(b"trailing");
        fs::write(&input_path, bytes).expect("static PNG trailing bytes must be written");

        let metadata = read_png_metadata(&input_path, MediaLimits::default(), || Ok(()))
            .expect("static PNG metadata must parse");
        assert!(metadata.animation.is_none());
        assert_eq!(metadata.warnings, vec!["png-trailing-bytes"]);

        let inspection =
            inspect_input_media_internal(input_path.to_string_lossy().as_ref(), UiLocale::En);
        assert!(inspection.ok);
        assert_eq!(inspection.warnings, vec!["png-trailing-bytes"]);
    }

    #[test]
    fn read_png_animation_metadata_does_not_require_decodable_pixels() {
        let test_dir = TestDir::new("apng-metadata-only");
        let input_path = test_dir.path.join("metadata-only.png");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes.extend_from_slice(&png_chunk(
            b"IHDR",
            &[0, 0, 0, 48, 0, 0, 0, 32, 8, 6, 0, 0, 0],
        ));
        bytes.extend_from_slice(&png_chunk(b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]));

        let mut first_frame = png_fctl(0, 48, 32, 0, 0);
        let delay_start = 8 + 20;
        first_frame[delay_start..delay_start + 2].copy_from_slice(&12u16.to_be_bytes());
        first_frame[delay_start + 2..delay_start + 4].copy_from_slice(&100u16.to_be_bytes());
        let first_payload = first_frame[8..34].to_vec();
        bytes.extend_from_slice(&png_chunk(b"fcTL", &first_payload));
        bytes.extend_from_slice(&png_chunk(b"IDAT", &[0]));

        let mut second_frame_data = [0u8; 26];
        second_frame_data[0..4].copy_from_slice(&1u32.to_be_bytes());
        second_frame_data[4..8].copy_from_slice(&48u32.to_be_bytes());
        second_frame_data[8..12].copy_from_slice(&32u32.to_be_bytes());
        second_frame_data[20..22].copy_from_slice(&24u16.to_be_bytes());
        second_frame_data[22..24].copy_from_slice(&100u16.to_be_bytes());
        bytes.extend_from_slice(&png_chunk(b"fcTL", &second_frame_data));
        bytes.extend_from_slice(&png_fdat(2, &[0]));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
        fs::write(&input_path, bytes).expect("metadata-only png should be written");

        let metadata = read_png_animation_metadata(&input_path, MediaLimits::default(), || Ok(()))
            .expect("metadata parser should read valid chunks")
            .expect("acTL should mark this as APNG metadata");

        assert_eq!(metadata.width, 48);
        assert_eq!(metadata.height, 32);
        assert_eq!(metadata.frame_count, Some(2));
        assert_eq!(metadata.frame_durations.len(), 2);
        assert!(approx_eq(metadata.frame_durations[0], 0.12));
        assert!(approx_eq(metadata.frame_durations[1], 0.24));
    }

    #[test]
    fn read_png_animation_metadata_accepts_native_writer_output() {
        let test_dir = TestDir::new("apng-native-parser-positive");
        let input_path = test_dir.path.join("native.png");
        let frames = vec![
            StickerFrame {
                pixels: RgbaImage::new(3, 2),
                duration_us: 120_000,
            },
            StickerFrame {
                pixels: RgbaImage::from_pixel(3, 2, Rgba([1, 2, 3, 255])),
                duration_us: 240_000,
            },
        ];
        write_native_apng_file(&input_path, &frames, "standard")
            .expect("native APNG fixture must be written");

        let metadata = read_png_animation_metadata(&input_path, MediaLimits::default(), || Ok(()))
            .expect("native APNG metadata must parse")
            .expect("native APNG must contain animation metadata");

        assert_eq!(metadata.frame_count, Some(2));
        assert_eq!(metadata.frame_durations.len(), 2);
    }

    #[test]
    fn write_native_apng_maps_sub_tick_duration_to_typed_invalid_request() {
        let test_dir = TestDir::new("apng-invalid-duration-error");
        let output_path = test_dir.path.join("invalid.png");
        let frames = vec![StickerFrame {
            pixels: RgbaImage::new(1, 1),
            duration_us: 99,
        }];

        let error = write_native_apng_file(&output_path, &frames, "standard")
            .expect_err("sub-tick authored duration must be rejected");

        assert_eq!(error.code(), "invalid-request");
        assert_eq!(error.reason_code(), Some("invalid-frame-duration"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn write_native_apng_preserves_sparse_changed_regions() {
        let test_dir = TestDir::new("apng-sparse-delta");
        let output_path = test_dir.path.join("sparse-delta.png");
        let expected_frames = vec![
            StickerFrame {
                pixels: sparse_sprite_frame(16, 16, Some((2, 2))),
                duration_us: 100_000,
            },
            StickerFrame {
                pixels: sparse_sprite_frame(16, 16, Some((9, 2))),
                duration_us: 120_000,
            },
            StickerFrame {
                pixels: sparse_sprite_frame(16, 16, None),
                duration_us: 140_000,
            },
            StickerFrame {
                pixels: sparse_sprite_frame(16, 16, Some((4, 10))),
                duration_us: 160_000,
            },
        ];

        write_native_apng_file(&output_path, &expected_frames, "compact")
            .expect("sparse APNG should be written");

        let decoded_frames = decode_apng_animation_frames(
            output_path.to_string_lossy().as_ref(),
            MediaLimits::default(),
            || Ok(()),
        )
        .expect("sparse APNG should decode");

        assert_eq!(decoded_frames.len(), expected_frames.len());
        for (decoded, expected) in decoded_frames.iter().zip(expected_frames.iter()) {
            assert_eq!(decoded.pixels.as_raw(), expected.pixels.as_raw());
            assert_eq!(decoded.duration_us, expected.duration_us);
        }
    }

    #[test]
    fn build_filter_graph_uses_cropped_dimensions_for_max_320_scaling() {
        let graph = build_filter_graph(
            12,
            1.0,
            Some(640),
            Some(320),
            Some(ResolvedCropRegion {
                x: 0,
                y: 0,
                width: 200,
                height: 100,
            }),
            None,
        );

        assert!(graph.contains("crop=200:100:0:0"));
        assert!(
            graph.contains("crop=200:100:0:0,setpts=PTS-STARTPTS,fps=12,scale=200:100,format=rgba")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn encode_candidate_internal_keeps_only_selected_frames() {
        let test_dir = TestDir::new("frame-selection");
        let input_path = create_seven_frame_animation(&test_dir);
        let candidate = build_test_candidate("selection-check", 12);
        let selected_frames = vec![0, 6];

        let result = encode_candidate_internal(
            &input_path,
            Some(test_dir.path.to_string_lossy().as_ref()),
            UiLocale::En,
            None,
            Some(48),
            Some(48),
            &candidate,
            Some(selected_frames.as_slice()),
            None,
            None,
        )
        .expect("encoding should succeed");
        assert_eq!(result.tool_source, "native");
        assert_eq!(result.tool_command, None);

        let output_path = result
            .pending_output
            .commit()
            .expect("selected-frame output must commit")
            .to_string_lossy()
            .into_owned();
        let inspection = inspect_input_media_internal(&output_path, UiLocale::En);

        assert!(inspection.ok, "inspection should succeed");
        assert_eq!(inspection.estimated_frames, Some(2));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn inspect_input_media_internal_reports_counted_animation_frames() {
        let test_dir = TestDir::new("frame-count-inspection");
        let input_path = create_three_frame_animation(&test_dir);

        let inspection = inspect_input_media_internal(&input_path, UiLocale::En);

        assert!(inspection.ok, "inspection should succeed");
        assert_eq!(inspection.estimated_frames, Some(3));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn inspect_input_media_internal_reports_true_frame_durations() {
        let test_dir = TestDir::new("frame-duration-inspection");
        let input_path = create_variable_duration_animation(&test_dir);

        let inspection = inspect_input_media_internal(&input_path, UiLocale::En);
        let frame_durations = inspection
            .frame_durations_seconds
            .expect("frame durations should be present for variable animation");

        assert!(inspection.ok, "inspection should succeed");
        assert_eq!(inspection.estimated_frames, Some(3));
        assert_eq!(frame_durations.len(), 3);
        assert!(approx_eq(frame_durations[0], 0.12));
        assert!(approx_eq(frame_durations[1], 0.24));
        assert!(approx_eq(frame_durations[2], 0.36));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn extract_frame_preview_internal_returns_static_png_data_url_for_animation_frame() {
        let test_dir = TestDir::new("frame-preview");
        let input_path = create_three_frame_animation(&test_dir);

        let first = extract_frame_preview_internal(&input_path, 1, UiLocale::En);
        let second = extract_frame_preview_internal(&input_path, 2, UiLocale::En);

        assert!(first.ok, "first frame preview should succeed");
        assert!(second.ok, "second frame preview should succeed");
        assert_eq!(first.width, Some(48));
        assert_eq!(first.height, Some(48));
        assert!(
            first
                .data_url
                .as_deref()
                .is_some_and(|value| value.starts_with("data:image/png;base64,")),
            "frame preview should be a PNG data URL"
        );
        assert_ne!(first.data_url, second.data_url);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn extract_frame_previews_internal_batches_static_png_data_urls() {
        let test_dir = TestDir::new("frame-preview-batch");
        let input_path = create_three_frame_animation(&test_dir);

        let response = extract_frame_previews_internal(&input_path, &[1, 2, 3], UiLocale::En);

        assert!(response.ok, "batch frame preview should succeed");
        assert_eq!(response.previews.len(), 3);
        assert_eq!(response.previews[0].source_frame_id, 1);
        assert_eq!(response.previews[1].source_frame_id, 2);
        assert_eq!(response.previews[2].source_frame_id, 3);
        assert!(
            response
                .previews
                .iter()
                .all(|preview| preview.data_url.starts_with("data:image/png;base64,")),
            "all frame previews should be PNG data URLs"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn configured_sample_gif_extracts_static_frame_preview() {
        let Some(sample_dir) = std::env::var_os("STICKERFIT_SAMPLE_GIF_DIR").map(PathBuf::from)
        else {
            return;
        };

        let mut gif_paths = fs::read_dir(&sample_dir)
            .expect("sample GIF directory should be readable")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
            })
            .collect::<Vec<_>>();
        gif_paths.sort();
        let sample_path = gif_paths
            .first()
            .expect("sample GIF directory should contain at least one GIF");
        let input_path = sample_path.to_string_lossy();

        let inspection = inspect_input_media_internal(&input_path, UiLocale::En);
        assert!(inspection.ok, "sample GIF inspection should succeed");
        assert_eq!(inspection.format_name.as_deref(), Some("gif"));
        assert!(
            inspection.estimated_frames.unwrap_or_default() > 0,
            "sample GIF should expose at least one frame"
        );

        let preview = extract_frame_preview_internal(&input_path, 1, UiLocale::En);
        assert!(preview.ok, "sample GIF frame preview should succeed");
        let expected_dimensions = preview_output_dimensions(
            inspection.width.expect("sample width"),
            inspection.height.expect("sample height"),
        )
        .expect("sample preview dimensions must be bounded");
        assert_eq!(preview.width, Some(expected_dimensions.0));
        assert_eq!(preview.height, Some(expected_dimensions.1));
        assert!(expected_dimensions.0 <= 128 && expected_dimensions.1 <= 128);
        assert!(
            preview
                .data_url
                .as_deref()
                .is_some_and(|value| value.starts_with("data:image/png;base64,")),
            "sample GIF frame preview should be a PNG data URL"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn inspect_input_media_internal_prefers_native_mp4_metadata_when_sample_exists() {
        let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tmp")
            .join("web-preview-test.mp4");
        if !sample_path.is_file() {
            return;
        }

        let inspection =
            inspect_input_media_internal(sample_path.to_string_lossy().as_ref(), UiLocale::En);

        assert!(
            inspection.ok,
            "inspection should succeed for the repository mp4 sample"
        );
        assert_eq!(inspection.tool_source.as_deref(), Some("native"));
        assert_eq!(inspection.format_name.as_deref(), Some("mp4"));
        assert!(inspection.width.unwrap_or_default() > 0);
        assert!(inspection.height.unwrap_or_default() > 0);
        assert!(
            inspection.duration_seconds.unwrap_or_default() > 0.0,
            "native mp4 inspection should report duration"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn inspect_input_media_internal_keeps_webm_on_single_ffmpeg_path_when_sample_exists() {
        let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tmp")
            .join("locale-check")
            .join("check.webm");
        if !sample_path.is_file() {
            return;
        }

        let inspection =
            inspect_input_media_internal(sample_path.to_string_lossy().as_ref(), UiLocale::En);

        assert!(
            inspection.ok,
            "inspection should succeed for the repository webm sample"
        );
        assert_eq!(inspection.tool_source.as_deref(), Some("sidecar"));
        assert_eq!(inspection.format_name.as_deref(), Some("matroska,webm"));
        assert!(inspection.width.unwrap_or_default() > 0);
        assert!(inspection.height.unwrap_or_default() > 0);
        assert!(
            inspection.duration_seconds.unwrap_or_default() > 0.0,
            "webm inspection should report duration"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "set STICKERFIT_SAMPLE_GIF_DIR to run local GIF sample optimization"]
    fn sample_gif_folder_optimizes_to_discord_limit() {
        let sample_dir = std::env::var_os("STICKERFIT_SAMPLE_GIF_DIR")
            .map(PathBuf::from)
            .expect("STICKERFIT_SAMPLE_GIF_DIR must point at a folder of GIF samples");
        let output_root = std::env::var_os("STICKERFIT_SAMPLE_GIF_OUTPUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("target")
                    .join("stickerfit-sample-gif-output")
            });
        let preset_strategy =
            std::env::var("STICKERFIT_SAMPLE_GIF_PRESET").unwrap_or_else(|_| "auto".into());
        let search_depth = std::env::var("STICKERFIT_SAMPLE_GIF_SEARCH_DEPTH")
            .unwrap_or_else(|_| "standard".into());
        let run_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let output_dir = output_root.join(format!("{}-{}", std::process::id(), run_id));
        fs::create_dir_all(&output_dir).expect("sample output directory should be created");

        let mut gif_paths = fs::read_dir(&sample_dir)
            .expect("sample GIF directory should be readable")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
            })
            .collect::<Vec<_>>();
        gif_paths.sort();
        assert!(
            !gif_paths.is_empty(),
            "sample GIF directory should contain at least one .gif file"
        );

        println!("SAMPLE_GIF_OUTPUT_DIR\t{}", output_dir.display());
        println!("SAMPLE_GIF_SETTINGS\tpreset={preset_strategy}\tsearch_depth={search_depth}");
        let mut failures = Vec::new();

        for path in gif_paths {
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<non-utf8>");
            let input_path = path.to_string_lossy().into_owned();
            let input_size = fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);

            let inspect_started = Instant::now();
            let inspection = inspect_input_media_internal(&input_path, UiLocale::En);
            let inspect_elapsed_ms = inspect_started.elapsed().as_millis();

            println!(
                "INSPECT\t{}\tok={}\tinput_bytes={}\t{}x{}\tduration={:.3}\tframes={}\tfps={:.3}\telapsed_ms={}",
                file_name,
                inspection.ok,
                input_size,
                inspection.width.unwrap_or_default(),
                inspection.height.unwrap_or_default(),
                inspection.duration_seconds.unwrap_or_default(),
                inspection.estimated_frames.unwrap_or_default(),
                inspection.avg_fps.unwrap_or_default(),
                inspect_elapsed_ms
            );

            if !inspection.ok {
                failures.push(format!(
                    "{file_name}: inspect failed: {}",
                    inspection.error_message.unwrap_or_default()
                ));
                continue;
            }

            let optimize_started = Instant::now();
            let response = run_optimizer_search_internal(
                OptimizerSearchRequest {
                    input_path: input_path.clone(),
                    source_revision: inspection.source_revision.clone(),
                    output_directory: Some(output_dir.to_string_lossy().into_owned()),
                    locale: Some("en".into()),
                    source_duration_seconds: inspection.duration_seconds,
                    input_width: inspection.width,
                    input_height: inspection.height,
                    avg_fps: inspection.avg_fps,
                    fit_mode: Some("contain".into()),
                    preset_strategy: Some(preset_strategy.clone()),
                    optimizer_goal: None,
                    quality_frame_drop_interval: None,
                    search_depth: Some(search_depth.clone()),
                    crop_region: None,
                    selected_frames: None,
                    base_frame_count: inspection
                        .estimated_frames
                        .and_then(|frame_count| u32::try_from(frame_count).ok()),
                    timeline_frames: None,
                },
                UiLocale::En,
            );
            let optimize_elapsed_ms = optimize_started.elapsed().as_millis();

            println!(
                "OPTIMIZE\t{}\tok={}\tbest_within_limit={}\tbest_bytes={}\tattempts={}\tstop={}\tselected={:.3}\telapsed_ms={}\toutput={}",
                file_name,
                response.ok,
                response.best_within_limit,
                response.best_size_bytes.unwrap_or_default(),
                response.real_attempt_count,
                response.stop_reason.as_deref().unwrap_or("-"),
                response.selected_duration_seconds.unwrap_or_default(),
                optimize_elapsed_ms,
                response.best_output_path.as_deref().unwrap_or("-")
            );

            if !response.ok || !response.best_within_limit {
                failures.push(format!(
                    "{file_name}: optimizer failed: {}",
                    response
                        .error_message
                        .clone()
                        .unwrap_or_else(|| response.summary.clone())
                ));
                continue;
            }

            let Some(best_output_path) = response.best_output_path.as_deref() else {
                failures.push(format!(
                    "{file_name}: optimizer did not keep an output path"
                ));
                continue;
            };
            let Some(best_size_bytes) = response.best_size_bytes else {
                failures.push(format!("{file_name}: optimizer did not report output size"));
                continue;
            };

            if best_size_bytes > DISCORD_MAX_STICKER_BYTES {
                failures.push(format!(
                    "{file_name}: output exceeded Discord limit: {best_size_bytes} bytes"
                ));
                continue;
            }

            let output_inspection = inspect_input_media_internal(best_output_path, UiLocale::En);
            if !output_inspection.ok {
                failures.push(format!(
                    "{file_name}: output inspection failed: {}",
                    output_inspection.error_message.unwrap_or_default()
                ));
                continue;
            }

            if output_inspection.width.unwrap_or_default() > 320
                || output_inspection.height.unwrap_or_default() > 320
            {
                failures.push(format!(
                    "{file_name}: output dimensions exceeded 320x320: {}x{}",
                    output_inspection.width.unwrap_or_default(),
                    output_inspection.height.unwrap_or_default()
                ));
            }
        }

        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn inspect_input_media_internal_marks_png_as_convertible_static_image() {
        let test_dir = TestDir::new("static-png-inspection");
        let input_path = create_static_image(&test_dir, "input.png", "red");

        let inspection = inspect_input_media_internal(&input_path, UiLocale::En);

        assert!(inspection.ok, "inspection should succeed");
        assert!(
            inspection.is_static_image,
            "png should be treated as a static image"
        );
        assert!(
            inspection.can_convert_to_png,
            "png sources should still expose the conversion path for cropped export"
        );
    }

    #[test]
    fn build_static_image_filter_graph_caps_output_to_max_320() {
        let graph = build_static_image_filter_graph(
            Some(640),
            Some(480),
            Some(ResolvedCropRegion {
                x: 0,
                y: 0,
                width: 400,
                height: 200,
            }),
        );

        assert!(graph.contains("crop=400:200:0:0"));
        assert!(graph.contains("scale=320:160"));
        assert!(graph.contains("format=rgba,setsar=1"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn convert_static_image_to_png_internal_converts_jpg_sources() {
        let test_dir = TestDir::new("static-jpg-conversion");
        let input_path = create_static_image(&test_dir, "input.jpg", "green");

        let result = convert_static_image_to_png_internal(
            &input_path,
            Some(test_dir.path.to_string_lossy().as_ref()),
            None,
            UiLocale::En,
            None,
        );

        assert!(result.ok, "jpg conversion should succeed");
        assert_eq!(result.tool_source.as_deref(), Some("native"));
        assert_eq!(result.tool_command, None);
        let output_path = result
            .output_path
            .clone()
            .expect("conversion should produce an output path");
        assert!(
            Path::new(&output_path).is_file(),
            "converted png should exist"
        );
        assert!(
            output_path.ends_with(".png"),
            "converted output should be a png file"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn convert_static_image_to_png_internal_downscales_large_crops_to_fit_320() {
        let test_dir = TestDir::new("static-large-conversion");
        let input_path = create_static_image_with_size(&test_dir, "large.jpg", "yellow", 640, 480);
        let crop_region = CropRegion {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };

        let result = convert_static_image_to_png_internal(
            &input_path,
            Some(test_dir.path.to_string_lossy().as_ref()),
            Some(&crop_region),
            UiLocale::En,
            None,
        );

        assert!(result.ok, "large static image conversion should succeed");
        assert_eq!(result.tool_source.as_deref(), Some("native"));
        assert_eq!(result.tool_command, None);
        let output_path = result
            .output_path
            .clone()
            .expect("conversion should produce an output path");
        let inspection = inspect_input_media_internal(&output_path, UiLocale::En);

        assert!(inspection.ok, "converted image inspection should succeed");
        assert_eq!(inspection.width, Some(320));
        assert_eq!(inspection.height, Some(240));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn convert_static_image_to_png_internal_allows_png_sources() {
        let test_dir = TestDir::new("static-png-conversion");
        let input_path = create_static_image(&test_dir, "input.png", "blue");

        let result = convert_static_image_to_png_internal(
            &input_path,
            Some(test_dir.path.to_string_lossy().as_ref()),
            None,
            UiLocale::En,
            None,
        );

        assert!(result.ok, "png conversion should succeed");
        assert_eq!(result.tool_source.as_deref(), Some("native"));
        assert_eq!(result.tool_command, None);
        let output_path = result
            .output_path
            .clone()
            .expect("conversion should produce an output path");
        assert!(
            Path::new(&output_path).is_file(),
            "converted png should exist"
        );
        assert_ne!(
            output_path.replace('\\', "/"),
            input_path.replace('\\', "/"),
            "conversion should create a new png file instead of mutating the source"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn convert_static_image_to_png_internal_preserves_unicode_source_stem() {
        let test_dir = TestDir::new("static-unicode-conversion");
        let input_path = create_static_image(&test_dir, "고양이 스티커.png", "blue");

        let result = convert_static_image_to_png_internal(
            &input_path,
            Some(test_dir.path.to_string_lossy().as_ref()),
            None,
            UiLocale::En,
            None,
        );

        let output_path = result
            .output_path
            .expect("Unicode source conversion must publish an output");
        let output_name = Path::new(&output_path)
            .file_name()
            .and_then(|name| name.to_str())
            .expect("output name must be valid Unicode");
        assert!(result.ok);
        assert!(output_name.starts_with("고양이 스티커-png-"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn encode_candidate_internal_respects_timeline_frame_order_and_durations() {
        let test_dir = TestDir::new("timeline-encode");
        let input_path = create_three_frame_animation(&test_dir);
        let candidate = build_test_candidate("timeline-sequence", 8);
        let timeline_frames = vec![
            ResolvedTimelineFrame {
                source_frame_index: 2,
                duration_us: 120_000,
            },
            ResolvedTimelineFrame {
                source_frame_index: 0,
                duration_us: 240_000,
            },
            ResolvedTimelineFrame {
                source_frame_index: 2,
                duration_us: 360_000,
            },
        ];

        let result = encode_candidate_internal(
            &input_path,
            Some(test_dir.path.to_string_lossy().as_ref()),
            UiLocale::En,
            None,
            Some(48),
            Some(48),
            &candidate,
            None,
            Some(timeline_frames.as_slice()),
            None,
        )
        .expect("timeline-based encoding should succeed");
        assert_eq!(result.tool_source, "native");
        assert_eq!(result.tool_command, None);

        let output_path = result
            .pending_output
            .commit()
            .expect("timeline output must commit");
        let inspection =
            inspect_input_media_internal(output_path.to_string_lossy().as_ref(), UiLocale::En);
        let frame_durations = inspection
            .frame_durations_seconds
            .expect("encoded timeline should report frame durations");

        assert!(inspection.ok, "inspection should succeed");
        assert_eq!(inspection.estimated_frames, Some(3));
        assert_eq!(frame_durations.len(), 3);
        assert!(approx_eq(frame_durations[0], 0.12));
        assert!(approx_eq(frame_durations[1], 0.24));
        assert!(approx_eq(frame_durations[2], 0.36));
        assert!(approx_eq(frame_durations.iter().sum::<f64>(), 0.72));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn run_optimizer_search_internal_accepts_timeline_frames() {
        let test_dir = TestDir::new("timeline-search");
        let input_path = create_three_frame_animation(&test_dir);
        let source_revision =
            SourceIdentity::from_path(Path::new(&input_path), MediaLimits::default())
                .expect("timeline source identity must be created")
                .revision();

        let response = run_optimizer_search_internal(
            OptimizerSearchRequest {
                input_path,
                source_revision: Some(source_revision),
                output_directory: Some(test_dir.path.to_string_lossy().into_owned()),
                locale: Some("en".into()),
                source_duration_seconds: Some(0.72),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(7.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: None,
                base_frame_count: Some(3),
                timeline_frames: Some(vec![
                    EditedTimelineFrame {
                        source_frame_id: 1,
                        duration_us: 120_000,
                    },
                    EditedTimelineFrame {
                        source_frame_id: 2,
                        duration_us: 240_000,
                    },
                    EditedTimelineFrame {
                        source_frame_id: 3,
                        duration_us: 360_000,
                    },
                ]),
            },
            UiLocale::En,
        );

        assert!(
            !response.attempts.is_empty(),
            "search should try at least one candidate"
        );
        assert!(
            response
                .attempts
                .iter()
                .any(|attempt| attempt.output_path.is_some()),
            "timeline search should produce at least one encoded output",
        );
    }

    #[test]
    fn run_optimizer_search_rejects_empty_frame_selection() {
        let (_test_dir, input_path, source_revision) =
            revisioned_placeholder("empty-frame-selection");
        let response = run_optimizer_search_internal(
            OptimizerSearchRequest {
                input_path,
                source_revision: Some(source_revision),
                output_directory: None,
                locale: Some("en".into()),
                source_duration_seconds: Some(1.0),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(7.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: Some(Vec::new()),
                base_frame_count: Some(7),
                timeline_frames: None,
            },
            UiLocale::En,
        );

        assert!(!response.ok);
        assert_eq!(response.error_code.as_deref(), Some("invalid-request"));
        assert_eq!(response.reason_code.as_deref(), Some("no-frames-selected"));
        assert_eq!(
            response.error_message.as_deref(),
            Some("Select at least one frame before exporting.")
        );
    }

    #[test]
    fn plan_and_search_preserve_invalid_frame_duration_for_99_microseconds() {
        let timeline_frames = Some(vec![EditedTimelineFrame {
            source_frame_id: 1,
            duration_us: 99,
        }]);
        let plan = prepare_optimizer_plan(
            &OptimizerPlanRequest {
                locale: Some("en".into()),
                source_duration_seconds: Some(0.000099),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(30.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: None,
                base_frame_count: Some(1),
                timeline_frames: timeline_frames.clone(),
            },
            UiLocale::En,
        );
        assert!(!plan.ok);
        assert_eq!(plan.error_code.as_deref(), Some("invalid-request"));
        assert_eq!(plan.reason_code.as_deref(), Some("invalid-frame-duration"));
        assert_eq!(
            plan.error_message.as_deref(),
            Some("One or more frame durations are invalid.")
        );

        let (_test_dir, input_path, source_revision) =
            revisioned_placeholder("invalid-frame-duration-search");
        let search = run_optimizer_search_internal(
            OptimizerSearchRequest {
                input_path,
                source_revision: Some(source_revision),
                output_directory: None,
                locale: Some("en".into()),
                source_duration_seconds: Some(0.000099),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(30.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: None,
                base_frame_count: Some(1),
                timeline_frames,
            },
            UiLocale::En,
        );
        assert!(!search.ok);
        assert!(search.attempts.is_empty());
        assert_eq!(search.error_code.as_deref(), Some("invalid-request"));
        assert_eq!(
            search.reason_code.as_deref(),
            Some("invalid-frame-duration")
        );
        assert_eq!(
            search.error_message.as_deref(),
            Some("One or more frame durations are invalid.")
        );
    }

    #[test]
    fn run_optimizer_search_rejects_out_of_range_frame_selection() {
        let (_test_dir, input_path, source_revision) =
            revisioned_placeholder("out-of-range-frame-selection");
        let response = run_optimizer_search_internal(
            OptimizerSearchRequest {
                input_path,
                source_revision: Some(source_revision),
                output_directory: None,
                locale: Some("en".into()),
                source_duration_seconds: Some(1.0),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(7.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: Some(vec![8]),
                base_frame_count: Some(7),
                timeline_frames: None,
            },
            UiLocale::En,
        );

        assert!(!response.ok);
        assert_eq!(response.error_code.as_deref(), Some("invalid-request"));
        assert_eq!(
            response.reason_code.as_deref(),
            Some("invalid-frame-selection")
        );
        assert_eq!(
            response.error_message.as_deref(),
            Some("The selected frame list does not match the available frame markers.")
        );
    }

    #[test]
    fn run_optimizer_search_aborts_on_operation_wide_output_directory_error() {
        let (test_dir, input_path, source_revision) =
            revisioned_placeholder("search-output-directory-error");
        let output_file = test_dir.path.join("not-a-directory");
        fs::write(&output_file, b"file").expect("output conflict fixture must be written");

        let response = run_optimizer_search_internal(
            OptimizerSearchRequest {
                input_path,
                source_revision: Some(source_revision),
                output_directory: Some(output_file.to_string_lossy().into_owned()),
                locale: Some("en".into()),
                source_duration_seconds: Some(0.1),
                input_width: Some(1),
                input_height: Some(1),
                avg_fps: Some(10.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: None,
                base_frame_count: Some(1),
                timeline_frames: None,
            },
            UiLocale::En,
        );

        assert!(!response.ok);
        assert!(response.attempts.is_empty());
        assert_eq!(response.real_attempt_count, 0);
        assert_eq!(response.error_code.as_deref(), Some("invalid-request"));
        assert_eq!(
            response.reason_code.as_deref(),
            Some("invalid-output-directory")
        );
    }

    #[test]
    fn prepare_optimizer_plan_uses_selected_frames_as_duration_source() {
        let response = prepare_optimizer_plan(
            &OptimizerPlanRequest {
                locale: Some("en".into()),
                source_duration_seconds: Some(1.0),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(7.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: Some(vec![1, 3, 7]),
                base_frame_count: Some(7),
                timeline_frames: None,
            },
            UiLocale::En,
        );

        assert!(response.ok);
        assert_eq!(response.selected_duration_seconds, Some(3.0 / 7.0));
        assert_eq!(
            response
                .candidates
                .first()
                .map(|candidate| candidate.duration_seconds),
            Some(3.0 / 7.0)
        );
    }

    #[test]
    fn prepare_optimizer_plan_rejects_long_selection_instead_of_speeding_it_up() {
        let response = prepare_optimizer_plan(
            &OptimizerPlanRequest {
                locale: Some("en".into()),
                source_duration_seconds: Some(8.0),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(24.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: Some((1..=189).collect()),
                base_frame_count: Some(189),
                timeline_frames: None,
            },
            UiLocale::En,
        );

        assert!(!response.ok);
        assert_eq!(response.reason_code.as_deref(), Some("duration-too-long"));
        assert_eq!(response.selected_duration_seconds, Some(8.0));
        assert!(response.candidates.is_empty());
    }

    #[test]
    fn prepared_duration_is_identical_across_candidate_fps_and_sample_steps() {
        let response = prepare_optimizer_plan(
            &OptimizerPlanRequest {
                locale: Some("en".into()),
                source_duration_seconds: Some(4.8),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(25.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: Some("deep".into()),
                crop_region: None,
                selected_frames: Some((1..=100).collect()),
                base_frame_count: Some(120),
                timeline_frames: None,
            },
            UiLocale::En,
        );

        assert!(response.ok);
        assert_eq!(response.selected_duration_seconds, Some(4.0));
        assert!(response
            .candidates
            .iter()
            .any(|candidate| candidate.frame_sample_step > 1));
        assert!(
            response
                .candidates
                .iter()
                .map(|candidate| candidate.fps)
                .collect::<BTreeSet<_>>()
                .len()
                > 1
        );
        assert!(response
            .candidates
            .iter()
            .all(|candidate| approx_eq(candidate.duration_seconds, 4.0)));
    }

    #[test]
    fn quality_goal_removes_every_nth_frame_from_selection() {
        let response = prepare_optimizer_plan(
            &OptimizerPlanRequest {
                locale: Some("en".into()),
                source_duration_seconds: Some(1.0),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(9.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: Some("quality".into()),
                quality_frame_drop_interval: Some(3),
                search_depth: None,
                crop_region: None,
                selected_frames: Some((1..=9).collect()),
                base_frame_count: Some(9),
                timeline_frames: None,
            },
            UiLocale::En,
        );

        assert!(response.ok);
        assert_eq!(response.selected_duration_seconds, Some(6.0 / 9.0));
        assert!(response
            .candidates
            .iter()
            .all(|candidate| candidate.frame_sample_step == 1));
    }

    #[test]
    fn quality_goal_zero_frame_drop_disables_automatic_sampling() {
        let response = prepare_optimizer_plan(
            &OptimizerPlanRequest {
                locale: Some("en".into()),
                source_duration_seconds: Some(8.0),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(24.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: Some("quality".into()),
                quality_frame_drop_interval: Some(0),
                search_depth: None,
                crop_region: None,
                selected_frames: Some((1..=189).collect()),
                base_frame_count: Some(189),
                timeline_frames: None,
            },
            UiLocale::En,
        );

        assert!(!response.ok);
        assert_eq!(response.error_code.as_deref(), Some("invalid-request"));
        assert_eq!(response.reason_code.as_deref(), Some("duration-too-long"));
    }

    #[test]
    fn motion_goal_rejects_source_duration_over_five_seconds() {
        let response = prepare_optimizer_plan(
            &OptimizerPlanRequest {
                locale: Some("en".into()),
                source_duration_seconds: Some(5.88),
                input_width: Some(100),
                input_height: Some(100),
                avg_fps: Some(8.333),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: Some("motion".into()),
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: Some((1..=49).collect()),
                base_frame_count: Some(49),
                timeline_frames: None,
            },
            UiLocale::En,
        );

        assert!(!response.ok);
        assert_eq!(response.reason_code.as_deref(), Some("duration-too-long"));
        assert!(response
            .selected_duration_seconds
            .is_some_and(|duration| approx_eq(duration, 5.88)));
        assert!(response.candidates.is_empty());
    }

    #[test]
    fn timeline_duration_us_accepts_exact_discord_limit() {
        let frames = vec![
            ResolvedTimelineFrame {
                source_frame_index: 0,
                duration_us: 1_666_667,
            },
            ResolvedTimelineFrame {
                source_frame_index: 1,
                duration_us: 1_666_667,
            },
            ResolvedTimelineFrame {
                source_frame_index: 2,
                duration_us: 1_666_666,
            },
        ];

        assert_eq!(timeline_duration_us(&frames), Ok(DISCORD_MAX_DURATION_US));
    }

    #[test]
    fn timeline_duration_limit_rejects_one_microsecond_over_before_writing() {
        let response = prepare_optimizer_plan(
            &OptimizerPlanRequest {
                locale: Some("en".into()),
                source_duration_seconds: Some(5.000001),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(0.6),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: None,
                base_frame_count: Some(3),
                timeline_frames: Some(vec![
                    EditedTimelineFrame {
                        source_frame_id: 1,
                        duration_us: 1_666_667,
                    },
                    EditedTimelineFrame {
                        source_frame_id: 2,
                        duration_us: 1_666_667,
                    },
                    EditedTimelineFrame {
                        source_frame_id: 3,
                        duration_us: 1_666_667,
                    },
                ]),
            },
            UiLocale::En,
        );

        assert!(!response.ok);
        assert!(response.candidates.is_empty());
        assert_eq!(response.error_code.as_deref(), Some("invalid-request"));
        assert_eq!(response.reason_code.as_deref(), Some("duration-too-long"));
    }

    #[test]
    fn timeline_duration_us_rejects_checked_add_overflow() {
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

        assert_eq!(
            timeline_duration_us(&frames),
            Err("invalid-frame-selection")
        );
    }

    #[test]
    fn sticker_frame_duration_us_rejects_checked_add_overflow() {
        let frames = vec![
            StickerFrame {
                pixels: RgbaImage::new(1, 1),
                duration_us: u64::MAX,
            },
            StickerFrame {
                pixels: RgbaImage::new(1, 1),
                duration_us: 1,
            },
        ];

        assert_eq!(
            sticker_frame_duration_us(&frames),
            Err("invalid-frame-selection")
        );
    }

    #[test]
    fn quantize_apng_delays_uses_positive_hundred_microsecond_ticks() {
        let delays = quantize_apng_delays(&[1_666_667, 1_666_667, 1_666_666])
            .expect("valid frame durations should quantize");

        assert!(delays.iter().all(|(numerator, _)| *numerator >= 1));
        assert!(delays.iter().all(|(_, denominator)| *denominator == 10_000));
    }

    #[test]
    fn quantize_apng_delays_keeps_total_error_within_half_a_tick() {
        let delays = quantize_apng_delays(&[1_666_667, 1_666_667, 1_666_666])
            .expect("valid frame durations should quantize");
        let quantized_total_us = delays
            .iter()
            .map(|(numerator, _)| u64::from(*numerator) * 100)
            .sum::<u64>();

        assert!(quantized_total_us.abs_diff(5_000_000) <= 50);
    }

    #[test]
    fn quantize_apng_delays_rejects_sub_tick_frame_duration() {
        assert_eq!(quantize_apng_delays(&[99]), Err("invalid-frame-duration"));
    }

    #[test]
    fn quantize_apng_delays_rejects_checked_cumulative_overflow() {
        assert_eq!(
            quantize_apng_delays(&[u64::MAX, 100]),
            Err("invalid-frame-selection")
        );
    }

    #[test]
    fn resolve_timeline_frames_preserves_order_and_duplicates() {
        let timeline_frames = resolve_timeline_frames(
            Some(&vec![
                EditedTimelineFrame {
                    source_frame_id: 3,
                    duration_us: 120_000,
                },
                EditedTimelineFrame {
                    source_frame_id: 1,
                    duration_us: 240_000,
                },
                EditedTimelineFrame {
                    source_frame_id: 3,
                    duration_us: 360_000,
                },
            ]),
            Some(7),
        )
        .expect("timeline frames should resolve")
        .expect("timeline frames should be present");

        assert_eq!(timeline_frames.len(), 3);
        assert_eq!(timeline_frames[0].source_frame_index, 2);
        assert_eq!(timeline_frames[0].duration_us, 120_000);
        assert_eq!(timeline_frames[1].source_frame_index, 0);
        assert_eq!(timeline_frames[1].duration_us, 240_000);
        assert_eq!(timeline_frames[2].source_frame_index, 2);
        assert_eq!(timeline_frames[2].duration_us, 360_000);
    }

    #[test]
    fn prepare_optimizer_plan_uses_timeline_frame_durations_when_present() {
        let response = prepare_optimizer_plan(
            &OptimizerPlanRequest {
                locale: Some("en".into()),
                source_duration_seconds: Some(1.0),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(7.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: None,
                base_frame_count: Some(7),
                timeline_frames: Some(vec![
                    EditedTimelineFrame {
                        source_frame_id: 1,
                        duration_us: 120_000,
                    },
                    EditedTimelineFrame {
                        source_frame_id: 3,
                        duration_us: 240_000,
                    },
                    EditedTimelineFrame {
                        source_frame_id: 1,
                        duration_us: 360_000,
                    },
                ]),
            },
            UiLocale::En,
        );

        assert!(response.ok);
        assert_eq!(response.selected_duration_seconds, Some(0.72));
        assert!(response
            .candidates
            .iter()
            .all(|candidate| approx_eq(candidate.duration_seconds, 0.72)));
    }

    #[test]
    fn animation_iterator_visits_decoded_frames_incrementally_in_order() {
        let context = OperationContext::detached(Duration::from_secs(1));
        let emitted = Rc::new(Cell::new(0_u8));
        let observed = Rc::new(Cell::new(0_u8));
        let iterator_emitted = Rc::clone(&emitted);
        let iterator_observed = Rc::clone(&observed);
        let frames = std::iter::from_fn(move || {
            let value = iterator_emitted.get();
            if value >= 3 {
                return None;
            }
            assert_eq!(
                value,
                iterator_observed.get(),
                "decoder iterator advanced before the previous frame was visited"
            );
            iterator_emitted.set(value + 1);
            Some(Ok(image::Frame::new(RgbaImage::from_pixel(
                1,
                1,
                Rgba([value, 0, 0, 255]),
            ))))
        });
        let mut visited = Vec::new();

        let count = visit_decoded_animation_frame_iterator(
            "test",
            frames,
            Some(3),
            MediaLimits::default(),
            &context,
            |source_frame_id, frame| {
                visited.push((source_frame_id, frame.buffer().get_pixel(0, 0).0[0]));
                observed.set(source_frame_id as u8);
                Ok(())
            },
        )
        .expect("incremental iterator must be accepted");

        assert_eq!(count, 3);
        assert_eq!(visited, vec![(1, 0), (2, 1), (3, 2)]);
    }

    #[test]
    fn animation_iterator_rejects_frame_301_before_visiting_it() {
        let context = OperationContext::detached(Duration::from_secs(1));
        let frames = (0..301).map(|_| Ok(image::Frame::new(RgbaImage::new(1, 1))));
        let mut visited = 0_u32;

        let error = visit_decoded_animation_frame_iterator(
            "test",
            frames,
            Some(301),
            MediaLimits::default(),
            &context,
            |_, _| {
                visited += 1;
                Ok(())
            },
        )
        .expect_err("frame 301 must exceed the bounded iterator limit");

        assert_eq!(visited, 300);
        assert!(matches!(
            error,
            PipelineError::LimitExceeded {
                resource: "frame-count",
                limit: 300,
                actual: 301,
            }
        ));
    }

    #[test]
    fn prepared_video_filter_selects_unique_indexes_then_crops_and_scales() {
        let indexes = [0, 4, 7].into_iter().collect::<BTreeSet<_>>();
        let filter = build_prepared_video_filter(
            &indexes,
            Some(ResolvedCropRegion {
                x: 10,
                y: 20,
                width: 640,
                height: 320,
            }),
            320,
            160,
        );

        assert_eq!(
            filter,
            "select='eq(n,0)+eq(n,4)+eq(n,7)',crop=640:320:10:20,scale=320:160:flags=lanczos,format=rgba,setsar=1"
        );
    }

    #[test]
    fn video_preparation_maps_first_video_stream_without_cfr_duplication() {
        let source = include_str!("lib.rs");
        let preparation = source_section(
            source,
            "fn prepare_video_search_source(",
            "impl FrameSourceLoader for DefaultFrameSourceLoader",
        );

        assert!(preparation.contains("OsString::from(\"-map\")"));
        assert!(preparation.contains("OsString::from(\"0:v:0\")"));
        assert!(preparation.contains("OsString::from(\"-fps_mode\")"));
        assert!(preparation.contains("OsString::from(\"passthrough\")"));
        assert_eq!(preparation.matches("stream_fixed_rgba_frames(").count(), 1);
    }

    #[test]
    fn video_preparation_union_covers_the_full_temporal_candidate_universe() {
        let base_sequence = (0..180)
            .map(|index| ResolvedTimelineFrame {
                source_frame_index: index,
                duration_us: 10_000 + u64::from(index % 7) * 3_000,
            })
            .collect::<Vec<_>>();
        let base_fps = prepared_sequence_base_fps(&base_sequence).expect("base FPS must resolve");
        let context = OperationContext::detached(Duration::from_secs(1));
        let required =
            required_video_source_indexes(&base_sequence, base_fps, "balanced", &context)
                .expect("preparation union must resolve");
        let placeholder = PreparedSearchSource {
            frames: Vec::new(),
            base_sequence: base_sequence.clone(),
            timing_authority: TimelineTimingAuthority::Inspected,
            base_fps,
            decoded_bytes: 0,
            tool_source: "test".into(),
            tool_command: None,
            tool_detail: None,
        };
        let duration_seconds =
            duration_us_to_seconds(checked_timeline_duration(&base_sequence).unwrap());
        let candidates = build_candidate_universe_with_checkpoint(
            base_sequence.len(),
            f64::from(base_fps),
            duration_seconds,
            Some(640),
            Some(320),
            "balanced",
            "balanced",
            UiLocale::En,
            &mut || Ok(()),
        )
        .expect("full candidate universe must build");

        assert!(required.len() <= MAX_SEARCH_OUTPUT_FRAMES);
        for candidate in &candidates {
            let sequence = build_candidate_output_sequence(&placeholder, candidate, &context)
                .expect("candidate sequence must build");
            assert!(sequence
                .frames
                .iter()
                .all(|frame| required.contains(&frame.source_frame_index)));
        }
        let authored_fps_candidate = build_test_candidate("authored-23fps", 23);
        let authored_sequence =
            build_candidate_output_sequence(&placeholder, &authored_fps_candidate, &context)
                .expect("authored fixed-FPS candidate must build");
        assert!(authored_sequence
            .frames
            .iter()
            .all(|frame| required.contains(&frame.source_frame_index)));
    }

    #[test]
    fn production_search_prepares_once_before_the_candidate_loop() {
        let source = include_str!("lib.rs");
        let search = source_section(
            source,
            "fn run_optimizer_search_with_loader(",
            "mod tests {",
        );
        let prepare_offset = search.find("loader.prepare(").expect("prepare call");
        let loop_offset = search
            .find("for candidate in &plan.candidates")
            .expect("candidate loop");

        assert_eq!(search.matches("loader.prepare(").count(), 1);
        assert!(prepare_offset < loop_offset);
        assert!(!search[loop_offset..].contains("loader.prepare("));
        assert!(!search[loop_offset..].contains("decode_native_animation_frames("));
        assert!(!search[loop_offset..].contains("stream_fixed_rgba_frames("));
    }

    #[test]
    fn native_preparation_deduplicates_pixels_and_uses_decoded_crop_dimensions() {
        let test_dir = TestDir::new("native-prepared-dedup");
        let input_path = test_dir.path.join("input.apng");
        write_native_apng_file(
            &input_path,
            &[
                StickerFrame {
                    pixels: RgbaImage::new(640, 320),
                    duration_us: 100_000,
                },
                StickerFrame {
                    pixels: RgbaImage::new(640, 320),
                    duration_us: 100_000,
                },
            ],
            "standard",
        )
        .expect("native preparation fixture must be written");
        let timeline = vec![
            ResolvedTimelineFrame {
                source_frame_index: 0,
                duration_us: 120_000,
            },
            ResolvedTimelineFrame {
                source_frame_index: 0,
                duration_us: 240_000,
            },
        ];
        let candidate = build_test_candidate("native-prepared", 10);
        let plan = OptimizerPlanResponse {
            ok: true,
            fit_mode: CANONICAL_FIT_MODE.into(),
            selected_duration_seconds: Some(0.36),
            recommended_max_duration_seconds: 3.0,
            search_budget: 1,
            warnings: Vec::new(),
            candidates: vec![candidate],
            error_code: None,
            reason_code: None,
            error_message: None,
        };
        let crop = CropRegion {
            x: 0.0,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        };
        let context = OperationContext::detached(Duration::from_secs(1));
        let loader = DefaultFrameSourceLoader;

        let prepared = loader
            .prepare(
                FramePreparationRequest {
                    input_path: &input_path,
                    source_revision: "test",
                    crop_region: Some(&crop),
                    input_width: Some(9_999),
                    input_height: Some(9_999),
                    base_frame_count: Some(2),
                    timeline_frames: None,
                    resolved_timeline_frames: Some(&timeline),
                    selected_frame_indexes: None,
                    source_duration_seconds: Some(0.2),
                    avg_fps: Some(10.0),
                    locale: UiLocale::En,
                    optimizer_goal: "balanced",
                },
                &plan,
                &context,
                MediaLimits::default(),
            )
            .expect("native search source must prepare");

        assert_eq!(prepared.frames.len(), 1);
        assert_eq!(prepared.base_sequence, timeline);
        assert_eq!(prepared.decoded_bytes, 320 * 320 * 4);
        assert_eq!(prepared.frames[0].pixels.dimensions(), (320, 320));
        let first = prepared
            .pixels_for_source_index(0)
            .expect("first duplicate must resolve");
        let duplicate = prepared
            .pixels_for_source_index(0)
            .expect("second duplicate must resolve");
        assert!(Arc::ptr_eq(first, duplicate));
    }

    #[test]
    fn one_prepared_source_load_is_independent_of_candidate_count() {
        let test_dir = TestDir::new("one-prepared-source");
        let input_path = test_dir.path.join("input.mp4");
        fs::write(&input_path, b"frame-source-loader-seam")
            .expect("source identity fixture must be written");
        let source_identity = SourceIdentity::from_path(&input_path, MediaLimits::default())
            .expect("source identity must resolve");
        let loader = CountingFrameSourceLoader::new();

        let response = run_optimizer_search_with_loader(
            OptimizerSearchRequest {
                input_path: input_path.to_string_lossy().into_owned(),
                source_revision: Some(source_identity.revision()),
                output_directory: Some(test_dir.path.to_string_lossy().into_owned()),
                locale: Some("en".into()),
                source_duration_seconds: Some(0.1),
                input_width: Some(1),
                input_height: Some(1),
                avg_fps: Some(10.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: Some("deep".into()),
                crop_region: None,
                selected_frames: None,
                base_frame_count: Some(1),
                timeline_frames: None,
            },
            UiLocale::En,
            None,
            &loader,
            || Ok(()),
            |_, _, _| {},
        );

        assert!(response.search_budget > 1);
        assert_eq!(loader.prepare_count.load(AtomicOrdering::SeqCst), 1);
        assert!(response.attempts.len() > 1);
    }

    #[test]
    fn source_mutation_during_preparation_is_rejected_before_candidate_encoding() {
        let test_dir = TestDir::new("prepared-source-mutation");
        let input_path = test_dir.path.join("input.mp4");
        fs::write(&input_path, b"original-source")
            .expect("source mutation fixture must be written");
        let source_identity = SourceIdentity::from_path(&input_path, MediaLimits::default())
            .expect("source identity must resolve");

        let response = run_optimizer_search_with_loader(
            OptimizerSearchRequest {
                input_path: input_path.to_string_lossy().into_owned(),
                source_revision: Some(source_identity.revision()),
                output_directory: Some(test_dir.path.to_string_lossy().into_owned()),
                locale: Some("en".into()),
                source_duration_seconds: Some(0.1),
                input_width: Some(1),
                input_height: Some(1),
                avg_fps: Some(10.0),
                fit_mode: Some("contain".into()),
                preset_strategy: None,
                optimizer_goal: None,
                quality_frame_drop_interval: None,
                search_depth: None,
                crop_region: None,
                selected_frames: None,
                base_frame_count: Some(1),
                timeline_frames: None,
            },
            UiLocale::En,
            None,
            &MutatingFrameSourceLoader,
            || Ok(()),
            |_, _, _| {},
        );

        assert!(!response.ok);
        assert_eq!(response.error_code.as_deref(), Some("source-changed"));
        assert!(response.attempts.is_empty());
    }

    #[test]
    fn prepared_duration_limit_error_keeps_authoritative_selected_duration() {
        let mut plan = OptimizerPlanResponse {
            ok: true,
            fit_mode: CANONICAL_FIT_MODE.into(),
            selected_duration_seconds: Some(2.0),
            recommended_max_duration_seconds: 3.0,
            search_budget: 1,
            warnings: Vec::new(),
            candidates: vec![build_test_candidate("stale-duration", 10)],
            error_code: None,
            reason_code: None,
            error_message: None,
        };
        let prepared = PreparedSearchSource {
            frames: vec![PreparedFrame {
                source_frame_id: 1,
                pixels: Arc::new(RgbaImage::new(1, 1)),
                duration_us: 6_000_000,
            }],
            base_sequence: vec![ResolvedTimelineFrame {
                source_frame_index: 0,
                duration_us: 6_000_000,
            }],
            timing_authority: TimelineTimingAuthority::Native,
            base_fps: 1,
            decoded_bytes: 4,
            tool_source: "native".into(),
            tool_command: None,
            tool_detail: None,
        };
        let context = OperationContext::detached(Duration::from_secs(1));

        let error = synchronize_candidates_with_prepared_duration(
            &mut plan,
            &prepared,
            Some(1),
            Some(1),
            "balanced",
            "balanced",
            UiLocale::En,
            &context,
        )
        .expect_err("prepared duration above five seconds must be rejected");

        assert_eq!(
            error,
            PipelineError::InvalidRequest {
                reason: "duration-too-long"
            }
        );
        assert_eq!(plan.selected_duration_seconds, Some(6.0));
    }

    #[test]
    fn authored_candidate_universe_clamps_reported_fps_to_thirty() {
        let mut plan = OptimizerPlanResponse {
            ok: true,
            fit_mode: CANONICAL_FIT_MODE.into(),
            selected_duration_seconds: Some(1.0),
            recommended_max_duration_seconds: 3.0,
            search_budget: 20,
            warnings: Vec::new(),
            candidates: vec![build_test_candidate("stale-authored-fps", 30)],
            error_code: None,
            reason_code: None,
            error_message: None,
        };
        let prepared = PreparedSearchSource {
            frames: Vec::new(),
            base_sequence: (0..300)
                .map(|index| ResolvedTimelineFrame {
                    source_frame_index: index,
                    duration_us: 3_333,
                })
                .collect(),
            timing_authority: TimelineTimingAuthority::Authored,
            base_fps: 300,
            decoded_bytes: 0,
            tool_source: "test".into(),
            tool_command: None,
            tool_detail: None,
        };
        let context = OperationContext::detached(Duration::from_secs(1));

        synchronize_candidates_with_prepared_duration(
            &mut plan,
            &prepared,
            Some(320),
            Some(320),
            "balanced",
            "balanced",
            UiLocale::En,
            &context,
        )
        .expect("authored universe must synchronize");

        assert!(!plan.candidates.is_empty());
        assert!(plan.candidates.iter().all(|candidate| candidate.fps == 30));
    }

    #[test]
    fn streaming_apng_writer_rejects_short_and_long_iterators() {
        let frame = StickerFrame {
            pixels: RgbaImage::new(2, 2),
            duration_us: 100_000,
        };
        let short = vec![Ok::<_, PipelineError>(frame.clone())].into_iter();
        let short_error = write_native_apng_iterator_with_checkpoint(
            Vec::new(),
            2,
            &[100_000, 100_000],
            short,
            "standard",
            || Ok(()),
        )
        .expect_err("short frame iterator must be rejected");
        assert!(matches!(
            short_error,
            PipelineError::MalformedProcessOutput { .. }
        ));

        let long = vec![
            Ok::<_, PipelineError>(frame.clone()),
            Ok::<_, PipelineError>(frame),
        ]
        .into_iter();
        let long_error = write_native_apng_iterator_with_checkpoint(
            Vec::new(),
            1,
            &[100_000],
            long,
            "standard",
            || Ok(()),
        )
        .expect_err("long frame iterator must be rejected");
        assert!(matches!(
            long_error,
            PipelineError::MalformedProcessOutput { .. }
        ));
    }

    #[test]
    fn streaming_apng_writer_checks_cancellation_before_each_frame() {
        let frames = (0..3)
            .map(|_| {
                Ok::<_, PipelineError>(StickerFrame {
                    pixels: RgbaImage::new(2, 2),
                    duration_us: 100_000,
                })
            })
            .collect::<Vec<_>>()
            .into_iter();
        let checkpoints = Cell::new(0_u32);

        let error = write_native_apng_iterator_with_checkpoint(
            Vec::new(),
            3,
            &[100_000; 3],
            frames,
            "standard",
            || {
                let next = checkpoints.get() + 1;
                checkpoints.set(next);
                if next >= 4 {
                    Err(PipelineError::Cancelled)
                } else {
                    Ok(())
                }
            },
        )
        .expect_err("cancellation must stop streaming APNG encoding");

        assert_eq!(error, PipelineError::Cancelled);
        assert!(checkpoints.get() >= 4);
    }

    #[test]
    fn two_stage_prepared_resize_stays_within_visual_error_budget() {
        let mut source = RgbaImage::new(640, 320);
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            let gradient = ((x * 255) / 639) as u8;
            let checker = if (x / 32 + y / 32) % 2 == 0 {
                8_u8
            } else {
                0_u8
            };
            *pixel = Rgba([
                gradient.saturating_add(checker),
                ((y * 255) / 319) as u8,
                255_u8.saturating_sub(gradient),
                255,
            ]);
        }

        let direct = transform_frame_for_candidate(&source, 0.5, None);
        let prepared = prepare_frame_for_search(source, None);
        let two_stage = scale_prepared_frame_for_candidate(&prepared, 0.5);
        let absolute_error = direct
            .as_raw()
            .iter()
            .zip(two_stage.as_raw())
            .map(|(left, right)| left.abs_diff(*right) as f64)
            .sum::<f64>();
        let mean_absolute_channel_error = absolute_error / direct.as_raw().len() as f64;

        assert_eq!(direct.dimensions(), two_stage.dimensions());
        assert!(
            mean_absolute_channel_error <= 1.5,
            "two-stage resize MAE was {mean_absolute_channel_error}"
        );
    }

    #[test]
    fn preview_ids_are_stably_deduplicated_and_bounded_to_twenty_four() {
        assert_eq!(
            normalize_preview_source_frame_ids(&[50, 1, 20, 1]),
            Ok(vec![50, 1, 20])
        );
        assert_eq!(
            normalize_preview_source_frame_ids(&(1..=24).collect::<Vec<_>>()),
            Ok((1..=24).collect::<Vec<_>>())
        );
        assert!(normalize_preview_source_frame_ids(&(1..=25).collect::<Vec<_>>()).is_err());
        assert!(normalize_preview_source_frame_ids(&[1, 0, 2]).is_err());
        assert!(normalize_preview_source_frame_ids(&[]).is_err());
    }

    #[test]
    fn preview_dimensions_cap_landscape_and_portrait_without_upscaling() {
        assert_eq!(preview_output_dimensions(640, 320), Ok((128, 64)));
        assert_eq!(preview_output_dimensions(320, 640), Ok((64, 128)));
        assert_eq!(preview_output_dimensions(64, 32), Ok((64, 32)));
        assert!(preview_output_dimensions(0, 320).is_err());
        assert!(preview_output_dimensions(320, 0).is_err());

        let identity = SourceIdentity {
            canonical_path: PathBuf::from("C:/preview/bounded.mp4"),
            file_len: 1,
            modified_nanos: 1,
        };
        assert!(matches!(
            preview_source_key(
                &identity.canonical_path,
                &identity,
                "source-bounded",
                Some(MediaLimits::default().max_dimension + 1),
                Some(1),
            ),
            Err(PipelineError::LimitExceeded {
                resource: "image-dimensions",
                ..
            })
        ));
    }

    #[test]
    fn video_extraction_uses_ascending_decode_order_but_retains_response_order() {
        let requested = normalize_preview_source_frame_ids(&[50, 1, 20, 1])
            .expect("fixture IDs must normalize");
        let extraction = ascending_video_preview_extraction(&requested)
            .expect("video extraction plan must be created");

        assert_eq!(requested, vec![50, 1, 20]);
        assert_eq!(extraction, vec![(0, 1), (19, 20), (49, 50)]);
    }

    #[test]
    fn cancelled_cache_hit_checkpoint_retains_the_resident_cache_key() {
        let identity = SourceIdentity {
            canonical_path: PathBuf::from("C:/preview/cancelled.gif"),
            file_len: 1,
            modified_nanos: 1,
        };
        let key = PreviewSourceKey {
            identity,
            source_revision: "source-cancelled".into(),
            variant: PreviewVariant::Native,
        };
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        assert!(cache.publish_complete(
            key.clone(),
            vec![CachedPreview {
                source_frame_id: 1,
                png_bytes: Arc::from([1_u8, 2, 3].as_slice()),
                width: 1,
                height: 1,
            }],
        ));
        let context = OperationContext::detached(MediaOperationKind::Preview.timeout());
        context.cancel();

        assert!(matches!(
            checkpoint_preview_cache_publication(&cache, &key, &context, None),
            Err(PipelineError::Cancelled)
        ));
        assert!(cache.contains_key(&key));
    }

    #[test]
    fn cancelled_new_publication_checkpoint_invalidates_the_cache_key() {
        let identity = SourceIdentity {
            canonical_path: PathBuf::from("C:/preview/cancelled.gif"),
            file_len: 1,
            modified_nanos: 1,
        };
        let key = PreviewSourceKey {
            identity,
            source_revision: "source-cancelled".into(),
            variant: PreviewVariant::Native,
        };
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        let publication = cache
            .get_or_try_build_complete(&key, &[1], || {
                Ok::<_, PipelineError>(vec![CachedPreview {
                    source_frame_id: 1,
                    png_bytes: Arc::from([1_u8, 2, 3].as_slice()),
                    width: 1,
                    height: 1,
                }])
            })
            .expect("new complete group must be published")
            .publication
            .expect("successful resident publication must return a receipt");
        let context = OperationContext::detached(MediaOperationKind::Preview.timeout());
        context.cancel();

        assert!(matches!(
            checkpoint_preview_cache_publication(&cache, &key, &context, Some(&publication)),
            Err(PipelineError::Cancelled)
        ));
        assert!(!cache.contains_key(&key));
    }

    #[test]
    fn changed_source_finalization_invalidates_the_published_cache_key() {
        let test_dir = TestDir::new("preview-cache-source-change");
        let source_path = test_dir.path.join("input.gif");
        fs::write(&source_path, b"source").expect("source fixture must be written");
        let identity = SourceIdentity::from_path(&source_path, MediaLimits::default())
            .expect("source identity must be created");
        let key = PreviewSourceKey {
            identity: identity.clone(),
            source_revision: identity.revision(),
            variant: PreviewVariant::Native,
        };
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        assert!(cache.publish_complete(
            key.clone(),
            vec![CachedPreview {
                source_frame_id: 1,
                png_bytes: Arc::from([1_u8, 2, 3].as_slice()),
                width: 1,
                height: 1,
            }],
        ));

        fs::write(&source_path, b"source changed to a different length")
            .expect("source fixture must be changed");
        let changed_len = fs::metadata(&source_path)
            .expect("changed source metadata must be readable")
            .len();
        assert_ne!(changed_len, identity.file_len);

        let context = OperationContext::detached(MediaOperationKind::Preview.timeout());
        let error = finalize_preview_cache_publication(&cache, &key, &identity, &context, None)
            .expect_err("changed source must reject preview-cache publication");

        assert_eq!(error, PipelineError::SourceChanged);
        assert!(!cache.contains_key(&key));
    }

    #[test]
    fn video_preview_filter_selects_then_scales_to_fixed_rgba_square_pixels() {
        let extraction = vec![(0, 1), (19, 20), (49, 50)];
        let filter = build_video_preview_filter(&extraction, 128, 64);

        assert_eq!(
            filter,
            "select='eq(n,0)+eq(n,19)+eq(n,49)',scale=128:64:flags=lanczos,format=rgba,setsar=1"
        );
    }

    #[test]
    fn requested_only_response_reconstructs_stable_order_and_base64_at_the_edge() {
        use crate::preview_cache::CachedPreview;

        let cached = vec![
            CachedPreview {
                source_frame_id: 1,
                png_bytes: Arc::from([1_u8, 2, 3].as_slice()),
                width: 128,
                height: 64,
            },
            CachedPreview {
                source_frame_id: 20,
                png_bytes: Arc::from([4_u8, 5, 6].as_slice()),
                width: 128,
                height: 64,
            },
            CachedPreview {
                source_frame_id: 50,
                png_bytes: Arc::from([7_u8, 8, 9].as_slice()),
                width: 128,
                height: 64,
            },
            CachedPreview {
                source_frame_id: 99,
                png_bytes: Arc::from([10_u8].as_slice()),
                width: 128,
                height: 64,
            },
        ];

        let response = cached_preview_items_for_requested_ids(&[50, 1, 20], &cached)
            .expect("requested cached previews must map to IPC items");

        assert_eq!(
            response
                .iter()
                .map(|item| item.source_frame_id)
                .collect::<Vec<_>>(),
            vec![50, 1, 20]
        );
        assert!(response
            .iter()
            .all(|item| item.data_url.starts_with("data:image/png;base64,")));
        assert!(!response.iter().any(|item| item.source_frame_id == 99));
    }

    #[test]
    fn preview_video_stream_requires_the_exact_selected_frame_count() {
        assert_eq!(validate_exact_selected_frame_stream(3, 3), Ok(()));
        assert!(matches!(
            validate_exact_selected_frame_stream(3, 2),
            Err(PipelineError::MalformedProcessOutput { .. })
        ));
        assert!(matches!(
            validate_exact_selected_frame_stream(3, 4),
            Err(PipelineError::MalformedProcessOutput { .. })
        ));
    }

    #[test]
    fn preview_backend_source_contract_is_streaming_shared_and_source_atomic() {
        let source = include_str!("lib.rs");
        let native = source_section(
            source,
            "fn build_native_preview_group(",
            "fn build_video_preview_filter(",
        );
        assert_eq!(native.matches("visit_native_animation_frames(").count(), 1);
        assert!(!native.contains("decode_native_animation_frames("));
        assert!(!native.contains("Vec<StickerFrame>"));
        assert!(native.contains("finalize_preview_cache_publication("));

        let video = source_section(
            source,
            "fn extract_video_preview_batch(",
            "fn cached_preview_items_for_requested_ids(",
        );
        assert_eq!(video.matches("stream_fixed_rgba_frames(").count(), 1);
        assert!(video.contains("validate_exact_selected_frame_stream("));
        assert!(video.contains("\"-map\""));
        assert!(video.contains("\"0:v:0\""));
        assert!(video.contains("\"-fps_mode\""));
        assert!(video.contains("\"passthrough\""));
        assert!(video.contains("finalize_preview_cache_publication("));

        let single = source_section(
            source,
            "fn extract_frame_preview_with_operation(",
            "fn extract_frame_preview_with_callbacks(",
        );
        let batch = source_section(
            source,
            "fn extract_frame_previews_with_operation(",
            "fn extract_frame_previews_with_callbacks(",
        );
        assert!(single.contains("load_cached_frame_previews("));
        assert!(batch.contains("load_cached_frame_previews("));

        let finalization = source_section(
            source,
            "fn finalize_preview_cache_publication(",
            "fn malformed_png(",
        );
        assert!(finalization.contains("ensure_source_unchanged("));
        assert!(finalization.contains("invalidate("));
        assert!(finalization.contains("fn finalize_preview_source_publication"));
        assert!(finalization.contains("context.finalize("));
    }

    fn tauri_command_source_block<'a>(source: &'a str, command: &str) -> &'a str {
        let marker = format!("fn {command}(");
        let start = source.find(&marker).expect("command source marker");
        let remainder = &source[start..];
        let body_start = remainder.find('{').expect("command body marker");
        let mut depth = 0usize;

        for (offset, character) in remainder[body_start..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth = depth.checked_sub(1).expect("balanced command body");
                    if depth == 0 {
                        let end = body_start + offset + character.len_utf8();
                        return &remainder[..end];
                    }
                }
                _ => {}
            }
        }

        panic!("unterminated command body")
    }

    fn rust_function_source_block<'a>(source: &'a str, marker: &str) -> &'a str {
        let start = source.find(marker).expect("function source marker");
        let remainder = &source[start..];
        let tail = &remainder[marker.len()..];
        let end = [
            "\nfn ",
            "\npub(crate) fn ",
            "\n#[tauri::command]",
            "\n#[cfg(test)]",
        ]
        .into_iter()
        .filter_map(|next_marker| tail.find(next_marker))
        .min()
        .map(|offset| marker.len() + offset)
        .unwrap_or(remainder.len());
        &remainder[..end]
    }

    fn assert_source_order(block: &str, markers: &[&str]) {
        let mut previous = 0;
        for marker in markers {
            let offset = block[previous..]
                .find(marker)
                .unwrap_or_else(|| panic!("missing ordered marker {marker}"))
                + previous;
            previous = offset + marker.len();
        }
    }

    #[test]
    fn reserved_preflight_observes_cancellation_before_returning_a_work_error() {
        let context = OperationContext::detached(MediaOperationKind::Inspect.timeout());

        let result = run_reserved_preflight(&context, || {
            assert!(context.cancel());
            Err::<(), PipelineError>(PipelineError::SourceChanged)
        });

        assert_eq!(result, Err(PipelineError::Cancelled));
    }

    #[test]
    fn managed_terminal_response_linearizes_late_cancellation() {
        let context = OperationContext::detached(MediaOperationKind::Inspect.timeout());

        assert_eq!(
            finalize_managed_response(&context, "domain-error-response"),
            Ok("domain-error-response")
        );
        assert!(context.is_completed());
        assert!(!context.cancel());
    }

    #[test]
    fn managed_terminal_response_observes_cancellation_before_completion() {
        let context = OperationContext::detached(MediaOperationKind::Inspect.timeout());
        assert!(context.cancel());

        assert_eq!(
            finalize_managed_response(&context, "stale-response"),
            Err(PipelineError::Cancelled)
        );
        assert!(!context.is_completed());
    }

    #[test]
    fn optimizer_search_domain_validation_rejects_invalid_selection_before_preflight() {
        let request = serde_json::from_str::<OptimizerSearchRequest>(
            r#"{
                "inputPath": "input.gif",
                "sourceRevision": "revision",
                "selectedFrames": [],
                "baseFrameCount": 1
            }"#,
        )
        .expect("bounded request fixture");

        let error = validate_optimizer_search_request_domain(&request, UiLocale::En)
            .expect_err("empty selection must fail domain validation");

        assert_eq!(error.reason, "no-frames-selected");
        assert_eq!(error.selected_duration_seconds, None);
    }

    #[test]
    fn optimizer_search_plan_origin_domain_error_preserves_plan_failure_metadata() {
        let request = serde_json::from_str::<OptimizerSearchRequest>(
            r#"{
                "inputPath": "input.gif",
                "sourceRevision": "revision",
                "selectedFrames": [1],
                "baseFrameCount": 1,
                "sourceDurationSeconds": 6.0,
                "inputWidth": 100,
                "inputHeight": 100,
                "cropRegion": { "x": 0.1, "y": 0.1, "width": 0.8, "height": 0.8 }
            }"#,
        )
        .expect("duration request fixture");
        let error = validate_optimizer_search_request_domain(&request, UiLocale::En)
            .expect_err("long duration must fail plan-domain validation");
        let expected_summary = error.message.clone();

        let response = optimizer_search_domain_error(UiLocale::En, Vec::new(), error);

        assert_eq!(response.stop_reason.as_deref(), Some("plan-invalid"));
        assert_eq!(response.summary, expected_summary);
        assert_eq!(
            response.warnings,
            vec![locale::crop_applied_before_scale_warning(UiLocale::En)]
        );
    }

    #[test]
    fn optimizer_search_domain_prevalidation_is_pure_and_bounded() {
        let source = include_str!("lib.rs");
        let validator =
            rust_function_source_block(source, "fn validate_optimizer_search_request_domain(");

        assert!(validator.contains("resolve_timeline_frames("));
        assert!(validator.contains("resolve_frame_selection("));
        assert!(validator.contains("resolve_crop_region("));
        assert!(!validator.contains("SourcePreflight"));
        assert!(!validator.contains("loader.prepare("));
        assert!(!validator.contains("build_candidate_"));
    }

    #[test]
    fn managed_commands_reserve_preflight_promote_and_run_one_managed_bundle() {
        let source = include_str!("lib.rs");
        let commands = [
            (
                "inspect_input_media",
                "MediaOperationKind::Inspect",
                Some("inspect_input_media_with_operation("),
            ),
            (
                "build_optimizer_plan",
                "MediaOperationKind::BuildPlan",
                None,
            ),
            (
                "estimate_static_output_size",
                "MediaOperationKind::StaticEstimate",
                Some("estimate_static_png_with_operation("),
            ),
            (
                "estimate_optimizer_candidates",
                "MediaOperationKind::OptimizerEstimate",
                Some("estimate_optimizer_candidates_with_loader("),
            ),
            (
                "probe_optimizer_candidate_size",
                "MediaOperationKind::OptimizerProbe",
                Some("probe_optimizer_candidate_with_loader("),
            ),
            (
                "convert_static_image_to_png",
                "MediaOperationKind::StaticConversion",
                Some("convert_static_image_to_png_with_operation("),
            ),
            (
                "run_optimizer_search",
                "MediaOperationKind::OptimizerSearch",
                Some("run_optimizer_search_with_operation("),
            ),
            (
                "extract_frame_preview",
                "MediaOperationKind::Preview",
                Some("extract_frame_preview_with_operation("),
            ),
            (
                "extract_frame_previews",
                "MediaOperationKind::Preview",
                Some("extract_frame_previews_with_operation("),
            ),
        ];

        for (command, kind, worker) in commands {
            let block = tauri_command_source_block(source, command);
            assert!(block.contains(kind), "{command} operation kind");
            if let Some(worker) = worker {
                assert_source_order(
                    block,
                    &[
                        ".reserve(&operation_id)",
                        "run_reserved_preflight(",
                        "SourcePreflight",
                        ".promote(",
                        "run_managed_blocking(managed",
                        "checkpointed_source_check(",
                        worker,
                    ],
                );
            } else {
                assert_source_order(
                    block,
                    &[
                        ".reserve(&operation_id)",
                        ".promote(",
                        "run_managed_blocking(managed",
                    ],
                );
                assert!(!block.contains("SourcePreflight"), "{command}");
            }
            assert_eq!(
                block.matches(".reserve(&operation_id)").count(),
                1,
                "{command}"
            );
            assert_eq!(block.matches(".promote(").count(), 1, "{command}");
            assert_eq!(
                block.matches("run_managed_blocking(managed").count(),
                1,
                "{command}"
            );
            assert_eq!(
                block.matches("finalize_managed_response(").count(),
                1,
                "{command}"
            );
        }

        let search = tauri_command_source_block(source, "run_optimizer_search");
        assert_source_order(
            search,
            &[
                "validate_optimizer_search_request_domain(",
                ".reserve(&operation_id)",
                "run_reserved_preflight(",
                ".promote(",
            ],
        );
    }

    #[test]
    fn command_business_logic_keeps_injectable_loader_checkpoint_and_progress_seams() {
        let source = include_str!("lib.rs");
        let search = source_section(
            source,
            "fn run_optimizer_search_with_loader(",
            "#[cfg(test)]",
        );

        assert!(search.contains("loader: &impl FrameSourceLoader"));
        assert!(search.contains("checkpoint: impl FnMut() -> Result<(), PipelineError>"));
        assert!(search.contains("progress: impl FnMut(ProgressStage, u32, Option<u32>)"));
        assert!(search.contains("loader.prepare("));
    }

    #[test]
    fn output_commands_recheck_worker_source_and_finalize_publication_in_one_order() {
        let source = include_str!("lib.rs");
        for (command, worker) in [
            (
                "convert_static_image_to_png",
                "convert_static_image_to_png_with_operation(",
            ),
            (
                "run_optimizer_search",
                "run_optimizer_search_with_operation(",
            ),
        ] {
            let block = tauri_command_source_block(source, command);
            assert_source_order(
                block,
                &[
                    "run_managed_blocking(managed",
                    "checkpointed_source_check(",
                    worker,
                ],
            );
        }

        let finalizer = rust_function_source_block(source, "fn finalize_source_publication(");
        assert_source_order(
            finalizer,
            &[
                "context.checkpoint()",
                "ensure_source_unchanged(",
                "context.checkpoint()",
                "context.finalize(",
            ],
        );

        let static_conversion = source_section(
            source,
            "fn convert_static_image_to_png_with_callbacks(",
            "fn static_conversion_pipeline_error(",
        );
        assert_source_order(
            static_conversion,
            &[
                "finalize_source_publication(",
                "ProgressStage::Finalizing",
                ".commit(",
            ],
        );

        let search = source_section(
            source,
            "fn run_optimizer_search_with_loader(",
            "#[cfg(test)]",
        );
        assert_source_order(
            search,
            &[
                "finalize_source_publication(",
                "ProgressStage::Finalizing",
                "publish_optimizer_selection(",
            ],
        );
    }

    #[test]
    fn cancelled_finalize_drops_pending_output_without_final_or_temporary_file() {
        for (kind, suffix) in [
            (MediaOperationKind::StaticConversion, "static-cancelled"),
            (MediaOperationKind::OptimizerSearch, "search-cancelled"),
        ] {
            let directory = tempfile::tempdir().expect("temp directory must be created");
            let source_path = directory.path().join("source.png");
            fs::write(&source_path, b"source").expect("source fixture must be written");
            let mut pending = PendingOutput::new(directory.path(), &source_path, suffix, "png")
                .expect("pending output must be created");
            pending
                .writer()
                .write_all(b"encoded")
                .expect("pending bytes must be written");
            let identity = SourceIdentity::from_path(&source_path, MediaLimits::default())
                .expect("source identity must be created");
            let context = OperationContext::detached(kind.timeout());
            let publisher_called = Arc::new(AtomicBool::new(false));
            let publisher_called_in_closure = Arc::clone(&publisher_called);
            assert!(context.cancel());

            let result = finalize_source_publication(&context, &identity, move || {
                publisher_called_in_closure.store(true, AtomicOrdering::SeqCst);
                pending.commit()
            });

            assert!(matches!(result, Err(PipelineError::Cancelled)));
            assert!(!publisher_called.load(AtomicOrdering::SeqCst));
            let paths = fs::read_dir(directory.path())
                .expect("output directory must be readable")
                .map(|entry| entry.expect("directory entry").path())
                .collect::<Vec<_>>();
            assert_eq!(paths, vec![source_path]);
        }
    }

    #[test]
    fn source_mutation_before_finalize_drops_pending_output_without_publish() {
        let directory = tempfile::tempdir().expect("temp directory must be created");
        let source_path = directory.path().join("source.png");
        fs::write(&source_path, b"source").expect("source fixture must be written");
        let identity = SourceIdentity::from_path(&source_path, MediaLimits::default())
            .expect("source identity must be created");
        let mut pending = PendingOutput::new(directory.path(), &source_path, "changed", "png")
            .expect("pending output must be created");
        pending
            .writer()
            .write_all(b"encoded")
            .expect("pending bytes must be written");
        fs::write(&source_path, b"source changed to another length")
            .expect("source fixture must be changed");
        let context = OperationContext::detached(MediaOperationKind::OptimizerSearch.timeout());
        let publisher_called = Arc::new(AtomicBool::new(false));
        let publisher_called_in_closure = Arc::clone(&publisher_called);

        let result = finalize_source_publication(&context, &identity, move || {
            publisher_called_in_closure.store(true, AtomicOrdering::SeqCst);
            pending.commit()
        });

        assert_eq!(result, Err(PipelineError::SourceChanged));
        assert!(!publisher_called.load(AtomicOrdering::SeqCst));
        let paths = fs::read_dir(directory.path())
            .expect("output directory must be readable")
            .map(|entry| entry.expect("directory entry").path())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec![source_path]);
    }

    #[test]
    fn optimizer_search_progress_stages_are_monotonic_and_finalizing_is_last() {
        let source = include_str!("lib.rs");
        let search = source_section(
            source,
            "fn run_optimizer_search_with_loader(",
            "#[cfg(test)]",
        );
        let estimating = search
            .find("progress(ProgressStage::Estimating")
            .expect("estimating progress");
        let decoding = search[estimating..]
            .find("progress(ProgressStage::Decoding")
            .expect("decoding progress")
            + estimating;
        let encoding = search[decoding..]
            .find("progress(ProgressStage::Encoding")
            .expect("encoding progress")
            + decoding;
        let finalizing = search[encoding..]
            .find("ProgressStage::Finalizing")
            .expect("finalizing progress")
            + encoding;

        assert!(estimating < decoding && decoding < encoding && encoding < finalizing);
        assert!(!search[finalizing + "ProgressStage::Finalizing".len()..]
            .contains("progress(ProgressStage::"));
    }

    #[test]
    fn preview_cancel_uses_matching_receipt_but_source_change_invalidates_full_key() {
        let test_dir = TestDir::new("preview-finalize-ownership");
        let source_path = test_dir.path.join("input.gif");
        fs::write(&source_path, b"source").expect("source fixture must be written");
        let identity = SourceIdentity::from_path(&source_path, MediaLimits::default())
            .expect("source identity must be created");
        let key = PreviewSourceKey {
            identity: identity.clone(),
            source_revision: identity.revision(),
            variant: PreviewVariant::Native,
        };
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        let stale_publication = cache
            .get_or_try_build_complete(&key, &[1], || {
                Ok::<_, PipelineError>(vec![CachedPreview {
                    source_frame_id: 1,
                    png_bytes: Arc::from([1_u8].as_slice()),
                    width: 1,
                    height: 1,
                }])
            })
            .expect("initial preview publication")
            .publication
            .expect("initial publication receipt");
        let current_publication = cache
            .merge_partial(
                key.clone(),
                vec![CachedPreview {
                    source_frame_id: 2,
                    png_bytes: Arc::from([2_u8].as_slice()),
                    width: 1,
                    height: 1,
                }],
            )
            .expect("replacement publication receipt");
        let context = OperationContext::detached(MediaOperationKind::Preview.timeout());
        assert!(context.cancel());

        assert!(matches!(
            checkpoint_preview_cache_publication(&cache, &key, &context, Some(&stale_publication),),
            Err(PipelineError::Cancelled)
        ));
        assert!(cache.contains_key(&key));

        fs::write(&source_path, b"source changed to a different length")
            .expect("source fixture must be changed");
        let source_change_context =
            OperationContext::detached(MediaOperationKind::Preview.timeout());
        assert_eq!(
            finalize_preview_cache_publication(
                &cache,
                &key,
                &identity,
                &source_change_context,
                Some(&current_publication),
            ),
            Err(PipelineError::SourceChanged)
        );
        assert!(!cache.contains_key(&key));
    }

    #[test]
    fn preview_source_change_invalidates_the_full_key_when_cancellation_wins() {
        let test_dir = TestDir::new("preview-source-change-cancel-race");
        let source_path = test_dir.path.join("input.gif");
        fs::write(&source_path, b"source").expect("source fixture must be written");
        let identity = SourceIdentity::from_path(&source_path, MediaLimits::default())
            .expect("source identity must be created");
        let key = PreviewSourceKey {
            identity: identity.clone(),
            source_revision: identity.revision(),
            variant: PreviewVariant::Native,
        };
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        let publication = cache
            .get_or_try_build_complete(&key, &[1], || {
                Ok::<_, PipelineError>(vec![CachedPreview {
                    source_frame_id: 1,
                    png_bytes: Arc::from([1_u8].as_slice()),
                    width: 1,
                    height: 1,
                }])
            })
            .expect("preview publication")
            .publication
            .expect("publication receipt");
        let context = OperationContext::detached(MediaOperationKind::Preview.timeout());

        let result = finalize_preview_cache_publication_with_check(
            &cache,
            &key,
            &context,
            Some(&publication),
            || {
                assert!(context.cancel());
                Err(PipelineError::SourceChanged)
            },
        );

        assert_eq!(result, Err(PipelineError::Cancelled));
        assert!(!cache.contains_key(&key));
    }

    #[test]
    fn preview_load_source_change_invalidates_the_full_key_before_returning_cancelled() {
        let test_dir = TestDir::new("preview-load-source-change-cancel-race");
        let source_path = test_dir.path.join("input.gif");
        fs::write(&source_path, b"source").expect("source fixture must be written");
        let identity = SourceIdentity::from_path(&source_path, MediaLimits::default())
            .expect("source identity must be created");
        let key = PreviewSourceKey {
            identity: identity.clone(),
            source_revision: identity.revision(),
            variant: PreviewVariant::Native,
        };
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        assert!(cache.publish_complete(
            key.clone(),
            vec![CachedPreview {
                source_frame_id: 1,
                png_bytes: Arc::from([1_u8].as_slice()),
                width: 1,
                height: 1,
            }],
        ));
        let context = OperationContext::detached(MediaOperationKind::Preview.timeout());
        assert!(context.cancel());

        let result = settle_preview_load_result::<()>(
            &cache,
            &key,
            &context,
            Err(PipelineError::SourceChanged),
        );

        assert_eq!(result, Err(PipelineError::Cancelled));
        assert!(!cache.contains_key(&key));
    }

    #[test]
    fn custom_png_metadata_parser_runs_inside_the_decoder_panic_boundary() {
        let source = include_str!("lib.rs");
        let parser = source_section(
            source,
            "fn read_png_metadata(",
            "fn read_png_animation_metadata(",
        );

        assert!(parser.contains("run_decoder_boundary(\"png\""));
        assert!(parser.contains("read_png_metadata_from_reader("));
    }
}

#[tauri::command]
async fn run_optimizer_search(
    request: OptimizerSearchRequest,
    operation_id: String,
    on_progress: Channel<OperationProgress>,
    pipeline_state: State<'_, PipelineState>,
) -> OptimizerSearchResponse {
    let locale = parse_ui_locale(request.locale.as_deref());
    let (_, fallback_fit_warning) = normalized_fit_mode(request.fit_mode.as_deref(), locale);
    if request.input_path.trim().is_empty()
        || request
            .source_revision
            .as_deref()
            .filter(|revision| !revision.trim().is_empty())
            .is_none()
    {
        return optimizer_search_pipeline_error(
            locale,
            fallback_fit_warning.into_iter().collect(),
            PipelineError::InvalidRequestWithoutReason,
        );
    }
    if let Err(error) = validate_optimizer_search_request_domain(&request, locale) {
        return optimizer_search_domain_error(
            locale,
            fallback_fit_warning.into_iter().collect(),
            error,
        );
    }
    let kind = MediaOperationKind::OptimizerSearch;
    let state = pipeline_state.inner().clone();
    let progress = ValidatedProgressSink::new(kind, ChannelProgressSink::new(on_progress));
    let reservation = match state.reserve(&operation_id) {
        Ok(reservation) => reservation,
        Err(error) => {
            return optimizer_search_pipeline_error(
                locale,
                fallback_fit_warning.into_iter().collect(),
                error,
            )
        }
    };
    let preflight_context = reservation.context().clone();
    let preflight_input_path = request.input_path.clone();
    let preflight_source_revision = request.source_revision.clone();
    let preflight = match run_blocking_task(move || {
        run_reserved_preflight(&preflight_context, || {
            SourcePreflight::validate_expected(
                Path::new(&preflight_input_path),
                preflight_source_revision.as_deref(),
                MediaLimits::default(),
            )
        })
    })
    .await
    {
        Ok(Ok(preflight)) => preflight,
        Ok(Err(error)) => {
            return optimizer_search_pipeline_error(
                locale,
                fallback_fit_warning.into_iter().collect(),
                error,
            )
        }
        Err(message) => {
            return optimizer_search_pipeline_error(
                locale,
                fallback_fit_warning.into_iter().collect(),
                PipelineError::Io {
                    operation: "join source preflight",
                    message,
                },
            )
        }
    };
    let managed = match reservation.promote(kind, &progress).await {
        Ok(managed) => managed,
        Err(error) => {
            return optimizer_search_pipeline_error(
                locale,
                fallback_fit_warning.into_iter().collect(),
                error,
            )
        }
    };
    let context = managed.context().clone();
    let worker_fallback_fit_warning = fallback_fit_warning.clone();

    match run_managed_blocking(managed, move || {
        let response = if let Err(error) =
            checkpointed_source_check(&context, preflight.identity(), MediaLimits::default())
        {
            optimizer_search_pipeline_error(
                locale,
                worker_fallback_fit_warning.clone().into_iter().collect(),
                error,
            )
        } else {
            run_optimizer_search_with_operation(request, locale, &preflight, &context, &progress)
        };
        match finalize_managed_response(&context, response) {
            Ok(response) => response,
            Err(error) => optimizer_search_pipeline_error(
                locale,
                worker_fallback_fit_warning.into_iter().collect(),
                error,
            ),
        }
    })
    .await
    {
        Ok(result) => result,
        Err(message) => optimizer_search_pipeline_error(
            locale,
            fallback_fit_warning.into_iter().collect(),
            PipelineError::Io {
                operation: "join media worker",
                message,
            },
        ),
    }
}

#[tauri::command]
async fn extract_frame_preview(
    input_path: String,
    source_revision: Option<String>,
    source_frame_id: u32,
    source_width: Option<u32>,
    source_height: Option<u32>,
    locale: Option<String>,
    operation_id: String,
    on_progress: Channel<OperationProgress>,
    pipeline_state: State<'_, PipelineState>,
) -> FramePreviewResponse {
    let locale = parse_ui_locale(locale.as_deref());
    if input_path.trim().is_empty()
        || source_revision
            .as_deref()
            .filter(|revision| !revision.trim().is_empty())
            .is_none()
    {
        return frame_preview_pipeline_error(&PipelineError::InvalidRequestWithoutReason, locale);
    }
    if let Err(error) = normalize_preview_source_frame_ids(&[source_frame_id]) {
        return frame_preview_pipeline_error(&error, locale);
    }
    let kind = MediaOperationKind::Preview;
    let state = pipeline_state.inner().clone();
    let preview_cache = state.preview_cache();
    let progress = ValidatedProgressSink::new(kind, ChannelProgressSink::new(on_progress));
    let reservation = match state.reserve(&operation_id) {
        Ok(reservation) => reservation,
        Err(error) => return frame_preview_pipeline_error(&error, locale),
    };
    let preflight_context = reservation.context().clone();
    let preflight_input_path = input_path.clone();
    let preflight_source_revision = source_revision.clone();
    let (preflight, key) = match run_blocking_task(move || {
        run_reserved_preflight(&preflight_context, || {
            let preflight = SourcePreflight::validate_expected(
                Path::new(&preflight_input_path),
                preflight_source_revision.as_deref(),
                MediaLimits::default(),
            )?;
            let revision = preflight.identity().revision();
            let key = preview_source_key(
                preflight.canonical_path(),
                preflight.identity(),
                &revision,
                source_width,
                source_height,
            )?;
            Ok((preflight, key))
        })
    })
    .await
    {
        Ok(Ok(preflight)) => preflight,
        Ok(Err(error)) => return frame_preview_pipeline_error(&error, locale),
        Err(message) => {
            return frame_preview_pipeline_error(
                &PipelineError::Io {
                    operation: "join source preflight",
                    message,
                },
                locale,
            )
        }
    };
    let managed = match reservation.promote(kind, &progress).await {
        Ok(managed) => managed,
        Err(error) => return frame_preview_pipeline_error(&error, locale),
    };
    let context = managed.context().clone();

    match run_managed_blocking(managed, move || {
        let response = if let Err(error) =
            checkpointed_source_check(&context, preflight.identity(), MediaLimits::default())
        {
            frame_preview_pipeline_error(&error, locale)
        } else {
            extract_frame_preview_with_operation(
                &preflight,
                source_frame_id,
                locale,
                &key,
                &preview_cache,
                &context,
                &progress,
            )
        };
        match finalize_managed_response(&context, response) {
            Ok(response) => response,
            Err(error) => frame_preview_pipeline_error(&error, locale),
        }
    })
    .await
    {
        Ok(response) => response,
        Err(message) => frame_preview_pipeline_error(
            &PipelineError::Io {
                operation: "join media worker",
                message,
            },
            locale,
        ),
    }
}

#[tauri::command]
async fn extract_frame_previews(
    input_path: String,
    source_revision: Option<String>,
    source_frame_ids: BoundedFrameIds,
    source_width: Option<u32>,
    source_height: Option<u32>,
    locale: Option<String>,
    operation_id: String,
    on_progress: Channel<OperationProgress>,
    pipeline_state: State<'_, PipelineState>,
) -> FramePreviewsResponse {
    let locale = parse_ui_locale(locale.as_deref());
    if input_path.trim().is_empty()
        || source_revision
            .as_deref()
            .filter(|revision| !revision.trim().is_empty())
            .is_none()
    {
        return frame_previews_pipeline_error(&PipelineError::InvalidRequestWithoutReason, locale);
    }
    let source_frame_ids = match normalize_preview_source_frame_ids(&source_frame_ids.0) {
        Ok(source_frame_ids) => source_frame_ids,
        Err(error) => return frame_previews_pipeline_error(&error, locale),
    };
    let kind = MediaOperationKind::Preview;
    let state = pipeline_state.inner().clone();
    let preview_cache = state.preview_cache();
    let progress = ValidatedProgressSink::new(kind, ChannelProgressSink::new(on_progress));
    let reservation = match state.reserve(&operation_id) {
        Ok(reservation) => reservation,
        Err(error) => return frame_previews_pipeline_error(&error, locale),
    };
    let preflight_context = reservation.context().clone();
    let preflight_input_path = input_path.clone();
    let preflight_source_revision = source_revision.clone();
    let (preflight, key) = match run_blocking_task(move || {
        run_reserved_preflight(&preflight_context, || {
            let preflight = SourcePreflight::validate_expected(
                Path::new(&preflight_input_path),
                preflight_source_revision.as_deref(),
                MediaLimits::default(),
            )?;
            let revision = preflight.identity().revision();
            let key = preview_source_key(
                preflight.canonical_path(),
                preflight.identity(),
                &revision,
                source_width,
                source_height,
            )?;
            Ok((preflight, key))
        })
    })
    .await
    {
        Ok(Ok(preflight)) => preflight,
        Ok(Err(error)) => return frame_previews_pipeline_error(&error, locale),
        Err(message) => {
            return frame_previews_pipeline_error(
                &PipelineError::Io {
                    operation: "join source preflight",
                    message,
                },
                locale,
            )
        }
    };
    let managed = match reservation.promote(kind, &progress).await {
        Ok(managed) => managed,
        Err(error) => return frame_previews_pipeline_error(&error, locale),
    };
    let context = managed.context().clone();

    match run_managed_blocking(managed, move || {
        let response = if let Err(error) =
            checkpointed_source_check(&context, preflight.identity(), MediaLimits::default())
        {
            frame_previews_pipeline_error(&error, locale)
        } else {
            extract_frame_previews_with_operation(
                &preflight,
                &source_frame_ids,
                locale,
                &key,
                &preview_cache,
                &context,
                &progress,
            )
        };
        match finalize_managed_response(&context, response) {
            Ok(response) => response,
            Err(error) => frame_previews_pipeline_error(&error, locale),
        }
    })
    .await
    {
        Ok(response) => response,
        Err(message) => frame_previews_pipeline_error(
            &PipelineError::Io {
                operation: "join media worker",
                message,
            },
            locale,
        ),
    }
}

#[tauri::command]
fn cancel_media_operation(operation_id: String, pipeline_state: State<'_, PipelineState>) -> bool {
    pipeline_state.cancel(&operation_id)
}

#[tauri::command]
fn open_folder_path(path: Option<String>, locale: Option<String>) -> Result<(), String> {
    #[allow(unused_variables)]
    let requested_locale = parse_ui_locale(locale.as_deref());

    let Some(path) = path else {
        return Ok(());
    };

    let requested_path = PathBuf::from(path);
    let target = if requested_path.is_file() {
        requested_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(requested_path)
    } else {
        requested_path
    };

    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(target)
            .spawn()
            .map_err(|error| error.to_string())?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(target)
            .spawn()
            .map_err(|error| error.to_string())?;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map_err(|error| error.to_string())?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err(locale::unsupported_platform_error(requested_locale))
}

pub fn run() {
    tauri::Builder::default()
        .manage(PipelineState::new())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            check_media_tools,
            inspect_input_media,
            build_optimizer_plan,
            estimate_static_output_size,
            estimate_optimizer_candidates,
            probe_optimizer_candidate_size,
            run_optimizer_search,
            convert_static_image_to_png,
            extract_frame_preview,
            extract_frame_previews,
            cancel_media_operation,
            open_folder_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running StickerFit");
}
