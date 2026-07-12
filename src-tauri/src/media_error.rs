use serde::Serialize;
use std::fmt;
use std::ops::Deref;
use std::path::PathBuf;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MediaOperationErrorCode {
    Cancelled,
    TimedOut,
    OperationConflict,
    InvalidRequest,
    SourceChanged,
    MediaInputTooLarge,
    MediaDimensionsTooLarge,
    MediaFrameLimit,
    DecodedByteLimit,
    PngChunkLimit,
    MalformedMedia,
    MalformedProcessOutput,
    ToolMissing,
    ProcessFailed,
    OutputConflict,
    InternalTaskFailed,
}

impl MediaOperationErrorCode {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed-out",
            Self::OperationConflict => "operation-conflict",
            Self::InvalidRequest => "invalid-request",
            Self::SourceChanged => "source-changed",
            Self::MediaInputTooLarge => "media-input-too-large",
            Self::MediaDimensionsTooLarge => "media-dimensions-too-large",
            Self::MediaFrameLimit => "media-frame-limit",
            Self::DecodedByteLimit => "decoded-byte-limit",
            Self::PngChunkLimit => "png-chunk-limit",
            Self::MalformedMedia => "malformed-media",
            Self::MalformedProcessOutput => "malformed-process-output",
            Self::ToolMissing => "tool-missing",
            Self::ProcessFailed => "process-failed",
            Self::OutputConflict => "output-conflict",
            Self::InternalTaskFailed => "internal-task-failed",
        }
    }

    pub(crate) fn from_code(value: &str) -> Option<Self> {
        match value {
            "cancelled" => Some(Self::Cancelled),
            "timed-out" => Some(Self::TimedOut),
            "operation-conflict" => Some(Self::OperationConflict),
            "invalid-request" => Some(Self::InvalidRequest),
            "source-changed" => Some(Self::SourceChanged),
            "media-input-too-large" => Some(Self::MediaInputTooLarge),
            "media-dimensions-too-large" => Some(Self::MediaDimensionsTooLarge),
            "media-frame-limit" => Some(Self::MediaFrameLimit),
            "decoded-byte-limit" => Some(Self::DecodedByteLimit),
            "png-chunk-limit" => Some(Self::PngChunkLimit),
            "malformed-media" => Some(Self::MalformedMedia),
            "malformed-process-output" => Some(Self::MalformedProcessOutput),
            "tool-missing" => Some(Self::ToolMissing),
            "process-failed" => Some(Self::ProcessFailed),
            "output-conflict" => Some(Self::OutputConflict),
            "internal-task-failed" => Some(Self::InternalTaskFailed),
            _ => None,
        }
    }

    pub(crate) fn for_invalid_request_reason(reason: &str) -> Self {
        if MediaOperationReasonCode::from_code(reason).is_some() {
            Self::InvalidRequest
        } else {
            Self::InternalTaskFailed
        }
    }
}

impl AsRef<str> for MediaOperationErrorCode {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Deref for MediaOperationErrorCode {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MediaOperationReasonCode {
    NoFramesSelected,
    InvalidFrameSelection,
    InvalidFrameDuration,
    DurationTooLong,
    InvalidCrop,
    InvalidOutputDirectory,
    UnsupportedSourceFormat,
    UnsupportedFramePreview,
    FramePreviewDecodeFailed,
    FramePreviewEncodeFailed,
    DecodeFailed,
    EncodeFailed,
    MissingOutput,
    PlanInvalid,
    InvokeFailed,
}

impl MediaOperationReasonCode {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::NoFramesSelected => "no-frames-selected",
            Self::InvalidFrameSelection => "invalid-frame-selection",
            Self::InvalidFrameDuration => "invalid-frame-duration",
            Self::DurationTooLong => "duration-too-long",
            Self::InvalidCrop => "invalid-crop",
            Self::InvalidOutputDirectory => "invalid-output-directory",
            Self::UnsupportedSourceFormat => "unsupported-source-format",
            Self::UnsupportedFramePreview => "unsupported-frame-preview",
            Self::FramePreviewDecodeFailed => "frame-preview-decode-failed",
            Self::FramePreviewEncodeFailed => "frame-preview-encode-failed",
            Self::DecodeFailed => "decode-failed",
            Self::EncodeFailed => "encode-failed",
            Self::MissingOutput => "missing-output",
            Self::PlanInvalid => "plan-invalid",
            Self::InvokeFailed => "invoke-failed",
        }
    }

