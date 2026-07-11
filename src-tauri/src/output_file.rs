use crate::media_error::PipelineError;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;
use unicode_normalization::UnicodeNormalization;

const MAX_STEM_UTF16_UNITS: usize = 96;
const MAX_SUFFIX_BYTES: usize = 64;
const MAX_EXTENSION_BYTES: usize = 16;
const MAX_PUBLISH_ATTEMPTS: usize = 32;

static OUTPUT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct PendingOutput {
    temp: NamedTempFile,
    final_path: PathBuf,
    directory: PathBuf,
    stem: String,
    suffix: String,
    extension: String,
}

impl PendingOutput {
    pub(crate) fn new(
        directory: &Path,
        input_path: &Path,
        suffix: &str,
        extension: &str,
    ) -> Result<Self, PipelineError> {
        validate_filename_fragment(suffix, MAX_SUFFIX_BYTES)?;
        validate_filename_fragment(extension, MAX_EXTENSION_BYTES)?;

        let directory = directory.to_path_buf();
        let stem = safe_unicode_stem(input_path);
        let final_path = next_final_path(&directory, &stem, suffix, extension);
        let temp = NamedTempFile::new_in(&directory)
            .map_err(|error| output_io_error("create temporary output", error))?;

        Ok(Self {
            temp,
            final_path,
            directory,
            stem,
            suffix: suffix.to_string(),
            extension: extension.to_string(),
        })
    }

    pub(crate) fn writer(&mut self) -> &mut File {
        self.temp.as_file_mut()
    }

    pub(crate) fn temp_path(&self) -> &Path {
        self.temp.path()
    }

    pub(crate) fn commit(mut self) -> Result<PathBuf, PipelineError> {
        self.temp
            .as_file_mut()
            .flush()
            .map_err(|error| output_io_error("flush temporary output", error))?;
        self.temp
            .as_file()
            .sync_all()
            .map_err(|error| output_io_error("sync temporary output", error))?;

        let Self {
            mut temp,
            mut final_path,
            directory,
            stem,
            suffix,
            extension,
        } = self;

        for attempt in 0..MAX_PUBLISH_ATTEMPTS {
            match temp.persist_noclobber(&final_path) {
                Ok(persisted_file) => {
                    drop(persisted_file);
                    sync_parent_directory_best_effort(&directory);
                    return Ok(final_path);
                }
                Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                    temp = error.file;
                    if attempt + 1 == MAX_PUBLISH_ATTEMPTS {
                        return Err(PipelineError::OutputConflict { path: final_path });
                    }
                    final_path = next_final_path(&directory, &stem, &suffix, &extension);
                }
                Err(error) => {
                    return Err(output_io_error("publish output", error.error));
                }
            }
        }

        unreachable!("bounded publication loop always returns")
    }
}

pub(crate) fn safe_unicode_stem(input_path: &Path) -> String {
    let source_stem = input_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned)
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "image".to_string());
    sanitize_unicode_stem(&source_stem)
}

