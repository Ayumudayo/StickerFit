mod locale;
mod media_error;
mod media_limits;
mod operation;
mod output_file;
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
use std::sync::OnceLock;
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

use crate::locale::{parse_ui_locale, UiLocale};
use crate::media_error::PipelineError;
use crate::media_limits::{
    checked_rgba_bytes, image_decode_limits, validate_file_metadata, MediaLimits, SourceIdentity,
};
use crate::operation::{
    publish_progress, run_managed_blocking, ChannelProgressSink, MediaOperationKind,
    OperationContext, OperationProgress, PipelineState, ProgressSink, ProgressStage,
};
use crate::output_file::PendingOutput;
use crate::process_runner::{
    run_captured, stream_fixed_rgba_frames, CapturedProcess, ProcessLimits,
};

const CANONICAL_FIT_MODE: &str = "contain";
const MAX_MEDIA_FRAME_COUNT: usize = 300;
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
    error_code: Option<String>,
    reason_code: Option<String>,
    error_message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FramePreviewResponse {
    ok: bool,
    data_url: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    error_code: Option<String>,
    reason_code: Option<String>,
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
    error_code: Option<String>,
    reason_code: Option<String>,
    error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CropRegion {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OptimizerPlanRequest {
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditedTimelineFrame {
    source_frame_id: u32,
    duration_us: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CandidatePreview {
    id: String,
    rank: usize,
    duration_seconds: f64,
    fps: u32,
    content_scale: f64,
    preset: String,
    fit_mode: String,
    score: f64,
    source_similarity_score: f64,
    summary: String,
    #[serde(skip_serializing)]
    frame_sample_step: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OptimizerPlanResponse {
    ok: bool,
    fit_mode: String,
    selected_duration_seconds: Option<f64>,
    recommended_max_duration_seconds: f64,
    search_budget: usize,
    warnings: Vec<String>,
    candidates: Vec<CandidatePreview>,
    error_code: Option<String>,
    reason_code: Option<String>,
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
    error_code: Option<String>,
    reason_code: Option<String>,
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
    error_code: Option<String>,
    reason_code: Option<String>,
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
    error_code: Option<String>,
    reason_code: Option<String>,
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

#[derive(Debug, Clone)]
struct ResolvedTimelineFrame {
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

fn candidate_estimated_size_factor(candidate: &CandidatePreview) -> f64 {
    let frame_factor = 1.0 / candidate.frame_sample_step.max(1) as f64;
    let scale_factor = candidate.content_scale.clamp(0.01, 1.0).powi(2);
    frame_factor * scale_factor * preset_size_factor(&candidate.preset)
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
            .collect::<BTreeSet<_>>()
            .into_iter()
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
            let selected_frames = match base_frame_count.filter(|count| *count > 0) {
                Some(base_count) if selected_frame_count == base_count as usize => None,
                _ => Some(frames),
            };

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

fn sampled_frame_indexes(
    selected_frames: Option<&[u32]>,
    base_frame_count: Option<u32>,
    sample_step: u32,
) -> Option<Vec<u32>> {
    if sample_step <= 1 {
        return selected_frames.map(|frames| frames.to_vec());
    }

    let frame_indexes = selected_frames
        .map(|frames| frames.to_vec())
        .or_else(|| base_frame_count.map(|count| (0..count).collect::<Vec<_>>()))?;

    let step = sample_step as usize;
    Some(frame_indexes.into_iter().step_by(step).collect())
}

fn natural_selection_duration_seconds(selected_frame_count: usize, source_fps: f64) -> f64 {
    selected_frame_count as f64 / source_fps.max(1.0)
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
                summary,
                frame_sample_step: 1,
            });
        }
    }

    select_ranked_candidate_subset(candidates, search_budget)
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

fn encode_native_png_data_url(pixels: &RgbaImage) -> Result<String, PipelineError> {
    let bytes = encode_native_png_bytes(pixels)?;
    Ok(format!(
        "data:image/png;base64,{}",
        general_purpose::STANDARD.encode(bytes)
    ))
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
    input_path: &str,
    source_frame_id: u32,
    locale: UiLocale,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> FramePreviewResponse {
    extract_frame_preview_with_callbacks(
        input_path,
        source_frame_id,
        locale,
        || context.checkpoint(),
        |stage, completed, total| {
            publish_progress(progress, context, stage, completed, total);
        },
    )
}

fn extract_frame_preview_with_callbacks(
    input_path: &str,
    source_frame_id: u32,
    locale: UiLocale,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
    mut progress: impl FnMut(ProgressStage, u32, Option<u32>),
) -> FramePreviewResponse {
    let Some(extension) = lowercase_source_extension(input_path) else {
        return frame_preview_pipeline_error(
            &PipelineError::InvalidRequest {
                reason: "unsupported-source-format",
            },
            locale,
        );
    };

    if source_frame_id == 0 {
        return frame_preview_pipeline_error(
            &PipelineError::InvalidRequest {
                reason: "invalid-frame-selection",
            },
            locale,
        );
    }

    if !matches!(extension.as_str(), "gif" | "apng" | "png") {
        return frame_preview_pipeline_error(
            &PipelineError::InvalidRequest {
                reason: "unsupported-frame-preview",
            },
            locale,
        );
    }

    progress(ProgressStage::Decoding, 0, Some(1));
    let frame = match decode_native_animation_frame(
        input_path,
        source_frame_id,
        MediaLimits::default(),
        || checkpoint(),
    ) {
        Ok(frame) => frame,
        Err(error) => return frame_preview_pipeline_error(&error, locale),
    };

    progress(ProgressStage::Encoding, 0, Some(1));
    if let Err(error) = checkpoint() {
        return frame_preview_pipeline_error(&error, locale);
    }
    let data_url = match encode_native_png_data_url(&frame.pixels) {
        Ok(data_url) => data_url,
        Err(error) => return frame_preview_pipeline_error(&error, locale),
    };
    if let Err(error) = checkpoint() {
        return frame_preview_pipeline_error(&error, locale);
    }
    progress(ProgressStage::Encoding, 1, Some(1));
    progress(ProgressStage::Finalizing, 1, Some(1));
    if let Err(error) = checkpoint() {
        return frame_preview_pipeline_error(&error, locale);
    }

    FramePreviewResponse {
        ok: true,
        data_url: Some(data_url),
        width: Some(frame.pixels.width()),
        height: Some(frame.pixels.height()),
        reason_code: None,
        error_code: None,
        error_message: None,
    }
}

fn frame_preview_pipeline_error(error: &PipelineError, locale: UiLocale) -> FramePreviewResponse {
    FramePreviewResponse {
        ok: false,
        data_url: None,
        width: None,
        height: None,
        reason_code: error.reason_code().map(str::to_string),
        error_code: Some(error.code().into()),
        error_message: Some(pipeline_error_diagnostic(error, locale)),
    }
}

fn frame_previews_pipeline_error(error: &PipelineError, locale: UiLocale) -> FramePreviewsResponse {
    FramePreviewsResponse {
        ok: false,
        previews: Vec::new(),
        reason_code: error.reason_code().map(str::to_string),
        error_code: Some(error.code().into()),
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
    input_path: &str,
    source_frame_ids: &[u32],
    locale: UiLocale,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> FramePreviewsResponse {
    extract_frame_previews_with_callbacks(
        input_path,
        source_frame_ids,
        locale,
        || context.checkpoint(),
        |stage, completed, total| {
            publish_progress(progress, context, stage, completed, total);
        },
    )
}

fn extract_frame_previews_with_callbacks(
    input_path: &str,
    source_frame_ids: &[u32],
    locale: UiLocale,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
    mut progress: impl FnMut(ProgressStage, u32, Option<u32>),
) -> FramePreviewsResponse {
    if source_frame_ids.len() > MAX_MEDIA_FRAME_COUNT {
        return frame_previews_pipeline_error(
            &PipelineError::LimitExceeded {
                resource: "frame-count",
                limit: MAX_MEDIA_FRAME_COUNT as u64,
                actual: u64::try_from(source_frame_ids.len()).unwrap_or(u64::MAX),
            },
            locale,
        );
    }
    if source_frame_ids
        .iter()
        .any(|source_frame_id| *source_frame_id == 0)
    {
        return frame_previews_pipeline_error(
            &PipelineError::InvalidRequest {
                reason: "invalid-frame-selection",
            },
            locale,
        );
    }

    let Some(extension) = lowercase_source_extension(input_path) else {
        return frame_previews_pipeline_error(
            &PipelineError::InvalidRequest {
                reason: "unsupported-source-format",
            },
            locale,
        );
    };

    if !matches!(extension.as_str(), "gif" | "apng" | "png") {
        return frame_previews_pipeline_error(
            &PipelineError::InvalidRequest {
                reason: "unsupported-frame-preview",
            },
            locale,
        );
    }

    let requested_frame_ids = source_frame_ids.iter().copied().collect::<BTreeSet<_>>();
    if requested_frame_ids.is_empty() {
        return frame_previews_pipeline_error(
            &PipelineError::InvalidRequest {
                reason: "invalid-frame-selection",
            },
            locale,
        );
    }

    let total = u32::try_from(requested_frame_ids.len()).unwrap_or(u32::MAX);
    progress(ProgressStage::Decoding, 0, Some(total));
    let frames =
        match decode_native_animation_frames(input_path, MediaLimits::default(), || checkpoint()) {
            Ok(frames) => frames,
            Err(error) => return frame_previews_pipeline_error(&error, locale),
        };

    progress(ProgressStage::Encoding, 0, Some(total));
    let mut previews = Vec::with_capacity(requested_frame_ids.len());
    for source_frame_id in requested_frame_ids {
        if let Err(error) = checkpoint() {
            return frame_previews_pipeline_error(&error, locale);
        }
        let Some(frame) = frames.get((source_frame_id - 1) as usize) else {
            return frame_previews_pipeline_error(
                &PipelineError::InvalidRequest {
                    reason: "invalid-frame-selection",
                },
                locale,
            );
        };

        let data_url = match encode_native_png_data_url(&frame.pixels) {
            Ok(data_url) => data_url,
            Err(error) => return frame_previews_pipeline_error(&error, locale),
        };

        previews.push(FramePreviewItem {
            source_frame_id,
            data_url,
            width: frame.pixels.width(),
            height: frame.pixels.height(),
        });
        progress(
            ProgressStage::Encoding,
            u32::try_from(previews.len()).unwrap_or(u32::MAX),
            Some(total),
        );
    }

    progress(ProgressStage::Finalizing, total, Some(total));
    if let Err(error) = checkpoint() {
        return frame_previews_pipeline_error(&error, locale);
    }

    FramePreviewsResponse {
        ok: true,
        previews,
        reason_code: None,
        error_code: None,
        error_message: None,
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
    let output_path = commit_output_after_source_validation(pending_output, Some(expected_source))?
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
    checkpoint()?;
    if frames.is_empty() {
        return Err(PipelineError::InvalidRequest {
            reason: "no-frames-selected",
        });
    }

    let width = frames[0].pixels.width();
    let height = frames[0].pixels.height();
    if frames
        .iter()
        .any(|frame| frame.pixels.width() != width || frame.pixels.height() != height)
    {
        return Err(PipelineError::InvalidRequest {
            reason: "invalid-frame-selection",
        });
    }

    let durations_us = frames
        .iter()
        .map(|frame| frame.duration_us)
        .collect::<Vec<_>>();
    let frame_delays = quantize_apng_delays(&durations_us)
        .map_err(|reason| PipelineError::InvalidRequest { reason })?;

    let mut encoder = NativePngEncoder::new(writer, width, height);
    encoder.set_color(PngColorType::Rgba);
    encoder.set_depth(PngBitDepth::Eight);
    encoder
        .set_animated(frames.len() as u32, 0)
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
    encoder.validate_sequence(true);

    let mut png_writer = encoder
        .write_header()
        .map_err(|error| pipeline_io_error("write APNG header", error))?;
    checkpoint()?;
    png_writer
        .write_image_data(frames[0].pixels.as_raw())
        .map_err(|error| pipeline_io_error("write APNG frame", error))?;
    checkpoint()?;

    let mut previous_frame = frames[0].pixels.clone();
    for (frame, &(delay_num, delay_den)) in frames[1..].iter().zip(&frame_delays[1..]) {
        checkpoint()?;
        let region = changed_frame_region(&previous_frame, &frame.pixels);
        png_writer
            .reset_frame_position()
            .map_err(|error| pipeline_io_error("configure APNG frame", error))?;
        png_writer
            .set_frame_dimension(region.width, region.height)
            .map_err(|error| pipeline_io_error("configure APNG frame", error))?;
        png_writer
            .set_frame_position(region.x, region.y)
            .map_err(|error| pipeline_io_error("configure APNG frame", error))?;
        png_writer
            .set_frame_delay(delay_num, delay_den)
            .map_err(|error| pipeline_io_error("configure APNG frame", error))?;
        png_writer
            .set_blend_op(PngBlendOp::Source)
            .map_err(|error| pipeline_io_error("configure APNG frame", error))?;
        png_writer
            .set_dispose_op(PngDisposeOp::None)
            .map_err(|error| pipeline_io_error("configure APNG frame", error))?;
        let region_pixels = frame_region_pixels(&frame.pixels, region);
        png_writer
            .write_image_data(&region_pixels)
            .map_err(|error| pipeline_io_error("write APNG frame", error))?;
        checkpoint()?;
        previous_frame = frame.pixels.clone();
    }

    checkpoint()?;
    png_writer
        .finish()
        .map_err(|error| pipeline_io_error("finish APNG output", error))
}

fn build_native_selected_animation_frames(
    source_frames: &[StickerFrame],
    selected_frames: Option<&[u32]>,
    candidate_fps: u32,
) -> Result<Vec<StickerFrame>, String> {
    let frame_indexes: Vec<u32> = selected_frames
        .map(|frames| frames.to_vec())
        .unwrap_or_else(|| (0..source_frames.len() as u32).collect());

    if frame_indexes.is_empty() {
        return Err("no frames available for selection".into());
    }

    let duration_us = frame_duration_us_for_fps(candidate_fps);
    frame_indexes
        .into_iter()
        .map(|index| {
            source_frames
                .get(index as usize)
                .cloned()
                .map(|frame| StickerFrame {
                    pixels: frame.pixels,
                    duration_us,
                })
                .ok_or_else(|| "selected frame index is out of range".to_string())
        })
        .collect()
}

fn build_native_timeline_frames(
    source_frames: &[StickerFrame],
    timeline_frames: &[ResolvedTimelineFrame],
) -> Result<Vec<StickerFrame>, String> {
    timeline_frames
        .iter()
        .map(|frame| {
            source_frames
                .get(frame.source_frame_index as usize)
                .cloned()
                .map(|source| StickerFrame {
                    pixels: source.pixels,
                    duration_us: frame.duration_us,
                })
                .ok_or_else(|| "timeline frame index is out of range".to_string())
        })
        .collect()
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

fn decode_video_frames_with_ffmpeg(
    input_path: &str,
    filter_graph: &str,
    frame_width: u32,
    frame_height: u32,
    locale: UiLocale,
    context: Option<&OperationContext>,
) -> Result<(Vec<RgbaImage>, ToolResolution), PipelineError> {
    let limits = MediaLimits::default();
    let frame_size = checked_rgba_bytes(frame_width, frame_height, limits)?;
    preflight_animation_decoded_bytes(frame_size, 1, limits)?;
    let resolution = resolve_tool("ffmpeg", locale)
        .map_err(|_| PipelineError::ToolMissing { tool: "ffmpeg" })?;
    let args = [
        OsString::from("-v"),
        OsString::from("error"),
        OsString::from("-i"),
        OsString::from(input_path),
        OsString::from("-vf"),
        OsString::from(filter_graph),
        OsString::from("-pix_fmt"),
        OsString::from("rgba"),
        OsString::from("-f"),
        OsString::from("rawvideo"),
        OsString::from("-an"),
        OsString::from("-"),
    ];
    let detached = context
        .is_none()
        .then(|| OperationContext::detached(RAW_DECODE_PROCESS_TIMEOUT));
    let context = context.unwrap_or_else(|| detached.as_ref().expect("detached decode context"));
    let mut frames = Vec::new();
    let summary = stream_fixed_rgba_frames(
        &resolution.command,
        &args,
        frame_size,
        limits.max_frame_count as usize,
        ProcessLimits {
            timeout: RAW_DECODE_PROCESS_TIMEOUT,
            max_stdout_bytes: usize::try_from(limits.max_total_decoded_bytes).unwrap_or(usize::MAX),
            max_stderr_bytes: PROCESS_CAPTURE_LIMIT_BYTES,
        },
        context,
        |frame| {
            frames.push(rgba_frame_from_bytes(
                frame_width,
                frame_height,
                frame.to_vec(),
            )?);
            Ok(())
        },
    )?;
    validate_resampled_frame_stream(summary.frame_count)?;

    Ok((frames, resolution))
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
            let pixels = rgba_frame_from_bytes(frame_width, frame_height, frame.to_vec())?;
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
    build_candidate_ladder_with_checkpoint(
        selected_frame_count,
        source_fps,
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

fn build_candidate_ladder_with_checkpoint(
    selected_frame_count: usize,
    source_fps: f64,
    input_width: Option<u32>,
    input_height: Option<u32>,
    preset_strategy: &str,
    optimizer_goal: &str,
    search_budget: usize,
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
    let source_duration_seconds =
        natural_selection_duration_seconds(selected_frame_count, source_fps);
    let frame_sample_steps = frame_sample_steps_for_goal(selected_frame_count, optimizer_goal);

    for frame_sample_step in frame_sample_steps {
        checkpoint()?;
        let encoded_frame_count = sampled_frame_count(selected_frame_count, frame_sample_step);
        let fps_ladder: Vec<u32> = [30, 27, 24, 21, 18, 15, 12, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1]
            .into_iter()
            .filter(|fps| {
                candidate_duration_seconds(encoded_frame_count, *fps)
                    <= duration_us_to_seconds(DISCORD_MAX_DURATION_US)
            })
            .collect();

        for fps in fps_ladder {
            checkpoint()?;
            let duration_seconds = candidate_duration_seconds(encoded_frame_count, fps);
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
                        summary,
                        frame_sample_step,
                    });
                }
            }
        }
    }

    checkpoint()?;
    Ok(select_ranked_candidate_subset(candidates, search_budget))
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
                reason_code: Some("invalid-crop".into()),
                error_code: Some("invalid-request".into()),
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
                                reason_code: Some(error.into()),
                                error_code: Some("invalid-request".into()),
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
                    reason_code: Some("no-frames-selected".into()),
                    error_code: Some("invalid-request".into()),
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
                    reason_code: Some(reason.into()),
                    error_code: Some("invalid-request".into()),
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
                    reason_code: Some(error.into()),
                    error_code: Some("invalid-request".into()),
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
                reason_code: Some("duration-too-long".into()),
                error_code: Some("invalid-request".into()),
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
                    reason_code: Some("no-frames-selected".into()),
                    error_code: Some("invalid-request".into()),
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
                    reason_code: Some("invalid-frame-selection".into()),
                    error_code: Some("invalid-request".into()),
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
                    reason_code: Some(error.into()),
                    error_code: Some("invalid-request".into()),
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
    let natural_duration_seconds =
        natural_selection_duration_seconds(frame_selection.selected_frame_count, source_fps);
    let shortest_frame_count =
        frame_sample_steps_for_goal(frame_selection.selected_frame_count, optimizer_goal)
            .into_iter()
            .map(|step| sampled_frame_count(frame_selection.selected_frame_count, step))
            .min()
            .unwrap_or(frame_selection.selected_frame_count);
    let shortest_duration_seconds = candidate_duration_seconds(shortest_frame_count, 30);

    if shortest_duration_seconds > duration_us_to_seconds(DISCORD_MAX_DURATION_US) {
        return OptimizerPlanResponse {
            ok: false,
            fit_mode: fit_mode.into(),
            selected_duration_seconds: Some(shortest_duration_seconds),
            recommended_max_duration_seconds: duration_us_to_seconds(RECOMMENDED_MAX_DURATION_US),
            search_budget,
            warnings,
            candidates: Vec::new(),
            reason_code: Some("duration-too-long".into()),
            error_code: Some("invalid-request".into()),
            error_message: Some(locale::selected_duration_limit_error(locale)),
        };
    }

    if natural_duration_seconds > duration_us_to_seconds(RECOMMENDED_MAX_DURATION_US) {
        warnings.push(locale::recommended_duration_warning(locale));
    }

    let candidates = match build_candidate_ladder_with_checkpoint(
        frame_selection.selected_frame_count,
        source_fps,
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
    if response.error_code.is_some() {
        return response;
    }
    publish_progress(progress, context, ProgressStage::Finalizing, 1, Some(1));
    match context.checkpoint() {
        Ok(()) => response,
        Err(error) => optimizer_plan_pipeline_error(locale, response.warnings, &error),
    }
}

fn encode_candidate_from_native_animation_internal(
    input_path: &str,
    output_directory: Option<&str>,
    locale: UiLocale,
    crop_region: Option<&CropRegion>,
    _input_width: Option<u32>,
    _input_height: Option<u32>,
    candidate: &CandidatePreview,
    timeline_frames: &[ResolvedTimelineFrame],
    expected_source: Option<&SourceIdentity>,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> Result<EncodeResult, PipelineError> {
    let output_directory =
        resolve_output_directory(output_directory, input_path, locale).map_err(|_| {
            PipelineError::InvalidRequest {
                reason: "invalid-output-directory",
            }
        })?;
    let source_frames =
        decode_native_animation_frames(input_path, MediaLimits::default(), || checkpoint())?;
    let first_frame = source_frames
        .first()
        .ok_or_else(|| PipelineError::MalformedInput {
            format: "animation",
            reason: "native animation did not contain frames".into(),
        })?;
    let resolved_crop_region = resolve_crop_region(
        crop_region,
        Some(first_frame.pixels.width()),
        Some(first_frame.pixels.height()),
        locale,
    )
    .map_err(|_| PipelineError::InvalidRequest {
        reason: "invalid-crop",
    })?;
    let frames = build_native_timeline_frames(&source_frames, timeline_frames)
        .map_err(|_| PipelineError::InvalidRequest {
            reason: "invalid-frame-selection",
        })?
        .into_iter()
        .map(|frame| {
            checkpoint()?;
            let transformed = StickerFrame {
                pixels: transform_frame_for_candidate(
                    &frame.pixels,
                    candidate.content_scale,
                    resolved_crop_region,
                ),
                duration_us: frame.duration_us,
            };
            checkpoint()?;
            Ok(transformed)
        })
        .collect::<Result<Vec<_>, PipelineError>>()?;
    if let Some(expected_source) = expected_source {
        ensure_source_unchanged(expected_source, MediaLimits::default())?;
    }
    let started = Instant::now();
    let mut pending_output = PendingOutput::new(
        &output_directory,
        Path::new(input_path),
        &candidate.id,
        "png",
    )?;
    write_native_apng_with_checkpoint(pending_output.writer(), &frames, &candidate.preset, || {
        checkpoint()
    })?;
    let size_bytes = pending_output_size(&mut pending_output)?;

    Ok(EncodeResult {
        pending_output,
        size_bytes,
        elapsed_ms: started.elapsed().as_millis() as u64,
        tool_source: "native".into(),
        tool_command: None,
        tool_detail: Some(locale::native_apng_encode_detail(locale)),
    })
}

fn encode_candidate_from_video_timeline_internal(
    input_path: &str,
    output_directory: Option<&str>,
    locale: UiLocale,
    crop_region: Option<&CropRegion>,
    input_width: Option<u32>,
    input_height: Option<u32>,
    candidate: &CandidatePreview,
    timeline_frames: &[ResolvedTimelineFrame],
    expected_source: Option<&SourceIdentity>,
    context: Option<&OperationContext>,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> Result<EncodeResult, PipelineError> {
    let output_directory =
        resolve_output_directory(output_directory, input_path, locale).map_err(|_| {
            PipelineError::InvalidRequest {
                reason: "invalid-output-directory",
            }
        })?;
    let resolved_crop_region = resolve_crop_region(crop_region, input_width, input_height, locale)
        .map_err(|_| PipelineError::InvalidRequest {
            reason: "invalid-crop",
        })?;
    let source_width =
        input_width
            .filter(|width| *width > 0)
            .ok_or(PipelineError::InvalidRequest {
                reason: "invalid-crop",
            })?;
    let source_height =
        input_height
            .filter(|height| *height > 0)
            .ok_or(PipelineError::InvalidRequest {
                reason: "invalid-crop",
            })?;
    let unique_frame_indexes = timeline_frames
        .iter()
        .map(|frame| frame.source_frame_index)
        .collect::<BTreeSet<_>>();
    checkpoint()?;
    let decoded = extract_video_source_frames_rgba(
        input_path,
        &unique_frame_indexes,
        source_width,
        source_height,
        locale,
        context,
    );
    checkpoint()?;
    let (source_frames, resolution) = decoded?;

    let frames = timeline_frames
        .iter()
        .map(|frame| {
            checkpoint()?;
            source_frames
                .get(&frame.source_frame_index)
                .map(|pixels| StickerFrame {
                    pixels: transform_frame_for_candidate(
                        pixels,
                        candidate.content_scale,
                        resolved_crop_region,
                    ),
                    duration_us: frame.duration_us,
                })
                .ok_or(PipelineError::InvalidRequest {
                    reason: "invalid-frame-selection",
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(expected_source) = expected_source {
        ensure_source_unchanged(expected_source, MediaLimits::default())?;
    }
    let started = Instant::now();
    let mut pending_output = PendingOutput::new(
        &output_directory,
        Path::new(input_path),
        &candidate.id,
        "png",
    )?;
    write_native_apng_with_checkpoint(pending_output.writer(), &frames, &candidate.preset, || {
        checkpoint()
    })?;
    let size_bytes = pending_output_size(&mut pending_output)?;

    Ok(EncodeResult {
        pending_output,
        size_bytes,
        elapsed_ms: started.elapsed().as_millis() as u64,
        tool_source: resolution.source.into(),
        tool_command: Some(resolution.command_display),
        tool_detail: resolution.fallback_reason,
    })
}

fn encode_candidate_with_ffmpeg_frames_internal(
    input_path: &str,
    output_directory: Option<&str>,
    locale: UiLocale,
    crop_region: Option<&CropRegion>,
    input_width: Option<u32>,
    input_height: Option<u32>,
    candidate: &CandidatePreview,
    selected_frames: Option<&[u32]>,
    expected_source: Option<&SourceIdentity>,
    context: Option<&OperationContext>,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> Result<EncodeResult, PipelineError> {
    let output_directory =
        resolve_output_directory(output_directory, input_path, locale).map_err(|_| {
            PipelineError::InvalidRequest {
                reason: "invalid-output-directory",
            }
        })?;
    let resolved_crop_region = resolve_crop_region(crop_region, input_width, input_height, locale)
        .map_err(|_| PipelineError::InvalidRequest {
            reason: "invalid-crop",
        })?;
    let filter_graph = build_filter_graph(
        candidate.fps,
        candidate.content_scale,
        input_width,
        input_height,
        resolved_crop_region,
        selected_frames,
    );
    let effective_width = resolved_crop_region.map(|crop| crop.width).or(input_width);
    let effective_height = resolved_crop_region
        .map(|crop| crop.height)
        .or(input_height);
    let (frame_width, frame_height) =
        scaled_output_dimensions(effective_width, effective_height, candidate.content_scale);
    checkpoint()?;
    let decoded = decode_video_frames_with_ffmpeg(
        input_path,
        &filter_graph,
        frame_width,
        frame_height,
        locale,
        context,
    );
    checkpoint()?;
    let (pixels, resolution) = decoded?;
    let frames = pixels
        .into_iter()
        .map(|pixels| {
            checkpoint()?;
            Ok(StickerFrame {
                pixels,
                duration_us: frame_duration_us_for_fps(candidate.fps),
            })
        })
        .collect::<Result<Vec<_>, PipelineError>>()?;
    if let Some(expected_source) = expected_source {
        ensure_source_unchanged(expected_source, MediaLimits::default())?;
    }
    let started = Instant::now();
    let mut pending_output = PendingOutput::new(
        &output_directory,
        Path::new(input_path),
        &candidate.id,
        "png",
    )?;
    write_native_apng_with_checkpoint(pending_output.writer(), &frames, &candidate.preset, || {
        checkpoint()
    })?;
    let size_bytes = pending_output_size(&mut pending_output)?;

    Ok(EncodeResult {
        pending_output,
        size_bytes,
        elapsed_ms: started.elapsed().as_millis() as u64,
        tool_source: resolution.source.into(),
        tool_command: Some(resolution.command_display),
        tool_detail: resolution.fallback_reason,
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
    encode_candidate_with_checkpoint(
        input_path,
        output_directory,
        locale,
        crop_region,
        input_width,
        input_height,
        candidate,
        selected_frames,
        timeline_frames,
        expected_source,
        None,
        &mut || Ok(()),
    )
}

fn encode_candidate_with_checkpoint(
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
    context: Option<&OperationContext>,
    checkpoint: &mut impl FnMut() -> Result<(), PipelineError>,
) -> Result<EncodeResult, PipelineError> {
    let extension = lowercase_source_extension(input_path).unwrap_or_default();

    if let Some(timeline_frames) = timeline_frames {
        return match extension.as_str() {
            "gif" | "apng" => encode_candidate_from_native_animation_internal(
                input_path,
                output_directory,
                locale,
                crop_region,
                input_width,
                input_height,
                candidate,
                timeline_frames,
                expected_source,
                checkpoint,
            ),
            _ => encode_candidate_from_video_timeline_internal(
                input_path,
                output_directory,
                locale,
                crop_region,
                input_width,
                input_height,
                candidate,
                timeline_frames,
                expected_source,
                context,
                checkpoint,
            ),
        };
    }

    if matches!(extension.as_str(), "gif" | "apng") {
        let output_directory = resolve_output_directory(output_directory, input_path, locale)
            .map_err(|_| PipelineError::InvalidRequest {
                reason: "invalid-output-directory",
            })?;
        let source_frames =
            decode_native_animation_frames(input_path, MediaLimits::default(), || checkpoint())?;
        let first_frame = source_frames
            .first()
            .ok_or_else(|| PipelineError::MalformedInput {
                format: "animation",
                reason: "native animation did not contain frames".into(),
            })?;
        let resolved_crop_region = resolve_crop_region(
            crop_region,
            Some(first_frame.pixels.width()),
            Some(first_frame.pixels.height()),
            locale,
        )
        .map_err(|_| PipelineError::InvalidRequest {
            reason: "invalid-crop",
        })?;
        let frames =
            build_native_selected_animation_frames(&source_frames, selected_frames, candidate.fps)
                .map_err(|_| PipelineError::InvalidRequest {
                    reason: "invalid-frame-selection",
                })?
                .into_iter()
                .map(|frame| {
                    checkpoint()?;
                    let transformed = StickerFrame {
                        pixels: transform_frame_for_candidate(
                            &frame.pixels,
                            candidate.content_scale,
                            resolved_crop_region,
                        ),
                        duration_us: frame.duration_us,
                    };
                    checkpoint()?;
                    Ok(transformed)
                })
                .collect::<Result<Vec<_>, PipelineError>>()?;
        if let Some(expected_source) = expected_source {
            ensure_source_unchanged(expected_source, MediaLimits::default())?;
        }
        let started = Instant::now();
        let mut pending_output = PendingOutput::new(
            &output_directory,
            Path::new(input_path),
            &candidate.id,
            "png",
        )?;
        write_native_apng_with_checkpoint(
            pending_output.writer(),
            &frames,
            &candidate.preset,
            || checkpoint(),
        )?;
        let size_bytes = pending_output_size(&mut pending_output)?;

        return Ok(EncodeResult {
            pending_output,
            size_bytes,
            elapsed_ms: started.elapsed().as_millis() as u64,
            tool_source: "native".into(),
            tool_command: None,
            tool_detail: Some(locale::native_apng_encode_detail(locale)),
        });
    }

    encode_candidate_with_ffmpeg_frames_internal(
        input_path,
        output_directory,
        locale,
        crop_region,
        input_width,
        input_height,
        candidate,
        selected_frames,
        expected_source,
        context,
        checkpoint,
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
        || Ok(()),
        |_, _, _| {},
    )
}

fn convert_static_image_to_png_with_operation(
    input_path: &str,
    output_directory: Option<&str>,
    crop_region: Option<&CropRegion>,
    locale: UiLocale,
    expected_source: Option<&SourceIdentity>,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> StaticImageConversionResult {
    convert_static_image_to_png_with_callbacks(
        input_path,
        output_directory,
        crop_region,
        locale,
        expected_source,
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
    context: Option<&OperationContext>,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
    mut progress: impl FnMut(ProgressStage, u32, Option<u32>),
) -> StaticImageConversionResult {
    progress(ProgressStage::Inspecting, 0, Some(1));
    let inspection = inspect_input_media_with_callbacks(
        input_path,
        locale,
        context,
        || checkpoint(),
        |_, _, _| {},
    );

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
            reason_code: Some("unsupported-source-format".into()),
            error_code: Some("invalid-request".into()),
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
                    reason_code: Some("invalid-crop".into()),
                    error_code: Some("invalid-request".into()),
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
                    reason_code: error.reason_code().map(str::to_string),
                    error_code: Some(error.code().into()),
                    error_message: Some(pipeline_error_diagnostic(&error, locale)),
                }
            }
        };
    if let Some(expected_source) = expected_source {
        if let Err(error) = checkpoint() {
            return static_conversion_pipeline_error(&error, locale);
        }
        if let Err(error) = ensure_source_unchanged(expected_source, MediaLimits::default()) {
            return StaticImageConversionResult {
                ok: false,
                output_path: None,
                size_bytes: None,
                elapsed_ms: None,
                tool_source: Some("native".into()),
                tool_command: None,
                tool_detail: Some(locale::native_image_detail(locale)),
                warnings: Vec::new(),
                reason_code: error.reason_code().map(str::to_string),
                error_code: Some(error.code().into()),
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
        progress(ProgressStage::Finalizing, 1, Some(1));
        checkpoint()?;
        let output_path = commit_output_after_source_validation(pending_output, expected_source)?;
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
        reason_code: error.reason_code().map(str::to_string),
        error_code: Some(error.code().into()),
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
        reason_code: error.reason_code().map(str::to_string),
        error_code: Some(error.code().into()),
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
        reason_code: reason_code.map(str::to_string),
        error_code: Some(error_code.into()),
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

fn normalize_backend_error_code(error_code: &str) -> (&'static str, Option<&'static str>) {
    match error_code {
        "cancelled"
        | "timed-out"
        | "operation-conflict"
        | "invalid-request"
        | "source-changed"
        | "media-input-too-large"
        | "media-dimensions-too-large"
        | "media-frame-limit"
        | "decoded-byte-limit"
        | "png-chunk-limit"
        | "malformed-media"
        | "malformed-process-output"
        | "tool-missing"
        | "process-failed"
        | "output-conflict"
        | "internal-task-failed" => (canonical_error_code(error_code), None),
        "no-frames-selected"
        | "invalid-frame-selection"
        | "invalid-frame-duration"
        | "duration-too-long"
        | "invalid-crop"
        | "invalid-output-directory"
        | "unsupported-source-format"
        | "unsupported-frame-preview"
        | "frame-preview-decode-failed"
        | "frame-preview-encode-failed"
        | "decode-failed"
        | "encode-failed"
        | "missing-output"
        | "plan-invalid"
        | "invoke-failed" => ("invalid-request", canonical_reason_code(error_code)),
        "inspect-failed" => ("malformed-media", Some("decode-failed")),
        "tool-unavailable" => ("tool-missing", None),
        _ => ("internal-task-failed", None),
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

fn canonical_error_code(error_code: &str) -> &'static str {
    match error_code {
        "cancelled" => "cancelled",
        "timed-out" => "timed-out",
        "operation-conflict" => "operation-conflict",
        "invalid-request" => "invalid-request",
        "source-changed" => "source-changed",
        "media-input-too-large" => "media-input-too-large",
        "media-dimensions-too-large" => "media-dimensions-too-large",
        "media-frame-limit" => "media-frame-limit",
        "decoded-byte-limit" => "decoded-byte-limit",
        "png-chunk-limit" => "png-chunk-limit",
        "malformed-media" => "malformed-media",
        "malformed-process-output" => "malformed-process-output",
        "tool-missing" => "tool-missing",
        "process-failed" => "process-failed",
        "output-conflict" => "output-conflict",
        _ => "internal-task-failed",
    }
}

fn canonical_reason_code(reason_code: &str) -> Option<&'static str> {
    match reason_code {
        "no-frames-selected" => Some("no-frames-selected"),
        "invalid-frame-selection" => Some("invalid-frame-selection"),
        "invalid-frame-duration" => Some("invalid-frame-duration"),
        "duration-too-long" => Some("duration-too-long"),
        "invalid-crop" => Some("invalid-crop"),
        "invalid-output-directory" => Some("invalid-output-directory"),
        "unsupported-source-format" => Some("unsupported-source-format"),
        "unsupported-frame-preview" => Some("unsupported-frame-preview"),
        "frame-preview-decode-failed" => Some("frame-preview-decode-failed"),
        "frame-preview-encode-failed" => Some("frame-preview-encode-failed"),
        "decode-failed" => Some("decode-failed"),
        "encode-failed" => Some("encode-failed"),
        "missing-output" => Some("missing-output"),
        "plan-invalid" => Some("plan-invalid"),
        "invoke-failed" => Some("invoke-failed"),
        _ => None,
    }
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
    locale: UiLocale,
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> MediaInspection {
    inspect_input_media_with_callbacks(
        input_path,
        locale,
        Some(context),
        || context.checkpoint(),
        |stage, completed, total| {
            publish_progress(progress, context, stage, completed, total);
        },
    )
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
    let identity = match SourceIdentity::from_path(Path::new(input_path), limits) {
        Ok(identity) => identity,
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
    let canonical_path = identity.canonical_path.to_string_lossy().into_owned();
    let inspection = inspect_input_media_canonical_with_checkpoint(
        &canonical_path,
        locale,
        context,
        &mut checkpoint,
    );
    let inspection = apply_inspection_source_revision(&identity, input_path, inspection);
    let inspection = enforce_desktop_inspection_frame_limit(inspection, locale);
    let tool_source = inspection.tool_source.clone();
    let tool_command = inspection.tool_command.clone();
    let tool_detail = inspection.tool_detail.clone();
    let fallback_reason_code = inspection.fallback_reason_code.clone();
    progress(ProgressStage::Finalizing, 1, Some(1));
    if let Err(error) = checkpoint() {
        return inspection_pipeline_error_with_provenance(input_path, &inspection, &error, locale);
    }
    finalize_source_checked(&identity, inspection, limits, |error| {
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
    let input_path_for_error = input_path.clone();
    let state = pipeline_state.inner().clone();
    let progress = ChannelProgressSink::new(on_progress);
    let registered = match state.register(&operation_id, MediaOperationKind::Inspect.timeout()) {
        Ok(registered) => registered,
        Err(error) => return inspection_pipeline_error(&input_path, &error, locale),
    };
    let context = registered.context().clone();
    publish_progress(&progress, &context, ProgressStage::Queued, 0, None);
    let permit = match state.acquire_decode(&context).await {
        Ok(permit) => permit,
        Err(error) => return inspection_pipeline_error(&input_path, &error, locale),
    };
    publish_progress(&progress, &context, ProgressStage::Inspecting, 0, Some(1));

    match run_managed_blocking(registered, permit, move || {
        inspect_input_media_with_operation(&input_path, locale, &context, &progress)
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
    let state = pipeline_state.inner().clone();
    let progress = ChannelProgressSink::new(on_progress);
    let registered = match state.register(&operation_id, MediaOperationKind::BuildPlan.timeout()) {
        Ok(registered) => registered,
        Err(error) => {
            return optimizer_plan_pipeline_error(
                locale,
                fallback_fit_warning.into_iter().collect(),
                &error,
            )
        }
    };
    let context = registered.context().clone();
    publish_progress(&progress, &context, ProgressStage::Queued, 0, None);

    match run_managed_blocking(registered, (), move || {
        prepare_optimizer_plan_with_operation(&request, locale, &context, &progress)
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
async fn convert_static_image_to_png(
    request: StaticImageConversionRequest,
    operation_id: String,
    on_progress: Channel<OperationProgress>,
    pipeline_state: State<'_, PipelineState>,
) -> StaticImageConversionResult {
    let locale = parse_ui_locale(request.locale.as_deref());
    let state = pipeline_state.inner().clone();
    let progress = ChannelProgressSink::new(on_progress);
    let registered = match state.register(
        &operation_id,
        MediaOperationKind::StaticConversion.timeout(),
    ) {
        Ok(registered) => registered,
        Err(error) => return static_conversion_pipeline_error(&error, locale),
    };
    let context = registered.context().clone();
    publish_progress(&progress, &context, ProgressStage::Queued, 0, None);
    let permits = match state.acquire_output_then_decode(&context).await {
        Ok(permits) => permits,
        Err(error) => return static_conversion_pipeline_error(&error, locale),
    };

    match run_managed_blocking(registered, permits, move || {
        let identity = match validate_source_revision(
            Path::new(&request.input_path),
            request.source_revision.as_deref(),
            MediaLimits::default(),
        ) {
            Ok(identity) => identity,
            Err(error) => return static_conversion_pipeline_error(&error, locale),
        };
        let canonical_path = identity.canonical_path.to_string_lossy().into_owned();
        let response = convert_static_image_to_png_with_operation(
            &canonical_path,
            request.output_directory.as_deref(),
            request.crop_region.as_ref(),
            locale,
            Some(&identity),
            &context,
            &progress,
        );
        finalize_static_conversion_source_unless_published(&identity, response, locale)
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
    OptimizerSearchResponse {
        ok: false,
        fit_mode: CANONICAL_FIT_MODE.into(),
        selected_duration_seconds: None,
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
        reason_code: error.reason_code().map(str::to_string),
        error_code: Some(error.code().into()),
        error_message: Some(pipeline_error_diagnostic(&error, locale)),
    }
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
    context: &OperationContext,
    progress: &impl ProgressSink,
) -> OptimizerSearchResponse {
    run_optimizer_search_with_callbacks(
        request,
        locale,
        Some(context),
        || context.checkpoint(),
        |stage, completed, total| {
            publish_progress(progress, context, stage, completed, total);
        },
    )
}

fn run_optimizer_search_with_callbacks(
    mut request: OptimizerSearchRequest,
    locale: UiLocale,
    context: Option<&OperationContext>,
    mut checkpoint: impl FnMut() -> Result<(), PipelineError>,
    mut progress: impl FnMut(ProgressStage, u32, Option<u32>),
) -> OptimizerSearchResponse {
    let (_, legacy_fit_warning) = normalized_fit_mode(request.fit_mode.as_deref(), locale);
    let legacy_fit_warnings = legacy_fit_warning.into_iter().collect::<Vec<_>>();
    if let Err(error) = checkpoint() {
        return optimizer_search_pipeline_error(locale, legacy_fit_warnings, error);
    }
    let source_identity = match validate_source_revision(
        Path::new(&request.input_path),
        request.source_revision.as_deref(),
        MediaLimits::default(),
    ) {
        Ok(identity) => identity,
        Err(error) => {
            return optimizer_search_pipeline_error(locale, legacy_fit_warnings, error);
        }
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
                                reason_code: Some(error.into()),
                                error_code: Some("invalid-request".into()),
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
                    reason_code: Some("no-frames-selected".into()),
                    error_code: Some("invalid-request".into()),
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
                    reason_code: Some(reason.into()),
                    error_code: Some("invalid-request".into()),
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
                                        reason_code: Some(error.into()),
                                        error_code: Some("invalid-request".into()),
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
                            reason_code: Some("no-frames-selected".into()),
                            error_code: Some("invalid-request".into()),
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
                            reason_code: Some("invalid-frame-selection".into()),
                            error_code: Some("invalid-request".into()),
                            error_message: Some(locale::invalid_frame_selection_error(locale)),
                        };
                    }
                },
            )
        } else {
            None
        };

        progress(ProgressStage::Estimating, 0, None);
        let plan = prepare_optimizer_plan_with_checkpoint(
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
                selected_duration_seconds: None,
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

        let mut attempts = Vec::new();
        let mut best_within_limit_output: Option<PendingSelectedEncodeOutput> = None;
        let mut smallest_oversize_output: Option<PendingSelectedEncodeOutput> = None;
        let mut stopped_after_best_within_limit = false;
        let total_candidates = u32::try_from(plan.candidates.len()).unwrap_or(u32::MAX);
        let mut completed_candidates = 0_u32;
        progress(ProgressStage::Encoding, 0, Some(total_candidates));
        for candidate in &plan.candidates {
            if let Err(error) = checkpoint() {
                return optimizer_search_pipeline_error(locale, plan.warnings.clone(), error);
            }
            if let Err(error) = ensure_source_unchanged(&source_identity, MediaLimits::default()) {
                return optimizer_search_pipeline_error(locale, plan.warnings.clone(), error);
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

            let sampled_legacy_frames = if resolved_timeline_frames.is_none() {
                sampled_frame_indexes(
                    legacy_selected_frames
                        .as_ref()
                        .and_then(|frames| frames.as_deref()),
                    request.base_frame_count,
                    candidate.frame_sample_step,
                )
            } else {
                None
            };

            let encode_result = if let Some(timeline_frames) = resolved_timeline_frames.as_deref() {
                encode_candidate_with_checkpoint(
                    &request.input_path,
                    request.output_directory.as_deref(),
                    locale,
                    request.crop_region.as_ref(),
                    request.input_width,
                    request.input_height,
                    candidate,
                    None,
                    Some(timeline_frames),
                    Some(&source_identity),
                    context,
                    &mut checkpoint,
                )
            } else {
                encode_candidate_with_checkpoint(
                    &request.input_path,
                    request.output_directory.as_deref(),
                    locale,
                    request.crop_region.as_ref(),
                    request.input_width,
                    request.input_height,
                    candidate,
                    sampled_legacy_frames.as_deref().or_else(|| {
                        legacy_selected_frames
                            .as_ref()
                            .and_then(|frames| frames.as_deref())
                    }),
                    None,
                    Some(&source_identity),
                    context,
                    &mut checkpoint,
                )
            };

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
                        duration_seconds: candidate.duration_seconds,
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
                            duration_seconds: candidate.duration_seconds,
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
                    return optimizer_search_pipeline_error(locale, plan.warnings.clone(), error);
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
        progress(
            ProgressStage::Finalizing,
            completed_candidates,
            Some(total_candidates),
        );
        if let Err(error) = checkpoint() {
            return optimizer_search_pipeline_error(locale, plan.warnings.clone(), error);
        }
        let published_output = match publish_optimizer_selection(
            &source_identity,
            best_within_limit_output,
            smallest_oversize_output,
            &mut attempts,
        ) {
            Ok(output) => output,
            Err(error) => {
                return optimizer_search_pipeline_error(locale, plan.warnings.clone(), error)
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
    finalize_optimizer_search_source_unless_published(&source_identity, response, locale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_error::PipelineError;
    use crate::media_limits::MediaLimits;
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

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

        let resampled = source_section(
            lib_source,
            "fn decode_video_frames_with_ffmpeg(",
            "fn extract_video_source_frames_rgba(",
        );
        assert!(resampled.contains("stream_fixed_rgba_frames("));
        assert!(resampled.contains("validate_resampled_frame_stream("));
        assert!(resampled.contains("context.unwrap_or"));
        let selected = source_section(
            lib_source,
            "fn extract_video_source_frames_rgba(",
            "fn validate_resampled_frame_stream(",
        );
        assert!(selected.contains("stream_fixed_rgba_frames("));
        assert!(selected.contains("validate_exact_selected_frame_stream("));
        assert!(selected.contains("context.unwrap_or"));

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
            "fn encode_candidate_with_checkpoint(",
        );
        let encode_propagation = source_section(
            lib_source,
            "fn encode_candidate_with_checkpoint(",
            "fn convert_static_image_to_png_internal(",
        );
        assert!(encode_direct.contains("None,"));
        assert!(encode_propagation.contains("context,"));

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
        assert!(runner_source.contains("sync_channel::<FrameEvent>(2)"));
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

        let published =
            publish_optimizer_selection(&source_identity, None, Some(oversize), &mut attempts)
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
            publish_optimizer_selection(&source_identity, Some(winner), None, &mut attempts)
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
    fn normalize_selected_frame_indexes_converts_ui_ids_to_zero_based_indexes() {
        assert_eq!(
            normalize_selected_frame_indexes(Some(&vec![7, 1, 3, 7])),
            Some(vec![0, 2, 6])
        );
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
        assert_eq!(preview.width, inspection.width);
        assert_eq!(preview.height, inspection.height);
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
    fn prepare_optimizer_plan_samples_long_frame_selection_to_fit_five_seconds() {
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

        assert!(response.ok);
        assert!(response
            .candidates
            .iter()
            .any(|candidate| candidate.frame_sample_step > 1
                && candidate.duration_seconds <= duration_us_to_seconds(DISCORD_MAX_DURATION_US)));
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
    fn motion_goal_keeps_frames_when_duration_can_fit() {
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

        assert!(response.ok);
        assert!(response
            .candidates
            .iter()
            .all(|candidate| candidate.frame_sample_step == 1));
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
    let state = pipeline_state.inner().clone();
    let progress = ChannelProgressSink::new(on_progress);
    let registered =
        match state.register(&operation_id, MediaOperationKind::OptimizerSearch.timeout()) {
            Ok(registered) => registered,
            Err(error) => {
                return optimizer_search_pipeline_error(
                    locale,
                    fallback_fit_warning.into_iter().collect(),
                    error,
                )
            }
        };
    let context = registered.context().clone();
    publish_progress(&progress, &context, ProgressStage::Queued, 0, None);
    let permits = match state.acquire_output_then_decode(&context).await {
        Ok(permits) => permits,
        Err(error) => {
            return optimizer_search_pipeline_error(
                locale,
                fallback_fit_warning.into_iter().collect(),
                error,
            )
        }
    };

    match run_managed_blocking(registered, permits, move || {
        run_optimizer_search_with_operation(request, locale, &context, &progress)
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
    locale: Option<String>,
    operation_id: String,
    on_progress: Channel<OperationProgress>,
    pipeline_state: State<'_, PipelineState>,
) -> FramePreviewResponse {
    let locale = parse_ui_locale(locale.as_deref());
    let state = pipeline_state.inner().clone();
    let progress = ChannelProgressSink::new(on_progress);
    let registered = match state.register(&operation_id, MediaOperationKind::Preview.timeout()) {
        Ok(registered) => registered,
        Err(error) => return frame_preview_pipeline_error(&error, locale),
    };
    let context = registered.context().clone();
    publish_progress(&progress, &context, ProgressStage::Queued, 0, None);
    let permit = match state.acquire_decode(&context).await {
        Ok(permit) => permit,
        Err(error) => return frame_preview_pipeline_error(&error, locale),
    };

    match run_managed_blocking(registered, permit, move || {
        let identity = match validate_source_revision(
            Path::new(&input_path),
            source_revision.as_deref(),
            MediaLimits::default(),
        ) {
            Ok(identity) => identity,
            Err(error) => return frame_preview_pipeline_error(&error, locale),
        };
        let canonical_path = identity.canonical_path.to_string_lossy().into_owned();
        let response = extract_frame_preview_with_operation(
            &canonical_path,
            source_frame_id,
            locale,
            &context,
            &progress,
        );
        finalize_frame_preview_source(&identity, response, locale)
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
    locale: Option<String>,
    operation_id: String,
    on_progress: Channel<OperationProgress>,
    pipeline_state: State<'_, PipelineState>,
) -> FramePreviewsResponse {
    let locale = parse_ui_locale(locale.as_deref());
    let source_frame_ids = source_frame_ids.0;
    let state = pipeline_state.inner().clone();
    let progress = ChannelProgressSink::new(on_progress);
    let registered = match state.register(&operation_id, MediaOperationKind::Preview.timeout()) {
        Ok(registered) => registered,
        Err(error) => return frame_previews_pipeline_error(&error, locale),
    };
    let context = registered.context().clone();
    publish_progress(&progress, &context, ProgressStage::Queued, 0, None);
    let permit = match state.acquire_decode(&context).await {
        Ok(permit) => permit,
        Err(error) => return frame_previews_pipeline_error(&error, locale),
    };

    match run_managed_blocking(registered, permit, move || {
        let identity = match validate_source_revision(
            Path::new(&input_path),
            source_revision.as_deref(),
            MediaLimits::default(),
        ) {
            Ok(identity) => identity,
            Err(error) => return frame_previews_pipeline_error(&error, locale),
        };
        let canonical_path = identity.canonical_path.to_string_lossy().into_owned();
        let response = extract_frame_previews_with_operation(
            &canonical_path,
            &source_frame_ids,
            locale,
            &context,
            &progress,
        );
        finalize_frame_previews_source(&identity, response, locale)
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