    pub(crate) fn from_code(value: &str) -> Option<Self> {
        match value {
            "no-frames-selected" => Some(Self::NoFramesSelected),
            "invalid-frame-selection" => Some(Self::InvalidFrameSelection),
            "invalid-frame-duration" => Some(Self::InvalidFrameDuration),
            "duration-too-long" => Some(Self::DurationTooLong),
            "invalid-crop" => Some(Self::InvalidCrop),
            "invalid-output-directory" => Some(Self::InvalidOutputDirectory),
            "unsupported-source-format" => Some(Self::UnsupportedSourceFormat),
            "unsupported-frame-preview" => Some(Self::UnsupportedFramePreview),
            "frame-preview-decode-failed" => Some(Self::FramePreviewDecodeFailed),
            "frame-preview-encode-failed" => Some(Self::FramePreviewEncodeFailed),
            "decode-failed" => Some(Self::DecodeFailed),
            "encode-failed" => Some(Self::EncodeFailed),
            "missing-output" => Some(Self::MissingOutput),
            "plan-invalid" => Some(Self::PlanInvalid),
            "invoke-failed" => Some(Self::InvokeFailed),
            _ => None,
        }
    }
}

impl AsRef<str> for MediaOperationReasonCode {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Deref for MediaOperationReasonCode {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PipelineError {
    Cancelled,
    TimedOut {
        stage: &'static str,
    },
    OperationConflict {
        operation_id: String,
    },
    InvalidRequest {
        reason: &'static str,
    },
    InvalidRequestWithoutReason,
    SourceChanged,
    LimitExceeded {
        resource: &'static str,
        limit: u64,
        actual: u64,
    },
    MalformedInput {
        format: &'static str,
        reason: String,
    },
    MalformedProcessOutput {
        reason: String,
    },
    ToolMissing {
        tool: &'static str,
    },
    ProcessFailed {
        command: String,
        exit_code: Option<i32>,
        stderr: String,
    },
    OutputConflict {
        path: PathBuf,
    },
    Io {
        operation: &'static str,
        message: String,
    },
}

impl PipelineError {
    pub(crate) fn error_code(&self) -> MediaOperationErrorCode {
        match self {
            Self::Cancelled => MediaOperationErrorCode::Cancelled,
            Self::TimedOut { .. } => MediaOperationErrorCode::TimedOut,
            Self::OperationConflict { .. } => MediaOperationErrorCode::OperationConflict,
            Self::InvalidRequest { reason } => {
                MediaOperationErrorCode::for_invalid_request_reason(reason)
            }
            Self::InvalidRequestWithoutReason => MediaOperationErrorCode::InvalidRequest,
            Self::SourceChanged => MediaOperationErrorCode::SourceChanged,
            Self::LimitExceeded { resource, .. } => match *resource {
                "input-bytes" => MediaOperationErrorCode::MediaInputTooLarge,
                "image-dimensions" | "image-pixels" => {
                    MediaOperationErrorCode::MediaDimensionsTooLarge
                }
                "frame-count" => MediaOperationErrorCode::MediaFrameLimit,
                "decoded-bytes" => MediaOperationErrorCode::DecodedByteLimit,
                "png-chunk-bytes" => MediaOperationErrorCode::PngChunkLimit,
                _ => MediaOperationErrorCode::InternalTaskFailed,
            },
            Self::MalformedInput { .. } => MediaOperationErrorCode::MalformedMedia,
            Self::MalformedProcessOutput { .. } => MediaOperationErrorCode::MalformedProcessOutput,
            Self::ToolMissing { .. } => MediaOperationErrorCode::ToolMissing,
            Self::ProcessFailed { .. } => MediaOperationErrorCode::ProcessFailed,
            Self::OutputConflict { .. } => MediaOperationErrorCode::OutputConflict,
            Self::Io { .. } => MediaOperationErrorCode::InternalTaskFailed,
        }
    }

