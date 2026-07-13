#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UiLocale {
    En,
    Ko,
}

pub(crate) fn parse_ui_locale(raw: Option<&str>) -> UiLocale {
    match raw.unwrap_or_default().trim().to_ascii_lowercase().as_str() {
        locale if locale.starts_with("ko") => UiLocale::Ko,
        _ => UiLocale::En,
    }
}

fn tr(locale: UiLocale, en: &'static str, ko: &'static str) -> String {
    match locale {
        UiLocale::En => en.into(),
        UiLocale::Ko => ko.into(),
    }
}

pub(crate) fn internal_task_error_message(locale: UiLocale) -> String {
    tr(
        locale,
        "A StickerFit background task did not finish correctly.",
        "StickerFit 백그라운드 작업이 정상적으로 끝나지 않았습니다.",
    )
}

pub(crate) fn source_output_directory_error(locale: UiLocale) -> String {
    tr(
        locale,
        "Could not derive an output folder from the source file path.",
        "원본 파일 경로에서 출력 폴더를 유추할 수 없습니다.",
    )
}

pub(crate) fn output_path_not_directory_error(locale: UiLocale) -> String {
    tr(
        locale,
        "The selected output path already exists and is not a directory.",
        "선택한 출력 경로가 이미 존재하지만 폴더가 아닙니다.",
    )
}

pub(crate) fn invalid_crop_number_error(locale: UiLocale) -> String {
    tr(
        locale,
        "Crop selection values must be valid numbers.",
        "크롭 선택 값은 올바른 숫자여야 합니다.",
    )
}

pub(crate) fn crop_needs_source_width_error(locale: UiLocale) -> String {
    tr(
        locale,
        "Crop selection needs the source width from media inspection.",
        "크롭 선택에는 원본 너비 정보가 필요합니다.",
    )
}

pub(crate) fn crop_needs_source_height_error(locale: UiLocale) -> String {
    tr(
        locale,
        "Crop selection needs the source height from media inspection.",
        "크롭 선택에는 원본 높이 정보가 필요합니다.",
    )
}

pub(crate) fn legacy_fit_mode_fallback_warning(locale: UiLocale) -> String {
    tr(
        locale,
        "A legacy or unknown fit mode was ignored; contain is now always used.",
        "레거시 또는 알 수 없는 맞춤 모드는 무시되며, 이제 항상 contain으로 처리합니다.",
    )
}

pub(crate) fn media_pipeline_diagnostic(locale: UiLocale, error_code: &str) -> String {
    match error_code {
        "cancelled" => tr(
            locale,
            "The media operation was cancelled.",
            "미디어 작업이 취소되었습니다.",
        ),
        "timed-out" => tr(
            locale,
            "The media operation timed out.",
            "미디어 작업 시간이 초과되었습니다.",
        ),
        "operation-conflict" => tr(
            locale,
            "Another media operation is already using this resource.",
            "다른 미디어 작업이 이 자원을 사용 중입니다.",
        ),
        "invalid-request" => tr(
            locale,
            "The media request is no longer valid.",
            "미디어 요청이 더 이상 유효하지 않습니다.",
        ),
        "source-changed" => tr(
            locale,
            "The source file changed. Inspect it again before continuing.",
            "원본 파일이 변경되었습니다. 계속하기 전에 다시 검사하세요.",
        ),
        "media-input-too-large"
        | "media-dimensions-too-large"
        | "media-frame-limit"
        | "decoded-byte-limit"
        | "png-chunk-limit" => tr(
            locale,
            "The media exceeds StickerFit's safe processing limits.",
            "미디어가 StickerFit의 안전한 처리 한도를 초과했습니다.",
        ),
        "malformed-media" | "malformed-process-output" => tr(
            locale,
            "The media data is malformed or incomplete.",
            "미디어 데이터가 손상되었거나 불완전합니다.",
        ),
        "tool-missing" => tr(
            locale,
            "A required media tool is unavailable.",
            "필요한 미디어 도구를 사용할 수 없습니다.",
        ),
        "process-failed" => tr(
            locale,
            "A media tool could not complete the operation.",
            "미디어 도구가 작업을 완료하지 못했습니다.",
        ),
        "output-conflict" => tr(
            locale,
            "The selected output already exists.",
            "선택한 출력이 이미 존재합니다.",
        ),
        _ => internal_task_error_message(locale),
    }
}

pub(crate) fn selected_duration_limit_error(locale: UiLocale) -> String {
    tr(
        locale,
        "Selected duration exceeds Discord's 5-second limit.",
        "선택한 길이가 디스코드의 5초 제한을 초과합니다.",
    )
}

