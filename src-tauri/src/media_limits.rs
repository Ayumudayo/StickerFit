use std::collections::hash_map::DefaultHasher;
use std::fs::{self, Metadata};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::media_error::PipelineError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MediaLimits {
    pub(crate) max_input_bytes: u64,
    pub(crate) max_dimension: u32,
    pub(crate) max_pixels: u64,
    pub(crate) max_single_rgba_bytes: u64,
    pub(crate) max_total_decoded_bytes: u64,
    pub(crate) max_frame_count: u32,
    pub(crate) max_png_chunk_bytes: u32,
}

impl Default for MediaLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 512 * 1024 * 1024,
            max_dimension: 16_384,
            max_pixels: 40_000_000,
            max_single_rgba_bytes: 160 * 1024 * 1024,
            max_total_decoded_bytes: 160 * 1024 * 1024,
            max_frame_count: 300,
            max_png_chunk_bytes: 64 * 1024 * 1024,
        }
    }
}

pub(crate) fn checked_rgba_bytes(
    width: u32,
    height: u32,
    limits: MediaLimits,
) -> Result<usize, PipelineError> {
    let largest_dimension = u64::from(width.max(height));
    if width > limits.max_dimension || height > limits.max_dimension {
        return Err(PipelineError::LimitExceeded {
            resource: "image-dimensions",
            limit: u64::from(limits.max_dimension),
            actual: largest_dimension,
        });
    }

    let pixels =
        u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or(PipelineError::LimitExceeded {
                resource: "image-pixels",
                limit: limits.max_pixels,
                actual: u64::MAX,
            })?;
    if pixels > limits.max_pixels {
        return Err(PipelineError::LimitExceeded {
            resource: "image-pixels",
            limit: limits.max_pixels,
            actual: pixels,
        });
    }

    let rgba_bytes = pixels.checked_mul(4).ok_or(PipelineError::LimitExceeded {
        resource: "decoded-bytes",
        limit: limits.max_single_rgba_bytes,
        actual: u64::MAX,
    })?;
    if rgba_bytes > limits.max_single_rgba_bytes {
        return Err(PipelineError::LimitExceeded {
            resource: "decoded-bytes",
            limit: limits.max_single_rgba_bytes,
            actual: rgba_bytes,
        });
    }

    usize::try_from(rgba_bytes).map_err(|_| PipelineError::LimitExceeded {
        resource: "decoded-bytes",
        limit: limits.max_single_rgba_bytes,
        actual: rgba_bytes,
    })
}

pub(crate) fn validate_input_file(
    path: &Path,
    limits: MediaLimits,
) -> Result<Metadata, PipelineError> {
    let metadata = fs::metadata(path).map_err(|error| PipelineError::Io {
        operation: "read input metadata",
        message: error.to_string(),
    })?;
    validate_file_metadata(metadata, limits)
}

pub(crate) fn validate_file_metadata(
    metadata: Metadata,
    limits: MediaLimits,
) -> Result<Metadata, PipelineError> {
    if !metadata.is_file() {
        return Err(PipelineError::MalformedInput {
            format: "input",
            reason: "source is not a regular file".to_string(),
        });
    }
    if metadata.len() > limits.max_input_bytes {
        return Err(PipelineError::LimitExceeded {
            resource: "input-bytes",
            limit: limits.max_input_bytes,
            actual: metadata.len(),
        });
    }
    Ok(metadata)
}