fn sanitize_unicode_stem(source_stem: &str) -> String {
    let mut sanitized = source_stem
        .nfc()
        .map(|character| {
            if is_windows_forbidden_character(character) {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();

    if sanitized.is_empty() {
        sanitized.push_str("image");
    }
    if is_windows_reserved_stem(&sanitized) {
        sanitized.insert(0, '_');
    }

    let mut truncated = String::new();
    let mut utf16_units = 0usize;
    for character in sanitized.chars() {
        let character_units = character.len_utf16();
        if utf16_units + character_units > MAX_STEM_UTF16_UNITS {
            break;
        }
        truncated.push(character);
        utf16_units += character_units;
    }

    let safe_end = truncated.trim_end_matches(['.', ' ']).len();
    if safe_end < truncated.len() {
        let trailing_count = truncated[safe_end..].chars().count();
        truncated.truncate(safe_end);
        truncated.push_str(&"_".repeat(trailing_count));
    }

    if truncated.is_empty() {
        "image".to_string()
    } else {
        truncated
    }
}

fn is_windows_forbidden_character(character: char) -> bool {
    character.is_ascii_control()
        || matches!(
            character,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
        )
}

fn is_windows_reserved_stem(stem: &str) -> bool {
    let device_name = stem
        .split_once('.')
        .map(|(prefix, _)| prefix)
        .unwrap_or(stem)
        .trim_end_matches(['.', ' ']);
    let uppercase = device_name.to_ascii_uppercase();

    matches!(uppercase.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || matches!(
            uppercase.strip_prefix("COM"),
            Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
        )
        || matches!(
            uppercase.strip_prefix("LPT"),
            Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
        )
}

fn validate_filename_fragment(value: &str, max_bytes: usize) -> Result<(), PipelineError> {
    if value.is_empty()
        || value.len() > max_bytes
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(PipelineError::InvalidRequest {
            reason: "invalid-output-name",
        });
    }
    Ok(())
}

fn next_final_path(directory: &Path, stem: &str, suffix: &str, extension: &str) -> PathBuf {
    let unix_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = OUTPUT_COUNTER.fetch_add(1, Ordering::Relaxed);
    directory.join(format!(
        "{stem}-{suffix}-{unix_nanos}-{}-{counter}.{extension}",
        std::process::id()
    ))
}

fn output_io_error(operation: &'static str, error: impl std::fmt::Display) -> PipelineError {
    PipelineError::Io {
        operation,
        message: error.to_string(),
    }
}

#[cfg(unix)]
fn sync_parent_directory_best_effort(directory: &Path) {
    if let Ok(directory_file) = File::open(directory) {
        let _ = directory_file.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_directory_best_effort(_directory: &Path) {}

#[cfg(test)]
mod tests {
    use super::{safe_unicode_stem, sanitize_unicode_stem, PendingOutput};
    use crate::media_error::PipelineError;
    use std::collections::HashSet;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn committed_file_name(path: &Path) -> String {
        path.file_name()
            .and_then(|name| name.to_str())
            .expect("committed output name must be valid Unicode")
            .to_string()
    }

    #[test]
    fn safe_unicode_stem_preserves_and_normalizes_unicode() {
        assert_eq!(
            safe_unicode_stem(Path::new("고양이 스티커.png")),
            "고양이 스티커"
        );
        assert_eq!(safe_unicode_stem(Path::new("e\u{301}.png")), "é");
    }

    #[test]
    fn sanitizer_replaces_only_windows_forbidden_controls_and_trailing_dot_space() {
        assert_eq!(
            sanitize_unicode_stem("고양이 (스티커)+<bad>:\"/\\|?*\u{0007}"),
            "고양이 (스티커)+_bad_________"
        );
        assert_eq!(sanitize_unicode_stem("trail. "), "trail__");
    }

    #[test]
    fn sanitizer_protects_windows_reserved_device_names() {
        for reserved in [
            "CON",
            "prn",
            "AUX",
            "nul",
            "COM1",
            "com9.log",
            "LPT1",
            "lpt9.txt",
            "COM¹",
            "com².png",
            "COM³",
            "LPT¹",
            "lpt².log",
            "LPT³",
        ] {
            let sanitized = sanitize_unicode_stem(reserved);
            assert!(
                sanitized.starts_with('_'),
                "reserved device stem {reserved:?} was not protected: {sanitized:?}"
            );
        }

        for ordinary in ["COM0", "COM10", "LPT0", "LPT10", "CONSOLE"] {
            assert_eq!(sanitize_unicode_stem(ordinary), ordinary);
        }
    }

    #[test]
    fn sanitizer_does_not_replace_non_ascii_controls_or_trailing_unicode_space() {
        assert_eq!(
            sanitize_unicode_stem("keep\u{0085}\u{00a0}"),
            "keep\u{0085}\u{00a0}"
        );
        assert_eq!(sanitize_unicode_stem("delete\u{007f}"), "delete_");
    }

    #[test]
    fn sanitizer_caps_stems_at_96_utf16_units_without_splitting_scalars() {
        let exact = sanitize_unicode_stem(&format!("{}x", "😀".repeat(48)));
        assert_eq!(exact.encode_utf16().count(), 96);
        assert_eq!(exact.chars().count(), 48);
        assert!(exact.ends_with('😀'));

        let would_split = sanitize_unicode_stem(&format!("{}😀", "a".repeat(95)));
        assert_eq!(would_split.encode_utf16().count(), 95);
        assert!(would_split.ends_with('a'));

        let exposed_trailing_dot = sanitize_unicode_stem(&format!("{}.b", "a".repeat(95)));
        assert_eq!(exposed_trailing_dot.encode_utf16().count(), 96);
        assert!(exposed_trailing_dot.ends_with('_'));

        let protected_reserved = sanitize_unicode_stem(&format!("CON.{}", "a".repeat(100)));
        assert_eq!(protected_reserved.encode_utf16().count(), 96);
        assert!(protected_reserved.starts_with("_CON."));
    }

    #[test]
    fn pending_outputs_commit_to_32_unique_names_concurrently() {
        let directory = Arc::new(tempfile::tempdir().expect("temp directory must be created"));
        let start_barrier = Arc::new(Barrier::new(32));
        let handles = (0..32)
            .map(|index| {
                let directory = Arc::clone(&directory);
                let start_barrier = Arc::clone(&start_barrier);
                thread::spawn(move || {
                    start_barrier.wait();
                    let mut output = PendingOutput::new(
                        directory.path(),
                        Path::new("고양이 스티커.png"),
                        "candidate",
                        "png",
                    )
                    .expect("pending output must be created");
                    write!(output.writer(), "candidate-{index}")
                        .expect("candidate bytes must be written");
                    output.commit().expect("candidate must commit")
                })
            })
            .collect::<Vec<_>>();

        let paths = handles
            .into_iter()
            .map(|handle| handle.join().expect("output worker must not panic"))
            .collect::<Vec<PathBuf>>();
        let names = paths
            .iter()
            .map(|path| committed_file_name(path))
            .collect::<HashSet<_>>();

        assert_eq!(paths.len(), 32);
        assert_eq!(names.len(), 32);
        assert!(paths.iter().all(|path| path.is_file()));
        assert!(names
            .iter()
            .all(|name| name.starts_with("고양이 스티커-candidate-") && name.ends_with(".png")));
    }

    #[test]
    fn dropping_or_erroring_a_pending_output_leaves_no_partial_file() {
        let directory = tempfile::tempdir().expect("temp directory must be created");
        let (temp_path, final_path) = {
            let mut output = PendingOutput::new(
                directory.path(),
                Path::new("source.png"),
                "cancelled",
                "png",
            )
            .expect("pending output must be created");
            output
                .writer()
                .write_all(b"partial")
                .expect("partial bytes must be written");
            (output.temp_path().to_path_buf(), output.final_path.clone())
        };
        assert!(!temp_path.exists());
        assert!(!final_path.exists());

        let error_result = (|| -> Result<(), PipelineError> {
            let mut output =
                PendingOutput::new(directory.path(), Path::new("source.png"), "failed", "png")?;
            output
                .writer()
                .write_all(b"partial")
                .map_err(|error| PipelineError::Io {
                    operation: "write test output",
                    message: error.to_string(),
                })?;
            Err(PipelineError::Cancelled)
        })();
        assert_eq!(error_result, Err(PipelineError::Cancelled));
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("temp directory must be readable")
                .count(),
            0
        );
    }

    #[test]
    fn commit_never_overwrites_an_existing_final_file() {
        let directory = tempfile::tempdir().expect("temp directory must be created");
        let mut output = PendingOutput::new(
            directory.path(),
            Path::new("source.png"),
            "candidate",
            "png",
        )
        .expect("pending output must be created");
        output
            .writer()
            .write_all(b"new output")
            .expect("output bytes must be written");
        let colliding_path = output.final_path.clone();
        fs::write(&colliding_path, b"existing output").expect("collision fixture must be written");

        let committed_path = output.commit().expect("collision must be retried");

        assert_ne!(committed_path, colliding_path);
        assert_eq!(
            fs::read(&colliding_path).expect("existing output must remain readable"),
            b"existing output"
        );
        assert_eq!(
            fs::read(&committed_path).expect("committed output must be readable"),
            b"new output"
        );
    }

    #[test]
    fn only_the_selected_pending_output_is_published() {
        let directory = tempfile::tempdir().expect("temp directory must be created");
        let mut winner =
            PendingOutput::new(directory.path(), Path::new("source.png"), "winner", "png")
                .expect("winner guard must be created");
        winner
            .writer()
            .write_all(b"winner")
            .expect("winner bytes must be written");
        let mut loser =
            PendingOutput::new(directory.path(), Path::new("source.png"), "loser", "png")
                .expect("loser guard must be created");
        loser
            .writer()
            .write_all(b"loser")
            .expect("loser bytes must be written");

        drop(loser);
        let winner_path = winner.commit().expect("winner must commit");
        let entries = fs::read_dir(directory.path())
            .expect("output directory must be readable")
            .map(|entry| entry.expect("directory entry must be readable").path())
            .collect::<Vec<_>>();

        assert_eq!(entries, vec![winner_path]);
    }

    #[test]
    fn pending_output_rejects_non_internal_filename_fragments() {
        let directory = tempfile::tempdir().expect("temp directory must be created");

        for (suffix, extension) in [
            ("../candidate", "png"),
            ("candidate", "p/ng"),
            ("후보", "png"),
            ("candidate", ""),
        ] {
            assert!(matches!(
                PendingOutput::new(directory.path(), Path::new("source.png"), suffix, extension,),
                Err(PipelineError::InvalidRequest { .. })
            ));
        }
    }

    #[test]
    fn pending_output_enforces_suffix_and_extension_length_bounds() {
        let directory = tempfile::tempdir().expect("temp directory must be created");
        let max_suffix = "s".repeat(64);
        let max_extension = "e".repeat(16);

        drop(
            PendingOutput::new(
                directory.path(),
                Path::new("source.png"),
                &max_suffix,
                &max_extension,
            )
            .expect("64-byte suffix and 16-byte extension must be accepted"),
        );
        assert!(matches!(
            PendingOutput::new(
                directory.path(),
                Path::new("source.png"),
                &"s".repeat(65),
                "png",
            ),
            Err(PipelineError::InvalidRequest { .. })
        ));
        assert!(matches!(
            PendingOutput::new(
                directory.path(),
                Path::new("source.png"),
                "candidate",
                &"e".repeat(17),
            ),
            Err(PipelineError::InvalidRequest { .. })
        ));
    }
}
