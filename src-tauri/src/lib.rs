mod locale;

use base64::{engine::general_purpose, Engine as _};
use image::codecs::gif::GifDecoder as ImageGifDecoder;
use image::codecs::png::PngDecoder as ImagePngDecoder;
use image::imageops::{self, FilterType};
use image::{AnimationDecoder, GenericImage, ImageReader, Rgba, RgbaImage};
#[cfg(test)]
use image::{DynamicImage, ImageFormat};
use png::{
    BitDepth as PngBitDepth, BlendOp as PngBlendOp, ColorType as PngColorType,
    DeflateCompression as PngDeflateCompression, DisposeOp as PngDisposeOp,
    Encoder as NativePngEncoder, Filter as PngFilter,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom};
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
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
    tool_source: Option<String>,
    tool_command: Option<String>,
    tool_detail: Option<String>,
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
    is_static_image: bool,
    can_convert_to_png: bool,
    error_code: Option<String>,
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
    fit_mode: String,
    preset_strategy: Option<String>,
    optimizer_goal: Option<String>,
    quality_frame_drop_interval: Option<u32>,
    search_depth: Option<String>,
    crop_region: Option<CropRegion>,
    selected_frames: Option<Vec<u32>>,
    base_frame_count: Option<u32>,
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
    error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StaticImageConversionRequest {
    input_path: String,
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
    error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OptimizerSearchRequest {
    input_path: String,
    output_directory: Option<String>,
    locale: Option<String>,
    source_duration_seconds: Option<f64>,
    input_width: Option<u32>,
    input_height: Option<u32>,
    avg_fps: Option<f64>,
    fit_mode: String,
    preset_strategy: Option<String>,
    optimizer_goal: Option<String>,
    quality_frame_drop_interval: Option<u32>,
    search_depth: Option<String>,
    crop_region: Option<CropRegion>,
    selected_frames: Option<Vec<u32>>,
    base_frame_count: Option<u32>,
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
    duration_seconds: f64,
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
    system_error: String,
}

struct EncodeResult {
    output_path: PathBuf,
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
    output_path: String,
}

#[derive(Clone)]
struct StickerFrame {
    pixels: RgbaImage,
    duration_seconds: f64,
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
}

const RECOMMENDED_MAX_DURATION_SECONDS: f64 = 3.0;
const DISCORD_MAX_DURATION_SECONDS: f64 = 5.0;
const DISCORD_MAX_STICKER_BYTES: u64 = 512 * 1024;
const MAX_SEARCH_BUDGET: usize = 20;
const INTERNAL_TASK_ERROR_CODE: &str = "internal-task-failed";
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

async fn run_blocking_task<T, F>(job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| error.to_string())
}