pub(crate) fn frame_selection_required_error(locale: UiLocale) -> String {
    tr(
        locale,
        "Select at least one frame before exporting.",
        "내보내기 전에 최소 한 개의 프레임을 선택하세요.",
    )
}

pub(crate) fn invalid_frame_selection_error(locale: UiLocale) -> String {
    tr(
        locale,
        "The selected frame list does not match the available frame markers.",
        "선택한 프레임 목록이 현재 프레임 표식 범위와 맞지 않습니다.",
    )
}

pub(crate) fn invalid_frame_duration_error(locale: UiLocale) -> String {
    tr(
        locale,
        "One or more frame durations are invalid.",
        "하나 이상의 프레임 길이가 올바르지 않습니다.",
    )
}

pub(crate) fn recommended_duration_warning(locale: UiLocale) -> String {
    tr(
        locale,
        "This selection is longer than the current recommended MVP duration.",
        "이 선택 구간은 현재 MVP 권장 길이보다 깁니다.",
    )
}

pub(crate) fn crop_applied_before_scale_warning(locale: UiLocale) -> String {
    tr(
        locale,
        "The selected crop area will be applied before output scale is evaluated.",
        "선택한 크롭 영역을 먼저 적용한 뒤 출력 스케일을 계산합니다.",
    )
}

pub(crate) fn unsupported_still_image_error(locale: UiLocale) -> String {
    tr(
        locale,
        "This source is not a supported still image.",
        "이 소스는 지원되는 정적 이미지가 아닙니다.",
    )
}

pub(crate) fn invalid_png_header_error(locale: UiLocale) -> String {
    tr(
        locale,
        "The PNG header could not be read.",
        "PNG 헤더를 읽지 못했습니다.",
    )
}

pub(crate) fn invalid_apng_error(locale: UiLocale) -> String {
    tr(
        locale,
        "The APNG animation chunks could not be read.",
        "APNG 애니메이션 청크를 읽지 못했습니다.",
    )
}

pub(crate) fn no_usable_video_stream_error(locale: UiLocale) -> String {
    tr(
        locale,
        "No usable video stream was found in the selected file.",
        "선택한 파일에서 사용할 수 있는 비디오 스트림을 찾지 못했습니다.",
    )
}

pub(crate) fn missing_sidecar_reason(
    locale: UiLocale,
    tool: &str,
    expected: &str,
    attempted_paths: &[String],
) -> String {
    if attempted_paths.is_empty() {
        match locale {
            UiLocale::En => format!(
                "No deterministic bundled sidecar locations were available for {tool} ({expected})."
            ),
            UiLocale::Ko => {
                format!("{tool}용 고정된 번들 사이드카 경로를 찾지 못했습니다 ({expected}).")
            }
        }
    } else {
        match locale {
            UiLocale::En => format!(
                "Bundled sidecar {expected} was not found for {tool}. Checked: {}.",
                attempted_paths.join(", ")
            ),
            UiLocale::Ko => format!(
                "{tool}용 번들 사이드카 {expected}를 찾지 못했습니다. 확인한 경로: {}.",
                attempted_paths.join(", ")
            ),
        }
    }
}

#[allow(dead_code)]
pub(crate) fn command_non_zero_exit_message(command_display: &str, locale: UiLocale) -> String {
    match locale {
        UiLocale::En => format!("{command_display} returned a non-zero exit code"),
        UiLocale::Ko => format!("{command_display} 명령이 0이 아닌 종료 코드를 반환했습니다."),
    }
}

pub(crate) fn tool_check_sidecar_ok_detail(
    locale: UiLocale,
    tool: &str,
    command_display: &str,
) -> String {
    match locale {
        UiLocale::En => {
            format!("Bundled sidecar responded successfully for {tool} at {command_display}.")
        }
        UiLocale::Ko => format!("{tool} 번들 사이드카가 정상 응답했습니다: {command_display}."),
    }
}

pub(crate) fn native_image_detail(locale: UiLocale) -> String {
    tr(
        locale,
        "Image metadata was parsed locally without ffmpeg.",
        "이미지 메타데이터를 ffmpeg 없이 로컬에서 읽었습니다.",
    )
}

pub(crate) fn native_animation_detail(locale: UiLocale, format_name: &str) -> String {
    match locale {
        UiLocale::En => format!("{format_name} animation metadata was parsed locally."),
        UiLocale::Ko => format!("{format_name} 애니메이션 메타데이터를 로컬에서 읽었습니다."),
    }
}