    pub(crate) fn media_reason_code(&self) -> Option<MediaOperationReasonCode> {
        let Self::InvalidRequest { reason } = self else {
            return None;
        };

        match *reason {
            "no-frames-selected" => Some(MediaOperationReasonCode::NoFramesSelected),
            "invalid-frame-selection" => Some(MediaOperationReasonCode::InvalidFrameSelection),
            "invalid-frame-duration" => Some(MediaOperationReasonCode::InvalidFrameDuration),
            "duration-too-long" => Some(MediaOperationReasonCode::DurationTooLong),
            "invalid-crop" => Some(MediaOperationReasonCode::InvalidCrop),
            "invalid-output-directory" => Some(MediaOperationReasonCode::InvalidOutputDirectory),
            "unsupported-source-format" => Some(MediaOperationReasonCode::UnsupportedSourceFormat),
            "unsupported-frame-preview" => Some(MediaOperationReasonCode::UnsupportedFramePreview),
            "frame-preview-decode-failed" => {
                Some(MediaOperationReasonCode::FramePreviewDecodeFailed)
            }
            "frame-preview-encode-failed" => {
                Some(MediaOperationReasonCode::FramePreviewEncodeFailed)
            }
            "decode-failed" => Some(MediaOperationReasonCode::DecodeFailed),
            "encode-failed" => Some(MediaOperationReasonCode::EncodeFailed),
            "missing-output" => Some(MediaOperationReasonCode::MissingOutput),
            "plan-invalid" => Some(MediaOperationReasonCode::PlanInvalid),
            "invoke-failed" => Some(MediaOperationReasonCode::InvokeFailed),
            _ => None,
        }
    }

    pub(crate) fn code(&self) -> &'static str {
        self.error_code().as_str()
    }

    pub(crate) fn reason_code(&self) -> Option<&'static str> {
        self.media_reason_code().map(|reason| reason.as_str())
    }
}

impl fmt::Display for PipelineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("operation cancelled"),
            Self::TimedOut { stage } => write!(formatter, "operation timed out during {stage}"),
            Self::OperationConflict { operation_id } => {
                write!(formatter, "operation conflict: {operation_id}")
            }
            Self::InvalidRequest { reason } => write!(formatter, "invalid request: {reason}"),
            Self::InvalidRequestWithoutReason => formatter.write_str("invalid request"),
            Self::SourceChanged => formatter.write_str("source identity changed"),
            Self::LimitExceeded {
                resource,
                limit,
                actual,
            } => write!(formatter, "{resource} limit exceeded ({actual} > {limit})"),
            Self::MalformedInput { format, reason } => {
                write!(formatter, "malformed {format}: {reason}")
            }
            Self::MalformedProcessOutput { reason } => {
                write!(formatter, "malformed process output: {reason}")
            }
            Self::ToolMissing { tool } => write!(formatter, "required tool is missing: {tool}"),
            Self::ProcessFailed {
                command,
                exit_code,
                stderr,
            } => write!(
                formatter,
                "process failed: {command} (exit {exit_code:?}): {stderr}"
            ),
            Self::OutputConflict { path } => write!(
                formatter,
                "output conflicts with {}",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("selected output")
            ),
            Self::Io { operation, message } => write!(formatter, "{operation}: {message}"),
        }
    }
}