fn configure_child_process(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
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

fn clear_attempt_output_path(attempts: &mut [SearchAttemptResult], candidate_id: &str) {
    if let Some(attempt) = attempts
        .iter_mut()
        .find(|attempt| attempt.candidate_id == candidate_id)
    {
        attempt.output_path = None;
    }
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
            if duration_seconds > RECOMMENDED_MAX_DURATION_SECONDS {
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

fn sanitize_path_fragment(value: &str, fallback: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();

    if sanitized.is_empty() {
        fallback.into()
    } else {
        sanitized
    }
}

fn sanitized_source_stem(input_path: &str) -> String {
    let source_stem = Path::new(input_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("image");

    sanitize_path_fragment(source_stem, "image")
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

fn resolve_tool(tool: &str, locale: UiLocale) -> Result<ToolResolution, String> {
    let expected = expected_sidecar_name(tool);
    let candidate_paths = sidecar_candidate_paths(tool);
    let attempted_paths: Vec<String> = candidate_paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();

    if let Some(existing_path) = candidate_paths.iter().find(|path| path.is_file()).cloned() {
        return Ok(ToolResolution {
            source: "sidecar",
            command: existing_path.as_os_str().to_os_string(),
            command_display: existing_path.to_string_lossy().into_owned(),
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
    args: &[&str],
    locale: UiLocale,
) -> Result<CommandOutput, String> {
    let mut command = Command::new(&resolution.command);
    configure_child_process(&mut command);

    let output = command
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;

    if output.status.success() {
        Ok(CommandOutput {
            resolution: resolution.clone(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    } else {
        Err(
            first_output_line(&output.stdout, &output.stderr).unwrap_or_else(|| {
                locale::command_non_zero_exit_message(&resolution.command_display, locale)
            }),
        )
    }
}

fn run_sidecar_tool(
    tool: &str,
    args: &[&str],
    locale: UiLocale,
) -> Result<CommandOutput, ToolRunError> {
    let resolved = match resolve_tool(tool, locale) {
        Ok(resolved) => resolved,
        Err(message) => {
            return Err(ToolRunError {
                resolution: ToolResolution {
                    source: "missing",
                    command: OsString::new(),
                    command_display: tool.into(),
                    attempted_sidecar_paths: sidecar_candidate_paths(tool)
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect(),
                    fallback_reason: None,
                },
                system_error: message,
            })
        }
    };

    match run_resolved_command(&resolved, args, locale) {
        Ok(output) => Ok(output),
        Err(message) => Err(ToolRunError {
            resolution: resolved,
            system_error: message,
        }),
    }
}

fn run_with_fallback(
    tool: &str,
    args: &[&str],
    locale: UiLocale,
) -> Result<CommandOutput, ToolRunError> {
    run_sidecar_tool(tool, args, locale)
}

fn check_tool(tool: &str, locale: UiLocale) -> ToolCheck {
    let expected = expected_sidecar_name(tool);

    match run_sidecar_tool(tool, &["-version"], locale) {
        Ok(output) => ToolCheck {
            tool: tool.to_string(),
            available: true,
            source: output.resolution.source.to_string(),
            resolved_command: Some(output.resolution.command_display.clone()),
            fallback_reason: output.resolution.fallback_reason.clone(),
            version_line: first_output_line(&output.stdout, &output.stderr),
            detail: locale::tool_check_sidecar_ok_detail(
                locale,
                tool,
                &output.resolution.command_display,
            ),
            expected_sidecar_name: expected,
            attempted_sidecar_paths: output.resolution.attempted_sidecar_paths.clone(),
        },
        Err(error) => ToolCheck {
            tool: tool.to_string(),
            available: false,
            source: "missing".into(),
            resolved_command: None,
            fallback_reason: error.resolution.fallback_reason.clone(),
            version_line: None,
            detail: error.system_error,
            expected_sidecar_name: expected,
            attempted_sidecar_paths: error.resolution.attempted_sidecar_paths,
        },
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

    let Some(base_frame_count) = base_frame_count.filter(|count| *count > 0) else {
        return Err("invalid-frame-selection");
    };

    let mut resolved = Vec::with_capacity(timeline_frames.len());

    for frame in timeline_frames {
        if frame.source_frame_id == 0 || frame.source_frame_id > base_frame_count {
            return Err("invalid-frame-selection");
        }

        if frame.duration_us == 0 {
            return Err("invalid-frame-selection");
        }

        resolved.push(ResolvedTimelineFrame {
            source_frame_index: frame.source_frame_id - 1,
            duration_seconds: frame.duration_us as f64 / 1_000_000.0,
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

    let required_step =
        (selected_frame_count as f64 / (DISCORD_MAX_DURATION_SECONDS * 30.0)).ceil() as u32;
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
            if candidate_duration_seconds(selected_frame_count, 30) <= DISCORD_MAX_DURATION_SECONDS
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

fn timeline_duration_seconds(timeline_frames: &[ResolvedTimelineFrame]) -> f64 {
    timeline_frames
        .iter()
        .map(|frame| frame.duration_seconds)
        .sum()
}

fn timeline_average_fps(timeline_frames: &[ResolvedTimelineFrame]) -> f64 {
    let duration_seconds = timeline_duration_seconds(timeline_frames);
    if duration_seconds <= 0.0 {
        1.0
    } else {
        timeline_frames.len() as f64 / duration_seconds
    }
}

fn build_candidate_ladder_fixed_duration(
    duration_seconds: f64,
    fps: u32,
    input_width: Option<u32>,
    input_height: Option<u32>,
    fit_mode: &str,
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
                    fit_mode,
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
                fit_mode: fit_mode.into(),
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
    fit_mode: &str,
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

    match fit_mode {
        "cover" => format!(
            "{crop_prefix}{selection_prefix}fps={fps},scale={target_width}:{target_height}:force_original_aspect_ratio=increase,crop={target_width}:{target_height},format=rgba,setsar=1"
        ),
        "fill" => format!(
            "{crop_prefix}{selection_prefix}fps={fps},scale={target_width}:{target_height},format=rgba,setsar=1"
        ),
        _ => format!(
            "{crop_prefix}{selection_prefix}fps={fps},scale={target_width}:{target_height}:force_original_aspect_ratio=decrease,format=rgba,pad={target_width}:{target_height}:(ow-iw)/2:(oh-ih)/2:color=black@0.0,setsar=1"
        ),
    }
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

fn frame_delay_seconds(delay_ms: u32, delay_den_ms: u32) -> f64 {
    if delay_den_ms == 0 {
        f64::from(delay_ms) / 100_000.0
    } else {
        f64::from(delay_ms) / f64::from(delay_den_ms) / 1000.0
    }
}

fn sticker_frame_delay(duration_seconds: f64) -> (u16, u16) {
    let duration_ms = (duration_seconds * 1000.0)
        .round()
        .clamp(1.0, u16::MAX as f64) as u16;
    (duration_ms, 1000)
}

fn crop_rgba_image(source: &RgbaImage, crop_region: Option<ResolvedCropRegion>) -> RgbaImage {
    match crop_region {
        Some(crop) => {
            imageops::crop_imm(source, crop.x, crop.y, crop.width, crop.height).to_image()
        }
        None => source.clone(),
    }
}

fn fit_contain_rgba_image(source: &RgbaImage, target_width: u32, target_height: u32) -> RgbaImage {
    if source.width() == target_width && source.height() == target_height {
        return source.clone();
    }

    let width_scale = target_width as f64 / source.width() as f64;
    let height_scale = target_height as f64 / source.height() as f64;
    let scale = width_scale.min(height_scale);
    let resized_width = (source.width() as f64 * scale)
        .round()
        .clamp(1.0, target_width as f64) as u32;
    let resized_height = (source.height() as f64 * scale)
        .round()
        .clamp(1.0, target_height as f64) as u32;
    let resized = imageops::resize(source, resized_width, resized_height, FilterType::Lanczos3);
    let mut canvas = RgbaImage::from_pixel(target_width, target_height, Rgba([0, 0, 0, 0]));
    let offset_x = (target_width.saturating_sub(resized_width)) / 2;
    let offset_y = (target_height.saturating_sub(resized_height)) / 2;
    let _ = canvas.copy_from(&resized, offset_x, offset_y);
    canvas
}

fn fit_cover_rgba_image(source: &RgbaImage, target_width: u32, target_height: u32) -> RgbaImage {
    if source.width() == target_width && source.height() == target_height {
        return source.clone();
    }

    let width_scale = target_width as f64 / source.width() as f64;
    let height_scale = target_height as f64 / source.height() as f64;
    let scale = width_scale.max(height_scale);
    let resized_width = (source.width() as f64 * scale)
        .round()
        .max(target_width as f64) as u32;
    let resized_height = (source.height() as f64 * scale)
        .round()
        .max(target_height as f64) as u32;
    let resized = imageops::resize(source, resized_width, resized_height, FilterType::Lanczos3);
    let offset_x = (resized_width.saturating_sub(target_width)) / 2;
    let offset_y = (resized_height.saturating_sub(target_height)) / 2;
    imageops::crop_imm(&resized, offset_x, offset_y, target_width, target_height).to_image()
}

fn transform_frame_for_candidate(
    source: &RgbaImage,
    fit_mode: &str,
    content_scale: f64,
    crop_region: Option<ResolvedCropRegion>,
) -> RgbaImage {
    let cropped = crop_rgba_image(source, crop_region);
    let (target_width, target_height) =
        scaled_output_dimensions(Some(cropped.width()), Some(cropped.height()), content_scale);

    match fit_mode {
        "cover" => fit_cover_rgba_image(&cropped, target_width, target_height),
        "fill" => imageops::resize(&cropped, target_width, target_height, FilterType::Lanczos3),
        _ => fit_contain_rgba_image(&cropped, target_width, target_height),
    }
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

fn decode_still_rgba_image(input_path: &str) -> Result<RgbaImage, String> {
    ImageReader::open(input_path)
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|error| error.to_string())
        .map(|image| image.into_rgba8())
}

fn decode_gif_animation_frames(input_path: &str) -> Result<Vec<StickerFrame>, String> {
    let file = File::open(input_path).map_err(|error| error.to_string())?;
    let decoder = ImageGifDecoder::new(BufReader::new(file)).map_err(|error| error.to_string())?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .map_err(|error| error.to_string())?;

    Ok(frames
        .into_iter()
        .map(|frame| {
            let (delay_ms, delay_den_ms) = frame.delay().numer_denom_ms();
            StickerFrame {
                pixels: frame.into_buffer(),
                duration_seconds: frame_delay_seconds(delay_ms, delay_den_ms),
            }
        })
        .collect())
}

fn image_frame_to_sticker_frame(frame: image::Frame) -> StickerFrame {
    let (delay_ms, delay_den_ms) = frame.delay().numer_denom_ms();
    StickerFrame {
        pixels: frame.into_buffer(),
        duration_seconds: frame_delay_seconds(delay_ms, delay_den_ms),
    }
}

fn decode_gif_animation_frame(
    input_path: &str,
    frame_index: usize,
) -> Result<StickerFrame, String> {
    let file = File::open(input_path).map_err(|error| error.to_string())?;
    let decoder = ImageGifDecoder::new(BufReader::new(file)).map_err(|error| error.to_string())?;
    let frame = decoder
        .into_frames()
        .nth(frame_index)
        .ok_or_else(|| "source frame id is out of range".to_string())?
        .map_err(|error| error.to_string())?;

    Ok(image_frame_to_sticker_frame(frame))
}

fn decode_apng_animation_frames(input_path: &str) -> Result<Vec<StickerFrame>, String> {
    let file = File::open(input_path).map_err(|error| error.to_string())?;
    let decoder = ImagePngDecoder::new(BufReader::new(file)).map_err(|error| error.to_string())?;
    let apng_decoder = decoder.apng().map_err(|error| error.to_string())?;
    let frames = apng_decoder
        .into_frames()
        .collect_frames()
        .map_err(|error| error.to_string())?;

    Ok(frames
        .into_iter()
        .map(|frame| {
            let (delay_ms, delay_den_ms) = frame.delay().numer_denom_ms();
            StickerFrame {
                pixels: frame.into_buffer(),
                duration_seconds: frame_delay_seconds(delay_ms, delay_den_ms),
            }
        })
        .collect())
}

fn decode_apng_animation_frame(
    input_path: &str,
    frame_index: usize,
) -> Result<StickerFrame, String> {
    let file = File::open(input_path).map_err(|error| error.to_string())?;
    let decoder = ImagePngDecoder::new(BufReader::new(file)).map_err(|error| error.to_string())?;
    let apng_decoder = decoder.apng().map_err(|error| error.to_string())?;
    let frame = apng_decoder
        .into_frames()
        .nth(frame_index)
        .ok_or_else(|| "source frame id is out of range".to_string())?
        .map_err(|error| error.to_string())?;

    Ok(image_frame_to_sticker_frame(frame))
}

fn decode_native_animation_frames(input_path: &str) -> Result<Vec<StickerFrame>, String> {
    match lowercase_source_extension(input_path).as_deref() {
        Some("gif") => decode_gif_animation_frames(input_path),
        Some("apng") | Some("png") => decode_apng_animation_frames(input_path),
        _ => Err("unsupported native animation source".into()),
    }
}

fn decode_native_animation_frame(
    input_path: &str,
    source_frame_id: u32,
) -> Result<StickerFrame, String> {
    if source_frame_id == 0 {
        return Err("source frame id must be one-based".into());
    }

    let frame_index = (source_frame_id - 1) as usize;
    match lowercase_source_extension(input_path).as_deref() {
        Some("gif") => decode_gif_animation_frame(input_path, frame_index),
        Some("apng") | Some("png") => decode_apng_animation_frame(input_path, frame_index),
        _ => Err("unsupported native animation source".into()),
    }
}

fn write_native_png(output_path: &Path, pixels: &RgbaImage) -> Result<(), String> {
    let file = File::create(output_path).map_err(|error| error.to_string())?;
    let writer = BufWriter::new(file);
    let mut encoder = NativePngEncoder::new(writer, pixels.width(), pixels.height());
    encoder.set_color(PngColorType::Rgba);
    encoder.set_depth(PngBitDepth::Eight);
    encoder.set_deflate_compression(PngDeflateCompression::Level(9));
    encoder.set_filter(PngFilter::Adaptive);
    let mut png_writer = encoder.write_header().map_err(|error| error.to_string())?;
    png_writer
        .write_image_data(pixels.as_raw())
        .map_err(|error| error.to_string())?;
    png_writer.finish().map_err(|error| error.to_string())
}

fn encode_native_png_bytes(pixels: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut encoder = NativePngEncoder::new(&mut bytes, pixels.width(), pixels.height());
        encoder.set_color(PngColorType::Rgba);
        encoder.set_depth(PngBitDepth::Eight);
        encoder.set_deflate_compression(PngDeflateCompression::Level(9));
        encoder.set_filter(PngFilter::Adaptive);
        let mut png_writer = encoder.write_header().map_err(|error| error.to_string())?;
        png_writer
            .write_image_data(pixels.as_raw())
            .map_err(|error| error.to_string())?;
        png_writer.finish().map_err(|error| error.to_string())?;
    }

    Ok(bytes)
}

fn encode_native_png_data_url(pixels: &RgbaImage) -> Result<String, String> {
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
    let Some(extension) = lowercase_source_extension(input_path) else {
        return FramePreviewResponse {
            ok: false,
            data_url: None,
            width: None,
            height: None,
            error_code: Some("unsupported-source-format".into()),
            error_message: Some(locale::unsupported_still_image_error(locale)),
        };
    };

    if source_frame_id == 0 {
        return FramePreviewResponse {
            ok: false,
            data_url: None,
            width: None,
            height: None,
            error_code: Some("invalid-frame-selection".into()),
            error_message: Some("source frame id must be one-based".into()),
        };
    }

    if !matches!(extension.as_str(), "gif" | "apng" | "png") {
        return FramePreviewResponse {
            ok: false,
            data_url: None,
            width: None,
            height: None,
            error_code: Some("unsupported-frame-preview".into()),
            error_message: Some(
                "frame preview extraction is available for native animation sources".into(),
            ),
        };
    }

    let frame = match decode_native_animation_frame(input_path, source_frame_id) {
        Ok(frame) => frame,
        Err(error) => {
            return FramePreviewResponse {
                ok: false,
                data_url: None,
                width: None,
                height: None,
                error_code: Some("frame-preview-decode-failed".into()),
                error_message: Some(error),
            };
        }
    };

    match encode_native_png_data_url(&frame.pixels) {
        Ok(data_url) => FramePreviewResponse {
            ok: true,
            data_url: Some(data_url),
            width: Some(frame.pixels.width()),
            height: Some(frame.pixels.height()),
            error_code: None,
            error_message: None,
        },
        Err(error) => FramePreviewResponse {
            ok: false,
            data_url: None,
            width: None,
            height: None,
            error_code: Some("frame-preview-encode-failed".into()),
            error_message: Some(error),
        },
    }
}

fn frame_previews_error(error_code: &str, error_message: String) -> FramePreviewsResponse {
    FramePreviewsResponse {
        ok: false,
        previews: Vec::new(),
        error_code: Some(error_code.into()),
        error_message: Some(error_message),
    }
}

fn extract_frame_previews_internal(
    input_path: &str,
    source_frame_ids: &[u32],
    locale: UiLocale,
) -> FramePreviewsResponse {
    let Some(extension) = lowercase_source_extension(input_path) else {
        return frame_previews_error(
            "unsupported-source-format",
            locale::unsupported_still_image_error(locale),
        );
    };

    if !matches!(extension.as_str(), "gif" | "apng" | "png") {
        return frame_previews_error(
            "unsupported-frame-preview",
            "frame preview extraction is available for native animation sources".into(),
        );
    }

    let requested_frame_ids = source_frame_ids
        .iter()
        .copied()
        .filter(|source_frame_id| *source_frame_id > 0)
        .collect::<BTreeSet<_>>();
    if requested_frame_ids.is_empty() {
        return frame_previews_error(
            "invalid-frame-selection",
            "at least one source frame id is required".into(),
        );
    }

    let frames = match decode_native_animation_frames(input_path) {
        Ok(frames) => frames,
        Err(error) => return frame_previews_error("frame-preview-decode-failed", error),
    };

    let mut previews = Vec::with_capacity(requested_frame_ids.len());
    for source_frame_id in requested_frame_ids {
        let Some(frame) = frames.get((source_frame_id - 1) as usize) else {
            return frame_previews_error(
                "invalid-frame-selection",
                "source frame id is out of range".into(),
            );
        };

        let data_url = match encode_native_png_data_url(&frame.pixels) {
            Ok(data_url) => data_url,
            Err(error) => return frame_previews_error("frame-preview-encode-failed", error),
        };

        previews.push(FramePreviewItem {
            source_frame_id,
            data_url,
            width: frame.pixels.width(),
            height: frame.pixels.height(),
        });
    }

    FramePreviewsResponse {
        ok: true,
        previews,
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

fn write_native_apng(
    output_path: &Path,
    frames: &[StickerFrame],
    preset: &str,
) -> Result<(), String> {
    if frames.is_empty() {
        return Err("no frames available for APNG output".into());
    }

    let width = frames[0].pixels.width();
    let height = frames[0].pixels.height();
    if frames
        .iter()
        .any(|frame| frame.pixels.width() != width || frame.pixels.height() != height)
    {
        return Err("APNG frames must share one canvas size".into());
    }

    let file = File::create(output_path).map_err(|error| error.to_string())?;
    let writer = BufWriter::new(file);
    let mut encoder = NativePngEncoder::new(writer, width, height);
    encoder.set_color(PngColorType::Rgba);
    encoder.set_depth(PngBitDepth::Eight);
    encoder
        .set_animated(frames.len() as u32, 0)
        .map_err(|error| error.to_string())?;
    encoder
        .set_sep_def_img(false)
        .map_err(|error| error.to_string())?;
    encoder.set_deflate_compression(native_png_deflate_for_preset(preset));
    encoder.set_filter(native_png_filter_for_preset(preset));
    encoder
        .set_blend_op(PngBlendOp::Source)
        .map_err(|error| error.to_string())?;
    encoder
        .set_dispose_op(PngDisposeOp::None)
        .map_err(|error| error.to_string())?;
    let (delay_num, delay_den) = sticker_frame_delay(frames[0].duration_seconds);
    encoder
        .set_frame_delay(delay_num, delay_den)
        .map_err(|error| error.to_string())?;
    encoder.validate_sequence(true);

    let mut png_writer = encoder.write_header().map_err(|error| error.to_string())?;
    png_writer
        .write_image_data(frames[0].pixels.as_raw())
        .map_err(|error| error.to_string())?;

    let mut previous_frame = frames[0].pixels.clone();
    for frame in &frames[1..] {
        let (delay_num, delay_den) = sticker_frame_delay(frame.duration_seconds);
        let region = changed_frame_region(&previous_frame, &frame.pixels);
        png_writer
            .reset_frame_position()
            .map_err(|error| error.to_string())?;
        png_writer
            .set_frame_dimension(region.width, region.height)
            .map_err(|error| error.to_string())?;
        png_writer
            .set_frame_position(region.x, region.y)
            .map_err(|error| error.to_string())?;
        png_writer
            .set_frame_delay(delay_num, delay_den)
            .map_err(|error| error.to_string())?;
        png_writer
            .set_blend_op(PngBlendOp::Source)
            .map_err(|error| error.to_string())?;
        png_writer
            .set_dispose_op(PngDisposeOp::None)
            .map_err(|error| error.to_string())?;
        let region_pixels = frame_region_pixels(&frame.pixels, region);
        png_writer
            .write_image_data(&region_pixels)
            .map_err(|error| error.to_string())?;
        previous_frame = frame.pixels.clone();
    }

    png_writer.finish().map_err(|error| error.to_string())
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

    let duration_seconds = 1.0 / candidate_fps.max(1) as f64;
    frame_indexes
        .into_iter()
        .map(|index| {
            source_frames
                .get(index as usize)
                .cloned()
                .map(|frame| StickerFrame {
                    pixels: frame.pixels,
                    duration_seconds,
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
                    duration_seconds: frame.duration_seconds,
                })
                .ok_or_else(|| "timeline frame index is out of range".to_string())
        })
        .collect()
}

fn raw_rgba_frame_size(width: u32, height: u32) -> Result<usize, String> {
    usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(height as usize))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "raw frame size overflow".to_string())
}

fn rgba_frame_from_bytes(width: u32, height: u32, bytes: Vec<u8>) -> Result<RgbaImage, String> {
    RgbaImage::from_raw(width, height, bytes)
        .ok_or_else(|| "ffmpeg raw frame buffer size did not match image dimensions".to_string())
}

fn decode_video_frames_with_ffmpeg(
    input_path: &str,
    filter_graph: &str,
    frame_width: u32,
    frame_height: u32,
    locale: UiLocale,
) -> Result<(Vec<RgbaImage>, ToolResolution), String> {
    let args = [
        "-v",
        "error",
        "-i",
        input_path,
        "-vf",
        filter_graph,
        "-pix_fmt",
        "rgba",
        "-f",
        "rawvideo",
        "-an",
        "-",
    ];

    let output = run_with_fallback("ffmpeg", &args, locale).map_err(|error| error.system_error)?;
    let frame_size = raw_rgba_frame_size(frame_width, frame_height)?;
    if frame_size == 0 || output.stdout.is_empty() || output.stdout.len() % frame_size != 0 {
        return Err("ffmpeg did not return a whole sequence of RGBA frames".into());
    }

    let frames = output
        .stdout
        .chunks(frame_size)
        .map(|chunk| rgba_frame_from_bytes(frame_width, frame_height, chunk.to_vec()))
        .collect::<Result<Vec<_>, _>>()?;

    Ok((frames, output.resolution))
}

fn extract_video_source_frames_rgba(
    input_path: &str,
    frame_indexes: &BTreeSet<u32>,
    frame_width: u32,
    frame_height: u32,
    locale: UiLocale,
) -> Result<(BTreeMap<u32, RgbaImage>, ToolResolution), String> {
    if frame_indexes.is_empty() {
        return Err("no timeline frames requested for extraction".into());
    }

    let select_filter = build_source_frame_select_filter(frame_indexes);
    let args = [
        "-v",
        "error",
        "-i",
        input_path,
        "-vf",
        select_filter.as_str(),
        "-pix_fmt",
        "rgba",
        "-f",
        "rawvideo",
        "-an",
        "-",
    ];

    let output = run_with_fallback("ffmpeg", &args, locale).map_err(|error| error.system_error)?;
    let frame_size = raw_rgba_frame_size(frame_width, frame_height)?;
    if frame_size == 0 || output.stdout.len() != frame_size * frame_indexes.len() {
        return Err("ffmpeg did not return the requested RGBA source frames".into());
    }

    let frames = frame_indexes
        .iter()
        .copied()
        .zip(output.stdout.chunks(frame_size))
        .map(|(frame_index, chunk)| {
            rgba_frame_from_bytes(frame_width, frame_height, chunk.to_vec())
                .map(|pixels| (frame_index, pixels))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;

    Ok((frames, output.resolution))
}

fn normalized_fit_mode(raw: &str, locale: UiLocale) -> (&'static str, Option<String>) {
    match raw {
        "cover" => ("cover", None),
        "contain" => ("contain", None),
        "fill" => ("fill", None),
        _ => (
            "contain",
            Some(locale::unknown_fit_mode_fallback_warning(locale)),
        ),
    }
}

fn build_candidate_ladder(
    selected_frame_count: usize,
    source_fps: f64,
    input_width: Option<u32>,
    input_height: Option<u32>,
    fit_mode: &str,
    preset_strategy: &str,
    optimizer_goal: &str,
    search_budget: usize,
    locale: UiLocale,
) -> Vec<CandidatePreview> {
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
        let encoded_frame_count = sampled_frame_count(selected_frame_count, frame_sample_step);
        let fps_ladder: Vec<u32> = [30, 27, 24, 21, 18, 15, 12, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1]
            .into_iter()
            .filter(|fps| {
                candidate_duration_seconds(encoded_frame_count, *fps)
                    <= DISCORD_MAX_DURATION_SECONDS
            })
            .collect();

        for fps in fps_ladder {
            let duration_seconds = candidate_duration_seconds(encoded_frame_count, fps);
            let preset_ladder = preset_ladder_for_strategy(duration_seconds, preset_strategy);

            for scale in &scale_ladder {
                for preset in &preset_ladder {
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
                            fit_mode,
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
                        fit_mode: fit_mode.into(),
                        score,
                        source_similarity_score: score,
                        summary,
                        frame_sample_step,
                    });
                }
            }
        }
    }

    select_ranked_candidate_subset(candidates, search_budget)
}

fn prepare_optimizer_plan(
    request: &OptimizerPlanRequest,
    locale: UiLocale,
) -> OptimizerPlanResponse {
    let (fit_mode, fit_warning) = normalized_fit_mode(&request.fit_mode, locale);
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
                recommended_max_duration_seconds: RECOMMENDED_MAX_DURATION_SECONDS,
                search_budget,
                warnings,
                candidates: Vec::new(),
                error_code: Some("invalid-crop".into()),
                error_message: Some(error),
            }
        }
    };

    if resolved_crop_region.is_some() {
        warnings.push(locale::crop_applied_before_fit_warning(locale));
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
                                recommended_max_duration_seconds: RECOMMENDED_MAX_DURATION_SECONDS,
                                search_budget,
                                warnings,
                                candidates: Vec::new(),
                                error_code: Some(error.into()),
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
                    recommended_max_duration_seconds: RECOMMENDED_MAX_DURATION_SECONDS,
                    search_budget,
                    warnings,
                    candidates: Vec::new(),
                    error_code: Some("no-frames-selected".into()),
                    error_message: Some(locale::frame_selection_required_error(locale)),
                };
            }
            Err(_) => {
                return OptimizerPlanResponse {
                    ok: false,
                    fit_mode: fit_mode.into(),
                    selected_duration_seconds: None,
                    recommended_max_duration_seconds: RECOMMENDED_MAX_DURATION_SECONDS,
                    search_budget,
                    warnings,
                    candidates: Vec::new(),
                    error_code: Some("invalid-frame-selection".into()),
                    error_message: Some(locale::invalid_frame_selection_error(locale)),
                };
            }
        }
    {
        let total_duration_seconds = timeline_duration_seconds(&timeline_frames);

        if total_duration_seconds > DISCORD_MAX_DURATION_SECONDS {
            return OptimizerPlanResponse {
                ok: false,
                fit_mode: fit_mode.into(),
                selected_duration_seconds: Some(total_duration_seconds),
                recommended_max_duration_seconds: RECOMMENDED_MAX_DURATION_SECONDS,
                search_budget,
                warnings,
                candidates: Vec::new(),
                error_code: Some("duration-too-long".into()),
                error_message: Some(locale::selected_duration_limit_error(locale)),
            };
        }

        if total_duration_seconds > RECOMMENDED_MAX_DURATION_SECONDS {
            warnings.push(locale::recommended_duration_warning(locale));
        }

        let effective_fps = timeline_average_fps(&timeline_frames)
            .round()
            .clamp(1.0, 30.0) as u32;

        return OptimizerPlanResponse {
            ok: true,
            fit_mode: fit_mode.into(),
            selected_duration_seconds: Some(total_duration_seconds),
            recommended_max_duration_seconds: RECOMMENDED_MAX_DURATION_SECONDS,
            search_budget,
            warnings,
            candidates: build_candidate_ladder_fixed_duration(
                total_duration_seconds,
                effective_fps,
                effective_input_width,
                effective_input_height,
                fit_mode,
                preset_strategy,
                optimizer_goal,
                search_budget,
                locale,
            ),
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
                    recommended_max_duration_seconds: RECOMMENDED_MAX_DURATION_SECONDS,
                    search_budget,
                    warnings,
                    candidates: Vec::new(),
                    error_code: Some("no-frames-selected".into()),
                    error_message: Some(locale::frame_selection_required_error(locale)),
                };
            }
            Err(_) => {
                return OptimizerPlanResponse {
                    ok: false,
                    fit_mode: fit_mode.into(),
                    selected_duration_seconds: None,
                    recommended_max_duration_seconds: RECOMMENDED_MAX_DURATION_SECONDS,
                    search_budget,
                    warnings,
                    candidates: Vec::new(),
                    error_code: Some("invalid-frame-selection".into()),
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
                    recommended_max_duration_seconds: RECOMMENDED_MAX_DURATION_SECONDS,
                    search_budget,
                    warnings,
                    candidates: Vec::new(),
                    error_code: Some(error.into()),
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

    if shortest_duration_seconds > DISCORD_MAX_DURATION_SECONDS {
        return OptimizerPlanResponse {
            ok: false,
            fit_mode: fit_mode.into(),
            selected_duration_seconds: Some(shortest_duration_seconds),
            recommended_max_duration_seconds: RECOMMENDED_MAX_DURATION_SECONDS,
            search_budget,
            warnings,
            candidates: Vec::new(),
            error_code: Some("duration-too-long".into()),
            error_message: Some(locale::selected_duration_limit_error(locale)),
        };
    }

    if natural_duration_seconds > RECOMMENDED_MAX_DURATION_SECONDS {
        warnings.push(locale::recommended_duration_warning(locale));
    }

    OptimizerPlanResponse {
        ok: true,
        fit_mode: fit_mode.into(),
        selected_duration_seconds: Some(natural_duration_seconds),
        recommended_max_duration_seconds: RECOMMENDED_MAX_DURATION_SECONDS,
        search_budget,
        warnings,
        candidates: build_candidate_ladder(
            frame_selection.selected_frame_count,
            source_fps,
            effective_input_width,
            effective_input_height,
            fit_mode,
            preset_strategy,
            optimizer_goal,
            search_budget,
            locale,
        ),
        error_code: None,
        error_message: None,
    }
}

fn make_output_path(
    output_directory: &Path,
    input_path: &str,
    suffix: &str,
    extension: &str,
) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    output_directory.join(format!(
        "{}-{}-{}.{}",
        sanitized_source_stem(input_path),
        suffix,
        timestamp,
        extension
    ))
}

fn encode_candidate_from_native_animation_internal(
    input_path: &str,
    output_directory: Option<&str>,
    locale: UiLocale,
    crop_region: Option<&CropRegion>,
    input_width: Option<u32>,
    input_height: Option<u32>,
    candidate: &CandidatePreview,
    timeline_frames: &[ResolvedTimelineFrame],
) -> Result<EncodeResult, String> {
    let output_directory = resolve_output_directory(output_directory, input_path, locale)?;
    let output_path = make_output_path(&output_directory, input_path, &candidate.id, "png");
    let resolved_crop_region = resolve_crop_region(crop_region, input_width, input_height, locale)?;
    let source_frames = decode_native_animation_frames(input_path)?;
    let frames = build_native_timeline_frames(&source_frames, timeline_frames)?
        .into_iter()
        .map(|frame| StickerFrame {
            pixels: transform_frame_for_candidate(
                &frame.pixels,
                &candidate.fit_mode,
                candidate.content_scale,
                resolved_crop_region,
            ),
            duration_seconds: frame.duration_seconds,
        })
        .collect::<Vec<_>>();
    let started = Instant::now();
    write_native_apng(&output_path, &frames, &candidate.preset)?;
    let metadata = fs::metadata(&output_path).map_err(|error| error.to_string())?;

    Ok(EncodeResult {
        output_path,
        size_bytes: metadata.len(),
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
) -> Result<EncodeResult, String> {
    let output_directory = resolve_output_directory(output_directory, input_path, locale)?;
    let output_path = make_output_path(&output_directory, input_path, &candidate.id, "png");
    let resolved_crop_region = resolve_crop_region(crop_region, input_width, input_height, locale)?;
    let source_width = input_width
        .filter(|width| *width > 0)
        .ok_or_else(|| locale::crop_needs_source_width_error(locale))?;
    let source_height = input_height
        .filter(|height| *height > 0)
        .ok_or_else(|| locale::crop_needs_source_height_error(locale))?;
    let unique_frame_indexes = timeline_frames
        .iter()
        .map(|frame| frame.source_frame_index)
        .collect::<BTreeSet<_>>();
    let (source_frames, resolution) = extract_video_source_frames_rgba(
        input_path,
        &unique_frame_indexes,
        source_width,
        source_height,
        locale,
    )?;

    let frames = timeline_frames
        .iter()
        .map(|frame| {
            source_frames
                .get(&frame.source_frame_index)
                .map(|pixels| StickerFrame {
                    pixels: transform_frame_for_candidate(
                        pixels,
                        &candidate.fit_mode,
                        candidate.content_scale,
                        resolved_crop_region,
                    ),
                    duration_seconds: frame.duration_seconds,
                })
                .ok_or_else(|| "timeline frame index is out of range".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let started = Instant::now();
    write_native_apng(&output_path, &frames, &candidate.preset)?;
    let metadata = fs::metadata(&output_path).map_err(|error| error.to_string())?;

    Ok(EncodeResult {
        output_path,
        size_bytes: metadata.len(),
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
) -> Result<EncodeResult, String> {
    let output_directory = resolve_output_directory(output_directory, input_path, locale)?;
    let output_path = make_output_path(&output_directory, input_path, &candidate.id, "png");
    let resolved_crop_region = resolve_crop_region(crop_region, input_width, input_height, locale)?;
    let filter_graph = build_filter_graph(
        &candidate.fit_mode,
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
    let (pixels, resolution) = decode_video_frames_with_ffmpeg(
        input_path,
        &filter_graph,
        frame_width,
        frame_height,
        locale,
    )?;
    let frames = pixels
        .into_iter()
        .map(|pixels| StickerFrame {
            pixels,
            duration_seconds: 1.0 / candidate.fps.max(1) as f64,
        })
        .collect::<Vec<_>>();
    let started = Instant::now();
    write_native_apng(&output_path, &frames, &candidate.preset)?;
    let metadata = fs::metadata(&output_path).map_err(|error| error.to_string())?;

    Ok(EncodeResult {
        output_path,
        size_bytes: metadata.len(),
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
) -> Result<EncodeResult, String> {
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
            ),
        };
    }

    if matches!(extension.as_str(), "gif" | "apng") {
        let output_directory = resolve_output_directory(output_directory, input_path, locale)?;
        let output_path = make_output_path(&output_directory, input_path, &candidate.id, "png");
        let resolved_crop_region =
            resolve_crop_region(crop_region, input_width, input_height, locale)?;
        let source_frames = decode_native_animation_frames(input_path)?;
        let frames =
            build_native_selected_animation_frames(&source_frames, selected_frames, candidate.fps)?
                .into_iter()
                .map(|frame| StickerFrame {
                    pixels: transform_frame_for_candidate(
                        &frame.pixels,
                        &candidate.fit_mode,
                        candidate.content_scale,
                        resolved_crop_region,
                    ),
                    duration_seconds: frame.duration_seconds,
                })
                .collect::<Vec<_>>();
        let started = Instant::now();
        write_native_apng(&output_path, &frames, &candidate.preset)?;
        let metadata = fs::metadata(&output_path).map_err(|error| error.to_string())?;

        return Ok(EncodeResult {
            output_path,
            size_bytes: metadata.len(),
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
    )
}

fn convert_static_image_to_png_internal(
    input_path: &str,
    output_directory: Option<&str>,
    crop_region: Option<&CropRegion>,
    locale: UiLocale,
) -> StaticImageConversionResult {
    let inspection = inspect_input_media_internal(input_path, locale);

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
            error_code: Some("unsupported-source-format".into()),
            error_message: Some(locale::unsupported_still_image_error(locale)),
        };
    }

    let output_directory = match resolve_output_directory(output_directory, input_path, locale) {
        Ok(directory) => directory,
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
                error_code: Some("invalid-output-directory".into()),
                error_message: Some(error),
            }
        }
    };

    let output_path = make_output_path(&output_directory, input_path, "png", "png");
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
                    error_code: Some("invalid-crop".into()),
                    error_message: Some(error),
                }
            }
        };

    let started = Instant::now();
    let source_pixels = match decode_still_rgba_image(input_path) {
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
                error_code: Some("decode-failed".into()),
                error_message: Some(error),
            }
        }
    };
    let output_pixels = transform_frame_for_static_png(&source_pixels, resolved_crop_region);

    match write_native_png(&output_path, &output_pixels) {
        Ok(()) => {
            let metadata = match fs::metadata(&output_path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    return StaticImageConversionResult {
                        ok: false,
                        output_path: None,
                        size_bytes: None,
                        elapsed_ms: None,
                        tool_source: Some("native".into()),
                        tool_command: None,
                        tool_detail: Some(locale::native_png_encode_detail(locale)),
                        warnings: Vec::new(),
                        error_code: Some("missing-output".into()),
                        error_message: Some(error.to_string()),
                    }
                }
            };

            StaticImageConversionResult {
                ok: true,
                output_path: Some(output_path.to_string_lossy().into_owned()),
                size_bytes: Some(metadata.len()),
                elapsed_ms: Some(started.elapsed().as_millis() as u64),
                tool_source: Some("native".into()),
                tool_command: None,
                tool_detail: Some(locale::native_png_encode_detail(locale)),
                warnings: Vec::new(),
                error_code: None,
                error_message: None,
            }
        }
        Err(error) => StaticImageConversionResult {
            ok: false,
            output_path: None,
            size_bytes: None,
            elapsed_ms: None,
            tool_source: Some("native".into()),
            tool_command: None,
            tool_detail: Some(locale::native_png_encode_detail(locale)),
            warnings: Vec::new(),
            error_code: Some("encode-failed".into()),
            error_message: Some(error),
        },
    }
}

fn inspection_error(
    input_path: &str,
    tool_source: Option<String>,
    tool_command: Option<String>,
    tool_detail: Option<String>,
    error_code: &str,
    error_message: String,
) -> MediaInspection {
    MediaInspection {
        ok: false,
        input_path: input_path.to_string(),
        tool_source,
        tool_command,
        tool_detail,
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
        is_static_image: false,
        can_convert_to_png: false,
        error_code: Some(error_code.into()),
        error_message: Some(error_message),
    }
}

fn skip_png_chunk_payload<R: Seek>(reader: &mut R, length: u32) -> Result<(), String> {
    reader
        .seek(SeekFrom::Current(i64::from(length) + 4))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn read_png_chunk_payload<R: Read>(reader: &mut R, length: u32) -> Result<Vec<u8>, String> {
    let mut data = vec![0u8; length as usize];
    reader
        .read_exact(&mut data)
        .map_err(|error| error.to_string())?;
    let mut crc = [0u8; 4];
    reader
        .read_exact(&mut crc)
        .map_err(|error| error.to_string())?;
    Ok(data)
}

fn read_png_animation_metadata(input_path: &Path) -> Result<Option<PngAnimationMetadata>, String> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

    let file = File::open(input_path).map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(file);
    let mut signature = [0u8; 8];
    reader
        .read_exact(&mut signature)
        .map_err(|error| error.to_string())?;
    if &signature != PNG_SIGNATURE {
        return Ok(None);
    }

    let mut width = None;
    let mut height = None;
    let mut frame_count = None;
    let mut frame_durations = Vec::new();
    let mut saw_animation_control = false;

    loop {
        let mut length_bytes = [0u8; 4];
        match reader.read_exact(&mut length_bytes) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.to_string()),
        }

        let length = u32::from_be_bytes(length_bytes);
        let mut chunk_type = [0u8; 4];
        reader
            .read_exact(&mut chunk_type)
            .map_err(|error| error.to_string())?;

        match &chunk_type {
            b"IHDR" => {
                let data = read_png_chunk_payload(&mut reader, length)?;
                if data.len() >= 8 {
                    width = Some(u32::from_be_bytes([data[0], data[1], data[2], data[3]]));
                    height = Some(u32::from_be_bytes([data[4], data[5], data[6], data[7]]));
                }
            }
            b"acTL" => {
                let data = read_png_chunk_payload(&mut reader, length)?;
                saw_animation_control = true;
                if data.len() >= 4 {
                    frame_count =
                        Some(u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u64);
                }
            }
            b"fcTL" => {
                let data = read_png_chunk_payload(&mut reader, length)?;
                if data.len() >= 26 {
                    let delay_num = u16::from_be_bytes([data[20], data[21]]);
                    let delay_den = u16::from_be_bytes([data[22], data[23]]);
                    let denominator = if delay_den == 0 { 100 } else { delay_den };
                    let delay = if delay_num == 0 {
                        0.01
                    } else {
                        f64::from(delay_num) / f64::from(denominator)
                    };
                    frame_durations.push(delay);
                }
            }
            b"IDAT" if !saw_animation_control => {
                return Ok(None);
            }
            b"IEND" => {
                skip_png_chunk_payload(&mut reader, length)?;
                break;
            }
            _ => {
                skip_png_chunk_payload(&mut reader, length)?;
            }
        }
    }

    if !saw_animation_control {
        return Ok(None);
    }

    Ok(Some(PngAnimationMetadata {
        width: width.unwrap_or(0),
        height: height.unwrap_or(0),
        frame_count,
        frame_durations,
    }))
}

fn inspect_still_image_metadata_internal(input_path: &str, locale: UiLocale) -> MediaInspection {
    let metadata = match fs::metadata(input_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_image_detail(locale)),
                "inspect-failed",
                error.to_string(),
            );
        }
    };

    let reader = match ImageReader::open(input_path) {
        Ok(reader) => reader,
        Err(error) => {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_image_detail(locale)),
                "inspect-failed",
                error.to_string(),
            );
        }
    };

    let (width, height) = match reader.into_dimensions() {
        Ok(dimensions) => dimensions,
        Err(error) => {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_image_detail(locale)),
                "inspect-failed",
                error.to_string(),
            );
        }
    };

    let format_name = lowercase_source_extension(input_path);

    MediaInspection {
        ok: true,
        input_path: input_path.to_string(),
        tool_source: Some("native".into()),
        tool_command: None,
        tool_detail: Some(locale::native_image_detail(locale)),
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
        is_static_image: true,
        can_convert_to_png: true,
        error_code: None,
        error_message: None,
    }
}

fn inspect_gif_metadata_internal(input_path: &str, locale: UiLocale) -> MediaInspection {
    let metadata = match fs::metadata(input_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_animation_detail(locale, "gif")),
                "inspect-failed",
                error.to_string(),
            );
        }
    };

    let frames = match decode_gif_animation_frames(input_path) {
        Ok(frames) => frames,
        Err(error) => {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_animation_detail(locale, "gif")),
                "inspect-failed",
                error,
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
    let frame_durations = frames
        .iter()
        .map(|frame| frame.duration_seconds)
        .collect::<Vec<_>>();
    let estimated_frames = frame_durations.len() as u64;
    let duration_seconds = frame_durations.iter().sum::<f64>();
    let avg_fps = if duration_seconds > 0.0 {
        Some(estimated_frames as f64 / duration_seconds)
    } else {
        None
    };

    MediaInspection {
        ok: true,
        input_path: input_path.to_string(),
        tool_source: Some("native".into()),
        tool_command: None,
        tool_detail: Some(locale::native_animation_detail(locale, "gif")),
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
        is_static_image: false,
        can_convert_to_png: false,
        error_code: None,
        error_message: None,
    }
}