pub(crate) fn image_decode_limits(limits: MediaLimits) -> image::Limits {
    let mut result = image::Limits::default();
    result.max_image_width = Some(limits.max_dimension);
    result.max_image_height = Some(limits.max_dimension);
    result.max_alloc = Some(limits.max_single_rgba_bytes);
    result
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SourceIdentity {
    pub(crate) canonical_path: PathBuf,
    pub(crate) file_len: u64,
    pub(crate) modified_nanos: u128,
}

impl SourceIdentity {
    pub(crate) fn from_path(path: &Path, limits: MediaLimits) -> Result<Self, PipelineError> {
        let canonical_path = fs::canonicalize(path).map_err(|error| PipelineError::Io {
            operation: "canonicalize input",
            message: error.to_string(),
        })?;
        let metadata = validate_input_file(&canonical_path, limits)?;
        let modified_nanos = metadata
            .modified()
            .map_err(|error| PipelineError::Io {
                operation: "read input modification time",
                message: error.to_string(),
            })?
            .duration_since(UNIX_EPOCH)
            .map_err(|error| PipelineError::Io {
                operation: "normalize input modification time",
                message: error.to_string(),
            })?
            .as_nanos();

        Ok(Self {
            canonical_path,
            file_len: metadata.len(),
            modified_nanos,
        })
    }

    pub(crate) fn revision(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        format!("source-{:016x}", hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        checked_rgba_bytes, image_decode_limits, validate_input_file, MediaLimits, SourceIdentity,
    };
    use crate::media_error::PipelineError;

    fn sparse_test_file(name: &str, length: u64) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "stickerfit-{name}-{}-{nonce}.bin",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .expect("sparse test file must be created");
        file.set_len(length)
            .expect("sparse test file length must be set");
        path
    }

    #[test]
    fn checked_rgba_bytes_rejects_checked_multiplication_overflow() {
        let limits = MediaLimits {
            max_dimension: u32::MAX,
            max_pixels: u64::MAX,
            max_single_rgba_bytes: u64::MAX,
            ..MediaLimits::default()
        };

        let error = checked_rgba_bytes(u32::MAX, u32::MAX, limits)
            .expect_err("RGBA byte multiplication must be checked");

        assert!(matches!(
            error,
            PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                ..
            }
        ));
    }

    #[test]
    fn checked_rgba_bytes_rejects_dimension_above_16384() {
        let error = checked_rgba_bytes(16_385, 1, MediaLimits::default())
            .expect_err("oversized dimension must be rejected");

        assert!(matches!(
            error,
            PipelineError::LimitExceeded {
                resource: "image-dimensions",
                limit: 16_384,
                actual: 16_385,
            }
        ));
    }

    #[test]
    fn checked_rgba_bytes_rejects_more_than_40000000_pixels() {
        let error = checked_rgba_bytes(10_000, 4_001, MediaLimits::default())
            .expect_err("oversized pixel count must be rejected");

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
    fn validate_input_file_rejects_512_mib_plus_one_byte() {
        let path = sparse_test_file("oversized-input", 512 * 1024 * 1024 + 1);

        let error = validate_input_file(&path, MediaLimits::default())
            .expect_err("oversized input must be rejected");
        fs::remove_file(path).expect("sparse test file must be removed");

        assert!(matches!(
            error,
            PipelineError::LimitExceeded {
                resource: "input-bytes",
                limit: 536_870_912,
                actual: 536_870_913,
            }
        ));
    }

    #[test]
    fn checked_rgba_bytes_rejects_more_than_160_mib() {
        let limits = MediaLimits {
            max_pixels: u64::MAX,
            ..MediaLimits::default()
        };
        let error = checked_rgba_bytes(10_000, 4_195, limits)
            .expect_err("oversized RGBA allocation must be rejected");

        assert!(matches!(
            error,
            PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: 167_772_160,
                actual: 167_800_000,
            }
        ));
    }

    #[test]
    fn validate_input_file_rejects_non_regular_files() {
        let error = validate_input_file(&std::env::temp_dir(), MediaLimits::default())
            .expect_err("directories are not media input files");

        assert!(matches!(
            error,
            PipelineError::MalformedInput {
                format: "input",
                ..
            }
        ));
    }

    #[test]
    fn image_decode_limits_match_media_limits() {
        let media_limits = MediaLimits::default();
        let limits = image_decode_limits(media_limits);

        assert_eq!(limits.max_image_width, Some(media_limits.max_dimension));
        assert_eq!(limits.max_image_height, Some(media_limits.max_dimension));
        assert_eq!(limits.max_alloc, Some(media_limits.max_single_rgba_bytes));
    }

    #[test]
    fn source_identity_revision_is_stable_and_changes_with_identity_fields() {
        let base = SourceIdentity {
            canonical_path: std::path::PathBuf::from("C:/media/input.png"),
            file_len: 10,
            modified_nanos: 20,
        };
        let changed_len = SourceIdentity {
            file_len: 11,
            ..base.clone()
        };
        let changed_modified = SourceIdentity {
            modified_nanos: 21,
            ..base.clone()
        };

        assert_eq!(base.revision(), base.revision());
        assert_ne!(base.revision(), changed_len.revision());
        assert_ne!(base.revision(), changed_modified.revision());
        assert!(!base.revision().contains("C:/media/input.png"));
    }
}
