use std::fmt;
use std::path::PathBuf;

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
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::TimedOut { .. } => "timed-out",
            Self::OperationConflict { .. } => "operation-conflict",
            Self::InvalidRequest { .. } | Self::InvalidRequestWithoutReason => "invalid-request",
            Self::SourceChanged => "source-changed",
            Self::LimitExceeded { resource, .. } => match *resource {
                "input-bytes" => "media-input-too-large",
                "image-dimensions" | "image-pixels" => "media-dimensions-too-large",
                "frame-count" => "media-frame-limit",
                "decoded-bytes" => "decoded-byte-limit",
                "png-chunk-bytes" => "png-chunk-limit",
                _ => "internal-task-failed",
            },
            Self::MalformedInput { .. } => "malformed-media",
            Self::MalformedProcessOutput { .. } => "malformed-process-output",
            Self::ToolMissing { .. } => "tool-missing",
            Self::ProcessFailed { .. } => "process-failed",
            Self::OutputConflict { .. } => "output-conflict",
            Self::Io { .. } => "internal-task-failed",
        }
    }

    pub(crate) fn reason_code(&self) -> Option<&'static str> {
        let Self::InvalidRequest { reason } = self else {
            return None;
        };

        match *reason {
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
            | "invoke-failed" => Some(reason),
            _ => None,
        }
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

    use super::PipelineError;

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
        assert_eq!(
            PipelineError::InvalidRequest {
                reason: "future-reason"
            }
            .reason_code(),
            None
        );
        assert_eq!(
            PipelineError::InvalidRequestWithoutReason.reason_code(),
            None
        );
    }
}