fn inspect_apng_metadata_internal(input_path: &str, locale: UiLocale) -> MediaInspection {
    let metadata = match fs::metadata(input_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_animation_detail(locale, "apng")),
                "inspect-failed",
                error.to_string(),
            );
        }
    };

    let animation_metadata = match read_png_animation_metadata(Path::new(input_path)) {
        Ok(Some(metadata)) => metadata,
        Ok(None) => {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_animation_detail(locale, "apng")),
                "inspect-failed",
                locale::invalid_apng_error(locale),
            );
        }
        Err(error) => {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(locale::native_animation_detail(locale, "apng")),
                "inspect-failed",
                error.to_string(),
            );
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

    MediaInspection {
        ok: true,
        input_path: input_path.to_string(),
        tool_source: Some("native".into()),
        tool_command: None,
        tool_detail: Some(locale::native_animation_detail(locale, "apng")),
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
        is_static_image: false,
        can_convert_to_png: false,
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

#[cfg(target_os = "windows")]
fn inspect_mp4_family_with_media_foundation(input_path: &str, locale: UiLocale) -> MediaInspection {
    let format_name = lowercase_source_extension(input_path).unwrap_or_else(|| "video".into());
    let detail = locale::native_video_detail(locale, &format_name);
    let result = unsafe {
        let mut should_uninitialize_com = false;
        let com_result = CoInitializeEx(None, COINIT_MULTITHREADED);
        if com_result.is_ok() {
            should_uninitialize_com = true;
        } else if com_result != RPC_E_CHANGED_MODE {
            return inspection_error(
                input_path,
                Some("native".into()),
                None,
                Some(detail),
                "inspect-failed",
                com_result.to_string(),
            );
        }

        let mut media_foundation_started = false;
        let inspection = (|| -> Result<MediaInspection, String> {
            MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|error| error.to_string())?;
            media_foundation_started = true;

            let wide_path: Vec<u16> = Path::new(input_path)
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let reader = MFCreateSourceReaderFromURL(PCWSTR(wide_path.as_ptr()), None)
                .map_err(|error| error.to_string())?;
            let media_type = reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                .map_err(|error| error.to_string())?;

            let frame_size = media_type
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map_err(|error| error.to_string())?;
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
                .map_err(|error| error.to_string())?;
            let duration_100ns =
                u64::try_from(&duration_value).map_err(|error| error.to_string())?;
            let duration_seconds =
                (duration_100ns > 0).then_some(duration_100ns as f64 / 10_000_000.0);
            let estimated_frames = duration_seconds
                .zip(avg_fps)
                .map(|(duration, fps)| (duration * fps).round().max(1.0) as u64);
            let size_bytes = fs::metadata(input_path).ok().map(|metadata| metadata.len());

            Ok(MediaInspection {
                ok: true,
                input_path: input_path.to_string(),
                tool_source: Some("native".into()),
                tool_command: None,
                tool_detail: Some(locale::native_video_detail(locale, &format_name)),
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
                is_static_image: false,
                can_convert_to_png: false,
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
    };

    match result {
        Ok(inspection) => inspection,
        Err(error) => inspection_error(
            input_path,
            Some("native".into()),
            None,
            Some(locale::native_video_detail(locale, &format_name)),
            "inspect-failed",
            error,
        ),
    }
}

#[cfg(not(target_os = "windows"))]
fn inspect_mp4_family_with_media_foundation(input_path: &str, locale: UiLocale) -> MediaInspection {
    inspect_video_with_ffmpeg(input_path, locale)
}

fn inspect_video_with_ffmpeg(input_path: &str, locale: UiLocale) -> MediaInspection {
    let tool = match resolve_tool("ffmpeg", locale) {
        Ok(tool) => tool,
        Err(error) => {
            return inspection_error(
                input_path,
                Some("missing".into()),
                None,
                None,
                "tool-unavailable",
                error,
            );
        }
    };

    let args = [
        "-hide_banner",
        "-i",
        input_path,
        "-map",
        "0:v:0",
        "-frames:v",
        "1",
        "-f",
        "null",
        "-",
    ];

    let mut command = Command::new(&tool.command);
    configure_child_process(&mut command);
    let output = match command.args(args).output() {
        Ok(output) => output,
        Err(error) => {
            return inspection_error(
                input_path,
                Some("sidecar".into()),
                Some(tool.command_display.clone()),
                tool.fallback_reason.clone(),
                "tool-unavailable",
                error.to_string(),
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
            let message = if output.status.success() {
                locale::no_usable_video_stream_error(locale)
            } else {
                String::from_utf8_lossy(&output.stderr).trim().to_string()
            };

            return inspection_error(
                input_path,
                Some("sidecar".into()),
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
        tool_source: Some("sidecar".into()),
        tool_command: Some(tool.command_display),
        tool_detail: tool.fallback_reason,
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
        is_static_image: false,
        can_convert_to_png: false,
        error_code: None,
        error_message: None,
    }
}

fn inspect_input_media_internal(input_path: &str, locale: UiLocale) -> MediaInspection {
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
        if matches!(
            read_png_animation_metadata(Path::new(input_path)),
            Ok(Some(_))
        ) {
            return inspect_apng_metadata_internal(input_path, locale);
        }

        return inspect_still_image_metadata_internal(input_path, locale);
    }

    if is_supported_static_image_extension(&extension) {
        return inspect_still_image_metadata_internal(input_path, locale);
    }

    if extension == "gif" {
        return inspect_gif_metadata_internal(input_path, locale);
    }

    if extension == "apng" {
        return inspect_apng_metadata_internal(input_path, locale);
    }

    if matches!(extension.as_str(), "mp4" | "m4v" | "mov") {
        return inspect_mp4_family_with_media_foundation(input_path, locale);
    }

    if is_supported_video_extension(&extension) {
        return inspect_video_with_ffmpeg(input_path, locale);
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
        Err(error) => ToolHealthReport {
            ready: false,
            checks: Vec::new(),
            summary: format!(
                "{} {} ({error})",
                locale::tool_health_check_failed_summary(locale),
                locale::internal_task_error_message(locale)
            ),
        },
    }
}

#[tauri::command]
async fn inspect_input_media(input_path: String, locale: Option<String>) -> MediaInspection {
    let locale = parse_ui_locale(locale.as_deref());
    let input_path_for_error = input_path.clone();

    match run_blocking_task(move || inspect_input_media_internal(&input_path, locale)).await {
        Ok(result) => result,
        Err(error) => MediaInspection {
            ok: false,
            input_path: input_path_for_error,
            tool_source: None,
            tool_command: None,
            tool_detail: None,
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
            is_static_image: false,
            can_convert_to_png: false,
            error_code: Some(INTERNAL_TASK_ERROR_CODE.into()),
            error_message: Some(format!(
                "{} ({error})",
                locale::internal_task_error_message(locale)
            )),
        },
    }
}

#[tauri::command]
async fn build_optimizer_plan(request: OptimizerPlanRequest) -> OptimizerPlanResponse {
    let locale = parse_ui_locale(request.locale.as_deref());
    let fallback_fit_mode = request.fit_mode.clone();

    match run_blocking_task(move || prepare_optimizer_plan(&request, locale)).await {
        Ok(result) => result,
        Err(error) => OptimizerPlanResponse {
            ok: false,
            fit_mode: fallback_fit_mode,
            selected_duration_seconds: None,
            recommended_max_duration_seconds: RECOMMENDED_MAX_DURATION_SECONDS,
            search_budget: MAX_SEARCH_BUDGET,
            warnings: Vec::new(),
            candidates: Vec::new(),
            error_code: Some(INTERNAL_TASK_ERROR_CODE.into()),
            error_message: Some(format!(
                "{} ({error})",
                locale::internal_task_error_message(locale)
            )),
        },
    }
}

#[tauri::command]
async fn convert_static_image_to_png(
    request: StaticImageConversionRequest,
) -> StaticImageConversionResult {
    let locale = parse_ui_locale(request.locale.as_deref());

    match run_blocking_task(move || {
        convert_static_image_to_png_internal(
            &request.input_path,
            request.output_directory.as_deref(),
            request.crop_region.as_ref(),
            locale,
        )
    })
    .await
    {
        Ok(result) => result,
        Err(error) => StaticImageConversionResult {
            ok: false,
            output_path: None,
            size_bytes: None,
            elapsed_ms: None,
            tool_source: None,
            tool_command: None,
            tool_detail: None,
            warnings: Vec::new(),
            error_code: Some(INTERNAL_TASK_ERROR_CODE.into()),
            error_message: Some(format!(
                "{} ({error})",
                locale::internal_task_error_message(locale)
            )),
        },
    }
}

fn run_optimizer_search_internal(
    request: OptimizerSearchRequest,
    locale: UiLocale,
) -> OptimizerSearchResponse {
    let optimizer_goal = normalized_optimizer_goal(
        request.optimizer_goal.as_deref(),
        request.preset_strategy.as_deref(),
    );
    let quality_frame_drop_interval =
        normalized_quality_frame_drop_interval(request.quality_frame_drop_interval);
    let resolved_timeline_frames =
        match resolve_timeline_frames(request.timeline_frames.as_ref(), request.base_frame_count) {
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
                                fit_mode: request.fit_mode.clone(),
                                selected_duration_seconds: None,
                                limit_bytes: DISCORD_MAX_STICKER_BYTES,
                                search_budget: MAX_SEARCH_BUDGET,
                                real_attempt_count: 0,
                                stop_reason: Some(error.into()),
                                selection_reason: "no_fit_found".into(),
                                summary: locale::plan_failed_message(locale),
                                warnings: Vec::new(),
                                attempts: Vec::new(),
                                winning_candidate_id: None,
                                closest_candidate_id: None,
                                best_output_path: None,
                                best_size_bytes: None,
                                best_within_limit: false,
                                error_code: Some(error.into()),
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
                    fit_mode: request.fit_mode.clone(),
                    selected_duration_seconds: None,
                    limit_bytes: DISCORD_MAX_STICKER_BYTES,
                    search_budget: MAX_SEARCH_BUDGET,
                    real_attempt_count: 0,
                    stop_reason: Some("no-frames-selected".into()),
                    selection_reason: "no_fit_found".into(),
                    summary: locale::plan_failed_message(locale),
                    warnings: Vec::new(),
                    attempts: Vec::new(),
                    winning_candidate_id: None,
                    closest_candidate_id: None,
                    best_output_path: None,
                    best_size_bytes: None,
                    best_within_limit: false,
                    error_code: Some("no-frames-selected".into()),
                    error_message: Some(locale::frame_selection_required_error(locale)),
                };
            }
            Err(_) => {
                return OptimizerSearchResponse {
                    ok: false,
                    fit_mode: request.fit_mode.clone(),
                    selected_duration_seconds: None,
                    limit_bytes: DISCORD_MAX_STICKER_BYTES,
                    search_budget: MAX_SEARCH_BUDGET,
                    real_attempt_count: 0,
                    stop_reason: Some("invalid-frame-selection".into()),
                    selection_reason: "no_fit_found".into(),
                    summary: locale::plan_failed_message(locale),
                    warnings: Vec::new(),
                    attempts: Vec::new(),
                    winning_candidate_id: None,
                    closest_candidate_id: None,
                    best_output_path: None,
                    best_size_bytes: None,
                    best_within_limit: false,
                    error_code: Some("invalid-frame-selection".into()),
                    error_message: Some(locale::invalid_frame_selection_error(locale)),
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
                                    fit_mode: request.fit_mode.clone(),
                                    selected_duration_seconds: None,
                                    limit_bytes: DISCORD_MAX_STICKER_BYTES,
                                    search_budget: MAX_SEARCH_BUDGET,
                                    real_attempt_count: 0,
                                    stop_reason: Some(error.into()),
                                    selection_reason: "no_fit_found".into(),
                                    summary: locale::plan_failed_message(locale),
                                    warnings: Vec::new(),
                                    attempts: Vec::new(),
                                    winning_candidate_id: None,
                                    closest_candidate_id: None,
                                    best_output_path: None,
                                    best_size_bytes: None,
                                    best_within_limit: false,
                                    error_code: Some(error.into()),
                                    error_message: Some(locale::frame_selection_required_error(
                                        locale,
                                    )),
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
                        fit_mode: request.fit_mode.clone(),
                        selected_duration_seconds: None,
                        limit_bytes: DISCORD_MAX_STICKER_BYTES,
                        search_budget: MAX_SEARCH_BUDGET,
                        real_attempt_count: 0,
                        stop_reason: Some("no-frames-selected".into()),
                        selection_reason: "no_fit_found".into(),
                        summary: locale::plan_failed_message(locale),
                        warnings: Vec::new(),
                        attempts: Vec::new(),
                        winning_candidate_id: None,
                        closest_candidate_id: None,
                        best_output_path: None,
                        best_size_bytes: None,
                        best_within_limit: false,
                        error_code: Some("no-frames-selected".into()),
                        error_message: Some(locale::frame_selection_required_error(locale)),
                    };
                }
                Err(_) => {
                    return OptimizerSearchResponse {
                        ok: false,
                        fit_mode: request.fit_mode.clone(),
                        selected_duration_seconds: None,
                        limit_bytes: DISCORD_MAX_STICKER_BYTES,
                        search_budget: MAX_SEARCH_BUDGET,
                        real_attempt_count: 0,
                        stop_reason: Some("invalid-frame-selection".into()),
                        selection_reason: "no_fit_found".into(),
                        summary: locale::plan_failed_message(locale),
                        warnings: Vec::new(),
                        attempts: Vec::new(),
                        winning_candidate_id: None,
                        closest_candidate_id: None,
                        best_output_path: None,
                        best_size_bytes: None,
                        best_within_limit: false,
                        error_code: Some("invalid-frame-selection".into()),
                        error_message: Some(locale::invalid_frame_selection_error(locale)),
                    };
                }
            },
        )
    } else {
        None
    };

    let plan = prepare_optimizer_plan(
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
            error_code: plan.error_code,
            error_message: plan.error_message,
        };
    }

    let mut attempts = Vec::new();
    let mut best_within_limit_output: Option<SelectedEncodeOutput> = None;
    let mut smallest_oversize_output: Option<SelectedEncodeOutput> = None;
    let mut stopped_after_best_within_limit = false;
    for candidate in &plan.candidates {
        if remaining_candidate_cannot_beat_within_limit(
            best_within_limit_output.as_ref(),
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
            encode_candidate_internal(
                &request.input_path,
                request.output_directory.as_deref(),
                locale,
                request.crop_region.as_ref(),
                request.input_width,
                request.input_height,
                candidate,
                None,
                Some(timeline_frames),
            )
        } else {
            encode_candidate_internal(
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
            )
        };

        match encode_result {
            Ok(result) => {
                let within_limit = result.size_bytes <= DISCORD_MAX_STICKER_BYTES;
                let output_path = result.output_path.to_string_lossy().into_owned();
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
                    output_path: Some(output_path.clone()),
                    size_bytes: Some(result.size_bytes),
                    elapsed_ms: Some(result.elapsed_ms),
                    tool_source: Some(result.tool_source.clone()),
                    tool_command: result.tool_command.clone(),
                    tool_detail: result.tool_detail.clone(),
                    warnings: Vec::new(),
                    error_code: None,
                    error_message: None,
                };
                attempts.push(attempt);
                let contender = SelectedEncodeOutput {
                    candidate_id: candidate.id.clone(),
                    rank: candidate.rank,
                    duration_seconds: candidate.duration_seconds,
                    size_bytes: result.size_bytes,
                    source_similarity_score: candidate.source_similarity_score,
                    output_path: output_path.clone(),
                };

                if within_limit {
                    let replace_current = best_within_limit_output
                        .as_ref()
                        .map(|current| is_better_within_limit_candidate(current, &contender))
                        .unwrap_or(true);

                    if replace_current {
                        if let Some(previous) = best_within_limit_output.replace(contender) {
                            let _ = fs::remove_file(&previous.output_path);
                            clear_attempt_output_path(&mut attempts, &previous.candidate_id);
                        }
                    } else {
                        let _ = fs::remove_file(&output_path);
                        clear_attempt_output_path(&mut attempts, &candidate.id);
                    }
                } else {
                    let replace_current = smallest_oversize_output
                        .as_ref()
                        .map(|current| is_better_oversize_candidate(current, &contender))
                        .unwrap_or(true);

                    if replace_current {
                        if let Some(previous) = smallest_oversize_output.replace(contender) {
                            let _ = fs::remove_file(&previous.output_path);
                            clear_attempt_output_path(&mut attempts, &previous.candidate_id);
                        }
                    } else {
                        let _ = fs::remove_file(&output_path);
                        clear_attempt_output_path(&mut attempts, &candidate.id);
                    }
                }
            }
            Err(error) => attempts.push(SearchAttemptResult {
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
                within_limit: false,
                output_path: None,
                size_bytes: None,
                elapsed_ms: None,
                tool_source: None,
                tool_command: None,
                tool_detail: None,
                warnings: Vec::new(),
                error_code: Some("invoke-failed".into()),
                error_message: Some(error),
            }),
        }
    }

    if best_within_limit_output.is_some() {
        if let Some(oversize_output) = smallest_oversize_output.take() {
            let _ = fs::remove_file(&oversize_output.output_path);
            clear_attempt_output_path(&mut attempts, &oversize_output.candidate_id);
        }
    }

    let stop_reason = if stopped_after_best_within_limit {
        "found-best-ranked-within-limit"
    } else if attempts.iter().any(|attempt| attempt.output_path.is_some()) {
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
    let selected_output = best_within_limit_output
        .as_ref()
        .or(smallest_oversize_output.as_ref());
    let winning_candidate_id = best_within_limit_output
        .as_ref()
        .map(|output| output.candidate_id.clone());
    let closest_candidate_id = selected_output.map(|output| output.candidate_id.clone());
    let best_output_path = selected_output.map(|output| output.output_path.clone());
    let best_size_bytes = selected_output.map(|output| output.size_bytes);
    let best_within_limit = best_within_limit_output.is_some();
    let summary = locale::optimizer_search_summary(locale, selection_reason);
    let final_duration_seconds = selected_output
        .map(|output| output.duration_seconds)
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
        error_code: None,
        error_message: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

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
            output_path: format!("C:/tmp/{candidate_id}.png"),
        }
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
                duration_seconds: 0.1,
            })
            .collect::<Vec<_>>();
        write_native_apng(Path::new(&input_path), &frames, "standard")
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
                write_native_png(Path::new(&output), &image)
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
                duration_seconds: 0.12,
            },
            StickerFrame {
                pixels: RgbaImage::from_pixel(48, 48, named_rgba("green")),
                duration_seconds: 0.24,
            },
            StickerFrame {
                pixels: RgbaImage::from_pixel(48, 48, named_rgba("blue")),
                duration_seconds: 0.36,
            },
        ];
        write_native_apng(Path::new(&output_path), &frames, "standard")
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
    fn build_filter_graph_retimes_selected_frames_before_fps_resampling() {
        let graph = build_filter_graph(
            "contain",
            12,
            1.0,
            Some(320),
            Some(320),
            None,
            Some(&[0, 6]),
        );

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

    fn png_chunk(chunk_type: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
        bytes.extend_from_slice(chunk_type);
        bytes.extend_from_slice(data);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes
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

        let mut first_frame = [0u8; 26];
        first_frame[20..22].copy_from_slice(&12u16.to_be_bytes());
        first_frame[22..24].copy_from_slice(&100u16.to_be_bytes());
        bytes.extend_from_slice(&png_chunk(b"fcTL", &first_frame));

        let mut second_frame = [0u8; 26];
        second_frame[20..22].copy_from_slice(&24u16.to_be_bytes());
        second_frame[22..24].copy_from_slice(&100u16.to_be_bytes());
        bytes.extend_from_slice(&png_chunk(b"fcTL", &second_frame));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
        fs::write(&input_path, bytes).expect("metadata-only png should be written");

        let metadata = read_png_animation_metadata(&input_path)
            .expect("metadata parser should read valid chunks")
            .expect("acTL should mark this as APNG metadata");

        assert_eq!(metadata.width, 48);
        assert_eq!(metadata.height, 32);
        assert_eq!(metadata.frame_count, Some(2));
        assert_eq!(metadata.frame_durations.len(), 2);
        assert!(approx_eq(metadata.frame_durations[0], 0.12));
        assert!(approx_eq(metadata.frame_durations[1], 0.24));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn write_native_apng_preserves_sparse_changed_regions() {
        let test_dir = TestDir::new("apng-sparse-delta");
        let output_path = test_dir.path.join("sparse-delta.png");
        let expected_frames = vec![
            StickerFrame {
                pixels: sparse_sprite_frame(16, 16, Some((2, 2))),
                duration_seconds: 0.10,
            },
            StickerFrame {
                pixels: sparse_sprite_frame(16, 16, Some((9, 2))),
                duration_seconds: 0.12,
            },
            StickerFrame {
                pixels: sparse_sprite_frame(16, 16, None),
                duration_seconds: 0.14,
            },
            StickerFrame {
                pixels: sparse_sprite_frame(16, 16, Some((4, 10))),
                duration_seconds: 0.16,
            },
        ];

        write_native_apng(&output_path, &expected_frames, "compact")
            .expect("sparse APNG should be written");

        let decoded_frames = decode_apng_animation_frames(output_path.to_string_lossy().as_ref())
            .expect("sparse APNG should decode");

        assert_eq!(decoded_frames.len(), expected_frames.len());
        for (decoded, expected) in decoded_frames.iter().zip(expected_frames.iter()) {
            assert_eq!(decoded.pixels.as_raw(), expected.pixels.as_raw());
            assert!(approx_eq(
                decoded.duration_seconds,
                expected.duration_seconds
            ));
        }
    }

    #[test]
    fn build_filter_graph_uses_cropped_dimensions_for_max_320_scaling() {
        let graph = build_filter_graph(
            "contain",
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
        assert!(graph.contains("scale=200:100:force_original_aspect_ratio=decrease"));
        assert!(graph.contains("pad=200:100:(ow-iw)/2:(oh-ih)/2:color=black@0.0"));
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
        )
        .expect("encoding should succeed");
        assert_eq!(result.tool_source, "native");
        assert_eq!(result.tool_command, None);

        let output_path = result.output_path.to_string_lossy().into_owned();
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
                    output_directory: Some(output_dir.to_string_lossy().into_owned()),
                    locale: Some("en".into()),
                    source_duration_seconds: inspection.duration_seconds,
                    input_width: inspection.width,
                    input_height: inspection.height,
                    avg_fps: inspection.avg_fps,
                    fit_mode: "contain".into(),
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
    fn encode_candidate_internal_respects_timeline_frame_order_and_durations() {
        let test_dir = TestDir::new("timeline-encode");
        let input_path = create_three_frame_animation(&test_dir);
        let candidate = build_test_candidate("timeline-sequence", 8);
        let timeline_frames = vec![
            ResolvedTimelineFrame {
                source_frame_index: 2,
                duration_seconds: 0.12,
            },
            ResolvedTimelineFrame {
                source_frame_index: 0,
                duration_seconds: 0.24,
            },
            ResolvedTimelineFrame {
                source_frame_index: 2,
                duration_seconds: 0.36,
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
        )
        .expect("timeline-based encoding should succeed");
        assert_eq!(result.tool_source, "native");
        assert_eq!(result.tool_command, None);

        let inspection = inspect_input_media_internal(
            result.output_path.to_string_lossy().as_ref(),
            UiLocale::En,
        );
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

        let response = run_optimizer_search_internal(
            OptimizerSearchRequest {
                input_path,
                output_directory: Some(test_dir.path.to_string_lossy().into_owned()),
                locale: Some("en".into()),
                source_duration_seconds: Some(0.72),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(7.0),
                fit_mode: "contain".into(),
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
        let response = run_optimizer_search_internal(
            OptimizerSearchRequest {
                input_path: "ignored.png".into(),
                output_directory: None,
                locale: Some("en".into()),
                source_duration_seconds: Some(1.0),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(7.0),
                fit_mode: "contain".into(),
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
        assert_eq!(response.error_code.as_deref(), Some("no-frames-selected"));
        assert_eq!(
            response.error_message.as_deref(),
            Some("Select at least one frame before exporting.")
        );
    }

    #[test]
    fn run_optimizer_search_rejects_out_of_range_frame_selection() {
        let response = run_optimizer_search_internal(
            OptimizerSearchRequest {
                input_path: "ignored.png".into(),
                output_directory: None,
                locale: Some("en".into()),
                source_duration_seconds: Some(1.0),
                input_width: Some(48),
                input_height: Some(48),
                avg_fps: Some(7.0),
                fit_mode: "contain".into(),
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
        assert_eq!(
            response.error_code.as_deref(),
            Some("invalid-frame-selection")
        );
        assert_eq!(
            response.error_message.as_deref(),
            Some("The selected frame list does not match the available frame markers.")
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
                fit_mode: "contain".into(),
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
                fit_mode: "contain".into(),
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
                && candidate.duration_seconds <= DISCORD_MAX_DURATION_SECONDS));
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
                fit_mode: "contain".into(),
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
                fit_mode: "contain".into(),
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
        assert_eq!(response.error_code.as_deref(), Some("duration-too-long"));
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
                fit_mode: "contain".into(),
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
        assert!(approx_eq(timeline_frames[0].duration_seconds, 0.12));
        assert_eq!(timeline_frames[1].source_frame_index, 0);
        assert!(approx_eq(timeline_frames[1].duration_seconds, 0.24));
        assert_eq!(timeline_frames[2].source_frame_index, 2);
        assert!(approx_eq(timeline_frames[2].duration_seconds, 0.36));
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
                fit_mode: "contain".into(),
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
async fn run_optimizer_search(request: OptimizerSearchRequest) -> OptimizerSearchResponse {
    let locale = parse_ui_locale(request.locale.as_deref());
    let fallback_fit_mode = request.fit_mode.clone();

    match run_blocking_task(move || run_optimizer_search_internal(request, locale)).await {
        Ok(result) => result,
        Err(error) => OptimizerSearchResponse {
            ok: false,
            fit_mode: fallback_fit_mode,
            selected_duration_seconds: None,
            limit_bytes: DISCORD_MAX_STICKER_BYTES,
            search_budget: MAX_SEARCH_BUDGET,
            real_attempt_count: 0,
            stop_reason: Some(INTERNAL_TASK_ERROR_CODE.into()),
            selection_reason: "no_fit_found".into(),
            summary: locale::internal_task_error_message(locale),
            warnings: Vec::new(),
            attempts: Vec::new(),
            winning_candidate_id: None,
            closest_candidate_id: None,
            best_output_path: None,
            best_size_bytes: None,
            best_within_limit: false,
            error_code: Some(INTERNAL_TASK_ERROR_CODE.into()),
            error_message: Some(format!(
                "{} ({error})",
                locale::internal_task_error_message(locale)
            )),
        },
    }
}

#[tauri::command]
async fn extract_frame_preview(
    input_path: String,
    source_frame_id: u32,
    locale: Option<String>,
) -> FramePreviewResponse {
    let locale = parse_ui_locale(locale.as_deref());

    match run_blocking_task(move || {
        extract_frame_preview_internal(&input_path, source_frame_id, locale)
    })
    .await
    {
        Ok(response) => response,
        Err(error) => FramePreviewResponse {
            ok: false,
            data_url: None,
            width: None,
            height: None,
            error_code: Some(INTERNAL_TASK_ERROR_CODE.into()),
            error_message: Some(format!(
                "{} ({error})",
                locale::internal_task_error_message(locale)
            )),
        },
    }
}

#[tauri::command]
async fn extract_frame_previews(
    input_path: String,
    source_frame_ids: Vec<u32>,
    locale: Option<String>,
) -> FramePreviewsResponse {
    let locale = parse_ui_locale(locale.as_deref());

    match run_blocking_task(move || {
        extract_frame_previews_internal(&input_path, &source_frame_ids, locale)
    })
    .await
    {
        Ok(response) => response,
        Err(error) => FramePreviewsResponse {
            ok: false,
            previews: Vec::new(),
            error_code: Some(INTERNAL_TASK_ERROR_CODE.into()),
            error_message: Some(format!(
                "{} ({error})",
                locale::internal_task_error_message(locale)
            )),
        },
    }
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
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            check_media_tools,
            inspect_input_media,
            build_optimizer_plan,
            run_optimizer_search,
            convert_static_image_to_png,
            extract_frame_preview,
            extract_frame_previews,
            open_folder_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running StickerFit");
}