pub(crate) fn native_video_detail(locale: UiLocale, format_name: &str) -> String {
    match locale {
        UiLocale::En => {
            format!("{format_name} video metadata was read through Windows Media Foundation.")
        }
        UiLocale::Ko => {
            format!("{format_name} 비디오 메타데이터를 Windows Media Foundation으로 읽었습니다.")
        }
    }
}

pub(crate) fn media_foundation_fallback_warning(locale: UiLocale) -> String {
    tr(
        locale,
        "Windows Media Foundation could not inspect this file, so StickerFit used the bundled FFmpeg decoder.",
        "Windows Media Foundation에서 이 파일을 검사하지 못해 StickerFit이 번들 FFmpeg 디코더를 사용했습니다.",
    )
}

pub(crate) fn media_foundation_fallback_attempt_detail(locale: UiLocale) -> String {
    tr(
        locale,
        "Windows Media Foundation could not inspect this file, so StickerFit attempted inspection with the bundled FFmpeg decoder.",
        "Windows Media Foundation에서 이 파일을 검사하지 못해 StickerFit이 번들 FFmpeg 디코더로 검사를 시도했습니다.",
    )
}

pub(crate) fn native_png_encode_detail(locale: UiLocale) -> String {
    tr(
        locale,
        "StickerFit wrote the PNG output without ffmpeg.",
        "StickerFit가 ffmpeg 없이 PNG 출력을 기록했습니다.",
    )
}

pub(crate) fn native_apng_encode_detail(locale: UiLocale) -> String {
    tr(
        locale,
        "StickerFit wrote the APNG output without ffmpeg.",
        "StickerFit가 ffmpeg 없이 APNG 출력을 기록했습니다.",
    )
}

pub(crate) fn tool_health_summary(locale: UiLocale, ready: bool) -> String {
    if ready {
        tr(
            locale,
            "Media tools are available for StickerFit.",
            "StickerFit에 필요한 미디어 도구를 사용할 수 있습니다.",
        )
    } else {
        tr(
            locale,
            "Bundled ffmpeg is unavailable.",
            "번들 ffmpeg를 사용할 수 없습니다.",
        )
    }
}

pub(crate) fn tool_health_check_failed_summary(locale: UiLocale) -> String {
    tr(
        locale,
        "Media tool check failed.",
        "미디어 도구 확인에 실패했습니다.",
    )
}

pub(crate) fn plan_failed_message(locale: UiLocale) -> String {
    tr(locale, "Plan failed.", "계획 생성에 실패했습니다.")
}

pub(crate) fn preset_label(locale: UiLocale, preset: &str) -> &'static str {
    match locale {
        UiLocale::En => match preset {
            "standard" => "Standard",
            "compact" => "Compact",
            "compactPlus" => "Compact+",
            _ => "Custom",
        },
        UiLocale::Ko => match preset {
            "standard" => "표준",
            "compact" => "압축",
            "compactPlus" => "압축+",
            _ => "사용자 지정",
        },
    }
}

pub(crate) fn candidate_summary(
    locale: UiLocale,
    fps: u32,
    content_scale: f64,
    preset: &str,
    duration_seconds: f64,
) -> String {
    match locale {
        UiLocale::En => format!(
            "{} FPS, {:.0}% content scale, {} preset for a {:.2}s clip.",
            fps,
            content_scale * 100.0,
            preset_label(locale, preset),
            duration_seconds
        ),
        UiLocale::Ko => format!(
            "{} FPS, 콘텐츠 스케일 {:.0}%, {:.2}초 클립용 {} 프리셋.",
            fps,
            content_scale * 100.0,
            duration_seconds,
            preset_label(locale, preset)
        ),
    }
}

pub(crate) fn optimizer_search_summary(locale: UiLocale, selection_reason: &str) -> String {
    match selection_reason {
        "best_within_limit" => tr(
            locale,
            "Chose the closest-to-source candidate that still fits the Discord limit.",
            "디스코드 제한 안에서 원본 특성에 가장 가까운 후보를 선택했습니다.",
        ),
        "smallest_oversize" => tr(
            locale,
            "No candidate fit the limit, so the smallest successful output is shown.",
            "제한을 만족한 후보가 없어, 성공한 출력 중 가장 작은 파일을 표시합니다.",
        ),
        _ => tr(
            locale,
            "No successful outputs were produced.",
            "성공적으로 만들어진 출력이 없습니다.",
        ),
    }
}

pub(crate) fn unsupported_platform_error(locale: UiLocale) -> String {
    tr(
        locale,
        "Unsupported platform",
        "지원하지 않는 플랫폼입니다.",
    )
}