impl std::error::Error for PipelineError {}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value;

    use super::{MediaOperationErrorCode, MediaOperationReasonCode, PipelineError};
    use crate::FramePreviewResponse;

    const EXPECTED_ERROR_CODES: [&str; 16] = [
        "cancelled",
        "timed-out",
        "operation-conflict",
        "invalid-request",
        "source-changed",
        "media-input-too-large",
        "media-dimensions-too-large",
        "media-frame-limit",
        "decoded-byte-limit",
        "png-chunk-limit",
        "malformed-media",
        "malformed-process-output",
        "tool-missing",
        "process-failed",
        "output-conflict",
        "internal-task-failed",
    ];

    const EXPECTED_REASON_CODES: [&str; 15] = [
        "no-frames-selected",
        "invalid-frame-selection",
        "invalid-frame-duration",
        "duration-too-long",
        "invalid-crop",
        "invalid-output-directory",
        "unsupported-source-format",
        "unsupported-frame-preview",
        "frame-preview-decode-failed",
        "frame-preview-encode-failed",
        "decode-failed",
        "encode-failed",
        "missing-output",
        "plan-invalid",
        "invoke-failed",
    ];

    fn canonical_codes() -> Value {
        serde_json::from_str(include_str!(
            "../../src/types/media-operation-error-codes.json"
        ))
        .expect("canonical media operation code fixture must parse")
    }

    #[test]
    fn pipeline_error_codes_are_canonical_json_values() {
        let fixture = canonical_codes();
        let fixture_codes = fixture["errorCodes"]
            .as_array()
            .expect("errorCodes must be an array")
            .iter()
            .map(|value| value.as_str().expect("error code must be a string"))
            .collect::<Vec<_>>();
        assert_eq!(fixture_codes, EXPECTED_ERROR_CODES);
        let mappings = vec![
            (PipelineError::Cancelled, "cancelled"),
            (PipelineError::TimedOut { stage: "decode" }, "timed-out"),
            (
                PipelineError::OperationConflict {
                    operation_id: "operation-1".into(),
                },
                "operation-conflict",
            ),
            (
                PipelineError::InvalidRequest {
                    reason: "invalid-crop",
                },
                "invalid-request",
            ),
            (
                PipelineError::InvalidRequestWithoutReason,
                "invalid-request",
            ),
            (PipelineError::SourceChanged, "source-changed"),
            (
                PipelineError::LimitExceeded {
                    resource: "input-bytes",
                    limit: 1,
                    actual: 2,
                },
                "media-input-too-large",
            ),
            (
                PipelineError::LimitExceeded {
                    resource: "image-dimensions",
                    limit: 1,
                    actual: 2,
                },
                "media-dimensions-too-large",
            ),
            (
                PipelineError::LimitExceeded {
                    resource: "image-pixels",
                    limit: 1,
                    actual: 2,
                },
                "media-dimensions-too-large",
            ),
            (
                PipelineError::LimitExceeded {
                    resource: "frame-count",
                    limit: 1,
                    actual: 2,
                },
                "media-frame-limit",
            ),
            (
                PipelineError::LimitExceeded {
                    resource: "decoded-bytes",
                    limit: 1,
                    actual: 2,
                },
                "decoded-byte-limit",
            ),
            (
                PipelineError::LimitExceeded {
                    resource: "png-chunk-bytes",
                    limit: 1,
                    actual: 2,
                },
                "png-chunk-limit",
            ),
            (
                PipelineError::LimitExceeded {
                    resource: "future-limit",
                    limit: 1,
                    actual: 2,
                },
                "internal-task-failed",
            ),
            (
                PipelineError::MalformedInput {
                    format: "png",
                    reason: "bad chunk".into(),
                },
                "malformed-media",
            ),
            (
                PipelineError::MalformedProcessOutput {
                    reason: "bad output".into(),
                },
                "malformed-process-output",
            ),
            (
                PipelineError::ToolMissing { tool: "ffmpeg" },
                "tool-missing",
            ),
            (
                PipelineError::ProcessFailed {
                    command: "ffmpeg".into(),
                    exit_code: Some(1),
                    stderr: "failed".into(),
                },
                "process-failed",
            ),
            (
                PipelineError::OutputConflict {
                    path: PathBuf::from("output.png"),
                },
                "output-conflict",
            ),
            (
                PipelineError::Io {
                    operation: "read",
                    message: "failed".into(),
                },
                "internal-task-failed",
            ),
        ];

        for (error, expected_code) in &mappings {
            assert_eq!(error.code(), *expected_code, "mapping for {error:?}");
        }

        let mut emitted_codes = mappings
            .iter()
            .map(|(_, expected_code)| *expected_code)
            .collect::<Vec<_>>();
        emitted_codes.sort_unstable();
        emitted_codes.dedup();
        let mut expected_codes = EXPECTED_ERROR_CODES.to_vec();
        expected_codes.sort_unstable();
        for code in &emitted_codes {
            assert!(
                expected_codes.contains(code),
                "{code} must come from the canonical JSON fixture",
            );
        }
        assert_eq!(emitted_codes, expected_codes);
    }

    #[test]
    fn invalid_request_reasons_are_canonical_json_values() {
        let fixture = canonical_codes();
        let fixture_reasons = fixture["reasonCodes"]
            .as_array()
            .expect("reasonCodes must be an array")
            .iter()
            .map(|value| value.as_str().expect("reason code must be a string"))
            .collect::<Vec<_>>();
        assert_eq!(fixture_reasons, EXPECTED_REASON_CODES);

        for reason in EXPECTED_REASON_CODES {
            assert_eq!(
                PipelineError::InvalidRequest { reason }.reason_code(),
                Some(reason)
            );
        }
        for reason in [
            "future-reason",
            "invalid-process-frame-config",
            "invalid-output-name",
            "invalid-operation-id",
        ] {
            let error = PipelineError::InvalidRequest { reason };
            assert_eq!(error.reason_code(), None);
            assert_eq!(
                error.error_code(),
                MediaOperationErrorCode::InternalTaskFailed
            );
        }
        assert_eq!(
            PipelineError::InvalidRequestWithoutReason.reason_code(),
            None
        );
    }

    #[test]
    fn closed_error_enum_serializes_all_sixteen_fixture_codes_in_exact_order() {
        let variants = [
            MediaOperationErrorCode::Cancelled,
            MediaOperationErrorCode::TimedOut,
            MediaOperationErrorCode::OperationConflict,
            MediaOperationErrorCode::InvalidRequest,
            MediaOperationErrorCode::SourceChanged,
            MediaOperationErrorCode::MediaInputTooLarge,
            MediaOperationErrorCode::MediaDimensionsTooLarge,
            MediaOperationErrorCode::MediaFrameLimit,
            MediaOperationErrorCode::DecodedByteLimit,
            MediaOperationErrorCode::PngChunkLimit,
            MediaOperationErrorCode::MalformedMedia,
            MediaOperationErrorCode::MalformedProcessOutput,
            MediaOperationErrorCode::ToolMissing,
            MediaOperationErrorCode::ProcessFailed,
            MediaOperationErrorCode::OutputConflict,
            MediaOperationErrorCode::InternalTaskFailed,
        ];
        let serialized = variants
            .into_iter()
            .map(|variant| serde_json::to_value(variant).expect("error enum serialization"))
            .collect::<Vec<_>>();

        assert_eq!(
            Value::Array(serialized),
            canonical_codes()["errorCodes"],
            "the closed Rust enum must own every canonical error spelling and no catch-all"
        );
    }

    #[test]
    fn closed_reason_enum_serializes_all_fifteen_fixture_codes_in_exact_order() {
        let variants = [
            MediaOperationReasonCode::NoFramesSelected,
            MediaOperationReasonCode::InvalidFrameSelection,
            MediaOperationReasonCode::InvalidFrameDuration,
            MediaOperationReasonCode::DurationTooLong,
            MediaOperationReasonCode::InvalidCrop,
            MediaOperationReasonCode::InvalidOutputDirectory,
            MediaOperationReasonCode::UnsupportedSourceFormat,
            MediaOperationReasonCode::UnsupportedFramePreview,
            MediaOperationReasonCode::FramePreviewDecodeFailed,
            MediaOperationReasonCode::FramePreviewEncodeFailed,
            MediaOperationReasonCode::DecodeFailed,
            MediaOperationReasonCode::EncodeFailed,
            MediaOperationReasonCode::MissingOutput,
            MediaOperationReasonCode::PlanInvalid,
            MediaOperationReasonCode::InvokeFailed,
        ];
        let serialized = variants
            .into_iter()
            .map(|variant| serde_json::to_value(variant).expect("reason enum serialization"))
            .collect::<Vec<_>>();

        assert_eq!(
            Value::Array(serialized),
            canonical_codes()["reasonCodes"],
            "the closed Rust enum must own every canonical reason spelling and no catch-all"
        );
    }

    fn response_wire_fields(response: FramePreviewResponse) -> Value {
        let serialized = serde_json::to_value(response).expect("frame preview response JSON");
        serde_json::json!({
            "errorCode": serialized["errorCode"].clone(),
            "reasonCode": serialized["reasonCode"].clone(),
            "errorMessage": serialized["errorMessage"].clone(),
        })
    }

    #[test]
    fn actual_typed_response_fields_serialize_to_the_shared_wire_cases() {
        let actual = [
            (
                "success-null",
                FramePreviewResponse {
                    ok: true,
                    data_url: None,
                    width: None,
                    height: None,
                    error_code: None,
                    reason_code: None,
                    error_message: None,
                },
            ),
            (
                "cancelled",
                FramePreviewResponse {
                    ok: false,
                    data_url: None,
                    width: None,
                    height: None,
                    error_code: Some(MediaOperationErrorCode::Cancelled),
                    reason_code: None,
                    error_message: Some("The media operation was cancelled.".into()),
                },
            ),
            (
                "invalid-request-invalid-crop",
                FramePreviewResponse {
                    ok: false,
                    data_url: None,
                    width: None,
                    height: None,
                    error_code: Some(MediaOperationErrorCode::InvalidRequest),
                    reason_code: Some(MediaOperationReasonCode::InvalidCrop),
                    error_message: Some("Crop selection values must be valid numbers.".into()),
                },
            ),
        ];
        let fixture = canonical_codes();
        let wire_cases = fixture["wireCases"]
            .as_array()
            .expect("wireCases must be an array");

        for (name, response) in actual {
            let expected = wire_cases
                .iter()
                .find(|case| case["name"].as_str() == Some(name))
                .unwrap_or_else(|| panic!("missing wire case {name}"));
            assert_eq!(response_wire_fields(response), expected["wire"]);
        }
    }

    #[test]
    fn all_managed_response_error_fields_use_the_closed_rust_enums() {
        let source = include_str!("lib.rs");
        let structs = [
            "struct MediaInspection",
            "struct FramePreviewResponse",
            "struct FramePreviewsResponse",
            "struct OptimizerPlanResponse",
            "struct StaticImageConversionResult",
            "struct SearchAttemptResult",
            "struct OptimizerSearchResponse",
        ];

        for marker in structs {
            let start = source.find(marker).expect("managed response struct marker");
            let tail = &source[start..];
            let end = tail.find("\n}").expect("managed response struct end");
            let definition = &tail[..end];
            assert!(
                definition.contains("error_code: Option<MediaOperationErrorCode>"),
                "{marker} error code must be closed"
            );
            assert!(
                definition.contains("reason_code: Option<MediaOperationReasonCode>"),
                "{marker} reason code must be closed"
            );
            assert!(
                !definition.contains("error_code: Option<String>"),
                "{marker}"
            );
            assert!(
                !definition.contains("reason_code: Option<String>"),
                "{marker}"
            );
        }
    }
}
