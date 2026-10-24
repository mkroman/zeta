//! The vendored UCD inputs and their version headers.
//!
//! Inputs are discovered through a candidate list instead of a fixed path, so the generator
//! accepts both the flat vendored layout (`data/DerivedName.txt`) and the layout of an
//! extracted Unicode release (`data/ucd/extracted/DerivedName.txt`,
//! `data/ucd/emoji/emoji-test.txt`). A missing file reports every path that was tried.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// The first Unicode release whose UCD ships `NamedSequences.txt` (4.1.0 introduced it).
///
/// Releases before this one may omit the file; later releases must include it.
pub const NAMED_SEQUENCES_SINCE: (u8, u8, u8) = (4, 1, 0);

/// The five UCD texts, the UCD version their headers agree on, and the Emoji version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inputs {
    /// `DerivedName.txt`: every individually named code point and `*` pattern.
    pub names: String,
    /// `DerivedGeneralCategory.txt`.
    pub gc: String,
    /// `NameAliases.txt`.
    pub aliases: String,
    /// `NamedSequences.txt`, or an empty string when the release predates it.
    pub sequences: String,
    /// `emoji-test.txt`, with pre-13.0 comments upgraded to the `E<version>` form.
    pub emoji: String,
    /// The `(major, minor, patch)` version every UCD header advertises.
    pub version: (u8, u8, u8),
    /// The Emoji version advertised by `emoji-test.txt`, which is an independent release
    /// (Unicode 10.0 ships Emoji 5.0, for example).
    pub emoji_version: (u8, u8),
    /// The resolved file behind each input.
    pub paths: InputPaths,
}

/// The resolved file behind each [`Inputs`] field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputPaths {
    /// The resolved `DerivedName.txt`.
    pub names: PathBuf,
    /// The resolved `DerivedGeneralCategory.txt`.
    pub gc: PathBuf,
    /// The resolved `NameAliases.txt`.
    pub aliases: PathBuf,
    /// The resolved `NamedSequences.txt`, or `None` when the release predates it.
    pub sequences: Option<PathBuf>,
    /// The resolved `emoji-test.txt`.
    pub emoji: PathBuf,
}

/// A UCD file that an input could not be resolved or validated against.
#[derive(Debug, Error)]
pub enum InputError {
    /// No candidate path for a required input exists.
    #[error("missing {file} (tried: {tried})", tried = join_paths(.tried))]
    Missing {
        /// The canonical file name, e.g. `DerivedName.txt`.
        file: &'static str,
        /// Every path that was tried, in order.
        tried: Vec<PathBuf>,
    },
    /// No candidate path for `DerivedName.txt` exists.
    #[error(
        "missing DerivedName.txt, required from Unicode 10.0 onward (tried: {tried}); \
         UnicodeData.txt is not a substitute because the algorithmic `*` pattern ranges \
         would be absent",
        tried = join_paths(.tried)
    )]
    MissingDerivedName {
        /// Every path that was tried, in order.
        tried: Vec<PathBuf>,
    },
    /// A candidate exists but cannot be read as UTF-8 text.
    #[error("failed to read {}: {source}", .path.display())]
    Read {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A UCD file has no version header in its first lines.
    #[error("{file} at {} has no version header", .path.display())]
    MissingVersion {
        /// The canonical file name.
        file: &'static str,
        /// The resolved path.
        path: PathBuf,
    },
    /// A version header exists but is not `<stem>-<major>.<minor>.<patch>.txt`.
    #[error("{} has a malformed {file} version header {header:?}", .path.display())]
    MalformedVersion {
        /// The canonical file name.
        file: &'static str,
        /// The resolved path.
        path: PathBuf,
        /// The offending header line.
        header: String,
    },
    /// A file advertises a different Unicode release than `DerivedName.txt`.
    #[error(
        "{} advertises Unicode {} for {file}, but {} advertises {}",
        .path.display(),
        display_version(*.found),
        .expected_file,
        display_version(*.expected)
    )]
    VersionMismatch {
        /// The canonical file name.
        file: &'static str,
        /// The resolved path.
        path: PathBuf,
        /// The file the expected version came from.
        expected_file: &'static str,
        /// The version `DerivedName.txt` advertises.
        expected: (u8, u8, u8),
        /// The version this file advertises.
        found: (u8, u8, u8),
    },
}

/// One UCD file: its canonical name and the paths to try, in order.
struct InputSpec {
    /// The canonical file name used in errors and the manifest.
    file: &'static str,
    /// Paths relative to the data root, tried in order.
    candidates: &'static [&'static str],
}

/// `DerivedName.txt`, flat or inside an extracted release.
const DERIVED_NAME: InputSpec = InputSpec {
    file: "DerivedName.txt",
    candidates: &[
        "DerivedName.txt",
        "extracted/DerivedName.txt",
        "ucd/DerivedName.txt",
        "ucd/extracted/DerivedName.txt",
    ],
};

/// `DerivedGeneralCategory.txt`, flat or inside an extracted release.
const GENERAL_CATEGORY: InputSpec = InputSpec {
    file: "DerivedGeneralCategory.txt",
    candidates: &[
        "DerivedGeneralCategory.txt",
        "extracted/DerivedGeneralCategory.txt",
        "ucd/DerivedGeneralCategory.txt",
        "ucd/extracted/DerivedGeneralCategory.txt",
    ],
};

/// `NameAliases.txt`, flat or inside an extracted release.
const NAME_ALIASES: InputSpec = InputSpec {
    file: "NameAliases.txt",
    candidates: &[
        "NameAliases.txt",
        "extracted/NameAliases.txt",
        "ucd/NameAliases.txt",
        "ucd/extracted/NameAliases.txt",
    ],
};

/// `NamedSequences.txt`, flat or inside an extracted release.
const NAMED_SEQUENCES: InputSpec = InputSpec {
    file: "NamedSequences.txt",
    candidates: &[
        "NamedSequences.txt",
        "extracted/NamedSequences.txt",
        "ucd/NamedSequences.txt",
        "ucd/extracted/NamedSequences.txt",
    ],
};

/// `emoji-test.txt`, flat or inside an extracted release's `emoji/` directory.
const EMOJI_TEST: InputSpec = InputSpec {
    file: "emoji-test.txt",
    candidates: &[
        "emoji-test.txt",
        "emoji/emoji-test.txt",
        "ucd/emoji/emoji-test.txt",
        "ucd/emoji-test.txt",
    ],
};

/// Iterates the non-empty, non-comment lines of a UCD text with their 1-based line numbers.
///
/// A leading byte-order mark is ignored, so a BOM-prefixed file parses like a plain one.
pub fn records(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .map(|(index, line)| (index + 1, line))
}

/// Parses one hexadecimal Unicode code point.
///
/// # Panics
///
/// Panics with the offending text when it is not a hexadecimal `u32`.
#[must_use]
pub fn hex_u32(text: &str) -> u32 {
    let text = text.trim();
    u32::from_str_radix(text, 16)
        .unwrap_or_else(|error| panic!("invalid hexadecimal code point {text:?}: {error}"))
}

/// Reads the five UCD texts from `data_dir` and validates their version headers.
///
/// Every file is resolved through a candidate list, so both a flat layout
/// (`data/DerivedName.txt`) and an extracted release (`data/ucd/extracted/DerivedName.txt`,
/// `data/emoji/emoji-test.txt`) are accepted. `NamedSequences.txt` may be absent only for
/// releases before Unicode 4.0; `DerivedName.txt` itself exists from Unicode 10.0 onward, and
/// its absence is reported with the paths that were tried. The Emoji version
/// in `emoji-test.txt` is recorded independently because it does not track the UCD release
/// number (Unicode 10.0 ships Emoji 5.0).
///
/// # Errors
///
/// Returns [`InputError`] when a required input is missing, cannot be read, or advertises a
/// different Unicode release than `DerivedName.txt`.
pub fn read_inputs(data_dir: &Path) -> Result<Inputs, InputError> {
    let (names, names_path) =
        read_required(data_dir, &DERIVED_NAME).map_err(|error| match error {
            InputError::Missing { tried, .. } => InputError::MissingDerivedName { tried },
            other => other,
        })?;
    let version = header_version(DERIVED_NAME.file, &names, &names_path)?;

    let (gc, gc_path) = read_required(data_dir, &GENERAL_CATEGORY)?;
    let (aliases, aliases_path) = read_required(data_dir, &NAME_ALIASES)?;
    let (emoji, emoji_path) = read_required(data_dir, &EMOJI_TEST)?;

    check_version(&GENERAL_CATEGORY, &gc, &gc_path, version)?;
    check_version(&NAME_ALIASES, &aliases, &aliases_path, version)?;
    // `emoji-test.txt` advertises the Emoji version, not the UCD version: Unicode 10.0 ships
    // Emoji 5.0, and an Emoji 13.1 file is valid on top of Unicode 13.0. Validate that the
    // header parses, but do not force it to equal the UCD release.
    let emoji_version = emoji_test_version(&emoji, &emoji_path)?;
    // Emoji 13.0 added the `E<version>` token to each line's comment. Older releases omit it;
    // upgrade them here so every downstream parser sees one shape.
    let emoji = upgrade_emoji_comments(&emoji, emoji_version);

    let (sequences, sequences_path) = match find_input(data_dir, &NAMED_SEQUENCES) {
        Some(path) => {
            let text = read_file(&path)?;
            check_version(&NAMED_SEQUENCES, &text, &path, version)?;
            (text, Some(path))
        }
        None if version < NAMED_SEQUENCES_SINCE => (String::new(), None),
        None => {
            return Err(InputError::Missing {
                file: NAMED_SEQUENCES.file,
                tried: candidates(data_dir, &NAMED_SEQUENCES),
            });
        }
    };

    Ok(Inputs {
        names,
        gc,
        aliases,
        sequences,
        emoji,
        version,
        emoji_version,
        paths: InputPaths {
            names: names_path,
            gc: gc_path,
            aliases: aliases_path,
            sequences: sequences_path,
            emoji: emoji_path,
        },
    })
}

/// Reads only the Unicode version advertised by `DerivedName.txt`.
///
/// This is the cheap probe a build script can use before selecting a training profile; it does
/// not read the other inputs.
///
/// # Errors
///
/// Returns [`InputError`] when `DerivedName.txt` is missing, cannot be read, or has no valid
/// version header.
pub fn read_version(data_dir: &Path) -> Result<(u8, u8, u8), InputError> {
    let path =
        find_input(data_dir, &DERIVED_NAME).ok_or_else(|| InputError::MissingDerivedName {
            tried: candidates(data_dir, &DERIVED_NAME),
        })?;
    let text = read_file(&path)?;
    header_version(DERIVED_NAME.file, &text, &path)
}

/// Finds the first existing candidate for `spec`.
fn find_input(data_dir: &Path, spec: &InputSpec) -> Option<PathBuf> {
    spec.candidates
        .iter()
        .map(|relative| data_dir.join(relative))
        .find(|path| path.is_file())
}

/// Every candidate path for `spec`, in the order [`find_input`] tries them.
fn candidates(data_dir: &Path, spec: &InputSpec) -> Vec<PathBuf> {
    spec.candidates
        .iter()
        .map(|relative| data_dir.join(relative))
        .collect()
}

/// Reads a required input, or reports every candidate path that was tried.
fn read_required(data_dir: &Path, spec: &InputSpec) -> Result<(String, PathBuf), InputError> {
    let Some(path) = find_input(data_dir, spec) else {
        return Err(InputError::Missing {
            file: spec.file,
            tried: candidates(data_dir, spec),
        });
    };
    let text = read_file(&path)?;
    Ok((text, path))
}

/// Reads a discovered file as UTF-8 text.
fn read_file(path: &Path) -> Result<String, InputError> {
    std::fs::read_to_string(path).map_err(|source| InputError::Read {
        path: path.to_path_buf(),
        source,
    })
}

/// Validates one file's header against the version `DerivedName.txt` advertised.
fn check_version(
    spec: &InputSpec,
    text: &str,
    path: &Path,
    expected: (u8, u8, u8),
) -> Result<(), InputError> {
    let found = header_version(spec.file, text, path)?;
    if found == expected {
        return Ok(());
    }
    Err(InputError::VersionMismatch {
        file: spec.file,
        path: path.to_path_buf(),
        expected_file: DERIVED_NAME.file,
        expected,
        found,
    })
}

/// Reads the `# <Stem>-<major>.<minor>.<patch>.txt` header shared by the UCD files.
fn header_version(file: &'static str, text: &str, path: &Path) -> Result<(u8, u8, u8), InputError> {
    let stem = file.strip_suffix(".txt").unwrap_or(file);
    for line in text.lines().take(32) {
        let line = line.strip_prefix('\u{feff}').unwrap_or(line);
        let Some(rest) = line.strip_prefix("# ") else {
            continue;
        };
        let Some(rest) = rest
            .strip_prefix(stem)
            .and_then(|rest| rest.strip_prefix('-'))
        else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(".txt") else {
            continue;
        };
        let parts: Vec<&str> = rest.split('.').collect();
        if parts.len() != 3 {
            return Err(InputError::MalformedVersion {
                file,
                path: path.to_path_buf(),
                header: line.to_owned(),
            });
        }
        let mut version = [0u8; 3];
        for (slot, part) in version.iter_mut().zip(&parts) {
            *slot = part.parse().map_err(|_| InputError::MalformedVersion {
                file,
                path: path.to_path_buf(),
                header: line.to_owned(),
            })?;
        }
        return Ok(version.into());
    }
    Err(InputError::MissingVersion {
        file,
        path: path.to_path_buf(),
    })
}

/// Reads the `# Version: <major>.<minor>` header of `emoji-test.txt`.
///
/// `emoji-test.txt` does not use the `<Stem>-<version>.txt` form and has no patch component.
fn emoji_test_version(text: &str, path: &Path) -> Result<(u8, u8), InputError> {
    for line in text.lines().take(32) {
        let line = line.strip_prefix('\u{feff}').unwrap_or(line);
        let Some(rest) = line.strip_prefix("# Version:") else {
            continue;
        };
        let mut parts = rest.trim().split('.');
        let (Some(major), Some(minor), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(InputError::MalformedVersion {
                file: EMOJI_TEST.file,
                path: path.to_path_buf(),
                header: line.to_owned(),
            });
        };
        let Ok(major) = major.parse() else {
            return Err(InputError::MalformedVersion {
                file: EMOJI_TEST.file,
                path: path.to_path_buf(),
                header: line.to_owned(),
            });
        };
        let Ok(minor) = minor.parse() else {
            return Err(InputError::MalformedVersion {
                file: EMOJI_TEST.file,
                path: path.to_path_buf(),
                header: line.to_owned(),
            });
        };
        return Ok((major, minor));
    }
    Err(InputError::MissingVersion {
        file: EMOJI_TEST.file,
        path: path.to_path_buf(),
    })
}

/// Upgrades the pre-13.0 `emoji-test.txt` comment shape to the modern one.
///
/// Emoji 13.0 added the `E<version>` token to each data line's comment; Emoji 5.0 through 12.0
/// write `<emoji> <name>` instead. The emoji-section generator expects the token, so the
/// ingestion layer inserts the file's own Emoji version into older files.
/// Header and blank lines are left untouched.
fn upgrade_emoji_comments(text: &str, version: (u8, u8)) -> String {
    let mut out = String::with_capacity(text.len() + 1_024);
    for line in text.split_inclusive('\n') {
        let (body, ending) = line.strip_suffix('\n').map_or((line, ""), |body| {
            body.strip_suffix('\r')
                .map_or((body, "\n"), |body| (body, "\r\n"))
        });
        let Some((data, comment)) = body.split_once('#') else {
            out.push_str(body);
            out.push_str(ending);
            continue;
        };
        if body.trim_start().starts_with('#') {
            out.push_str(body);
            out.push_str(ending);
            continue;
        }
        let Some((emoji, rest)) = comment.trim_start().split_once(char::is_whitespace) else {
            out.push_str(body);
            out.push_str(ending);
            continue;
        };
        let rest = rest.trim_start();
        let first_name_word = rest.split_whitespace().next().unwrap_or_default();
        if is_emoji_version(first_name_word) {
            out.push_str(body);
            out.push_str(ending);
            continue;
        }
        out.push_str(data);
        out.push_str("# ");
        out.push_str(emoji);
        let _ = write!(out, " E{}.{} ", version.0, version.1);
        out.push_str(rest);
        out.push_str(ending);
    }
    out
}

/// Does `word` look like the `E<major>.<minor>` token in modern `emoji-test.txt` comments?
fn is_emoji_version(word: &str) -> bool {
    let Some(rest) = word.strip_prefix('E') else {
        return false;
    };
    let mut parts = rest.split('.');
    let numeric = |part: Option<&str>| {
        part.is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    numeric(parts.next()) && numeric(parts.next()) && parts.next().is_none()
}

/// Formats a version for an error message.
fn display_version(version: (u8, u8, u8)) -> String {
    format!("{}.{}.{}", version.0, version.1, version.2)
}

/// Joins tried paths for an error message.
fn join_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Writes one UCD text with its version header.
    fn write(dir: &Path, relative: &str, contents: &str) {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture directory");
        }
        fs::write(path, contents).expect("fixture file");
    }

    /// Writes the five flat 18.0.0 fixtures.
    fn flat_18(dir: &Path) {
        write(
            dir,
            "DerivedName.txt",
            "# DerivedName-18.0.0.txt\n0041; LATIN CAPITAL LETTER A\n",
        );
        write(
            dir,
            "DerivedGeneralCategory.txt",
            "# DerivedGeneralCategory-18.0.0.txt\n0041; Lu\n",
        );
        write(
            dir,
            "NameAliases.txt",
            "# NameAliases-18.0.0.txt\n0000; NULL; control\n",
        );
        write(
            dir,
            "NamedSequences.txt",
            "# NamedSequences-18.0.0.txt\nKEYCAP DIGIT TWO; 0032 20E3\n",
        );
        write(
            dir,
            "emoji-test.txt",
            "# emoji-test.txt\n# Version: 18.0\n1F600 ; fully-qualified # x E1.0 grinning face\n",
        );
    }

    #[test]
    fn reads_a_flat_unicode_18_layout() {
        let dir = tempfile::tempdir().expect("temp dir");
        flat_18(dir.path());

        let inputs = read_inputs(dir.path()).expect("inputs");
        assert_eq!(inputs.version, (18, 0, 0));
        assert_eq!(inputs.emoji_version, (18, 0));
        assert_eq!(inputs.paths.names, dir.path().join("DerivedName.txt"));
        assert_eq!(
            inputs.paths.sequences,
            Some(dir.path().join("NamedSequences.txt"))
        );
        assert!(inputs.names.contains("LATIN CAPITAL LETTER A"));
        assert!(inputs.emoji.contains("grinning face"));
    }

    #[test]
    fn reads_an_extracted_unicode_15_1_layout() {
        let dir = tempfile::tempdir().expect("temp dir");
        write(
            dir.path(),
            "ucd/extracted/DerivedName.txt",
            "# DerivedName-15.1.0.txt\n0041; LATIN CAPITAL LETTER A\n",
        );
        write(
            dir.path(),
            "ucd/extracted/DerivedGeneralCategory.txt",
            "# DerivedGeneralCategory-15.1.0.txt\n0041; Lu\n",
        );
        write(
            dir.path(),
            "ucd/NameAliases.txt",
            "# NameAliases-15.1.0.txt\n0000; NULL; control\n",
        );
        write(
            dir.path(),
            "ucd/NamedSequences.txt",
            "# NamedSequences-15.1.0.txt\nKEYCAP DIGIT TWO; 0032 20E3\n",
        );
        write(
            dir.path(),
            "ucd/emoji/emoji-test.txt",
            "# emoji-test.txt\n# Version: 15.1\n\
             1F600 ; fully-qualified # x E1.0 grinning face\n\
             1F3FB ; component # x E1.0 light skin tone\n\
             1F1E6 1F1E7 ; minimally-qualified # x E2.0 flag\n\
             1F1E8 1F1E9 ; unqualified # x E2.0 flag\n",
        );

        let inputs = read_inputs(dir.path()).expect("inputs");
        assert_eq!(inputs.version, (15, 1, 0));
        assert_eq!(inputs.emoji_version, (15, 1));
        assert_eq!(
            inputs.paths.names,
            dir.path().join("ucd/extracted/DerivedName.txt")
        );
        assert_eq!(
            inputs.paths.emoji,
            dir.path().join("ucd/emoji/emoji-test.txt")
        );
    }

    #[test]
    fn records_the_independent_emoji_version_of_unicode_10() {
        let dir = tempfile::tempdir().expect("temp dir");
        write(
            dir.path(),
            "DerivedName.txt",
            "# DerivedName-10.0.0.txt\n0041; LATIN CAPITAL LETTER A\n",
        );
        write(
            dir.path(),
            "DerivedGeneralCategory.txt",
            "# DerivedGeneralCategory-10.0.0.txt\n0041; Lu\n",
        );
        write(
            dir.path(),
            "NameAliases.txt",
            "# NameAliases-10.0.0.txt\n0000; NULL; control\n",
        );
        write(
            dir.path(),
            "NamedSequences.txt",
            "# NamedSequences-10.0.0.txt\nKEYCAP DIGIT TWO; 0032 20E3\n",
        );
        write(
            dir.path(),
            "emoji/emoji-test.txt",
            "# emoji-test.txt\n# Version: 5.0\n1F600 ; fully-qualified # 😀 grinning face\n",
        );

        let inputs = read_inputs(dir.path()).expect("Unicode 10 + Emoji 5 inputs");
        assert_eq!(inputs.version, (10, 0, 0));
        assert_eq!(inputs.emoji_version, (5, 0));
        assert_eq!(inputs.paths.emoji, dir.path().join("emoji/emoji-test.txt"));
        assert!(
            inputs
                .emoji
                .contains("1F600 ; fully-qualified # 😀 E5.0 grinning face"),
            "old comment shape was not upgraded: {}",
            inputs.emoji
        );
        let (_, stats) =
            crate::nomina::emoji::build(&inputs.emoji, &inputs.sequences, &inputs.names);
        assert_eq!(stats.entries, 1, "the upgraded comment must parse");
    }

    #[test]
    fn upgrades_old_emoji_comments_without_touching_new_ones() {
        let text = "# emoji-test.txt\n# Version: 5.0\n\
                    1F600 ; fully-qualified # 😀 grinning face\n\
                    1F601 ; fully-qualified # 😁 E1.0 beaming face\n";
        let upgraded = upgrade_emoji_comments(text, (5, 0));
        assert!(upgraded.contains("# 😀 E5.0 grinning face"));
        assert!(upgraded.contains("# 😁 E1.0 beaming face"));
        assert!(upgraded.starts_with("# emoji-test.txt\n# Version: 5.0\n"));
    }

    #[test]
    fn rejects_headers_from_different_releases() {
        let dir = tempfile::tempdir().expect("temp dir");
        flat_18(dir.path());
        write(
            dir.path(),
            "DerivedGeneralCategory.txt",
            "# DerivedGeneralCategory-17.0.0.txt\n0041; Lu\n",
        );

        let error = read_inputs(dir.path()).expect_err("mismatched headers");
        assert!(matches!(
            error,
            InputError::VersionMismatch {
                file: "DerivedGeneralCategory.txt",
                expected: (18, 0, 0),
                found: (17, 0, 0),
                ..
            }
        ));
    }

    #[test]
    fn reports_every_tried_path_for_a_missing_derived_name() {
        let dir = tempfile::tempdir().expect("temp dir");
        let error = read_inputs(dir.path()).expect_err("missing DerivedName");

        let InputError::MissingDerivedName { tried } = error else {
            panic!("expected MissingDerivedName");
        };
        assert_eq!(
            tried,
            [
                "DerivedName.txt",
                "extracted/DerivedName.txt",
                "ucd/DerivedName.txt",
                "ucd/extracted/DerivedName.txt",
            ]
            .map(|relative| dir.path().join(relative))
        );
    }

    #[test]
    fn reports_a_missing_required_emoji_test() {
        let dir = tempfile::tempdir().expect("temp dir");
        flat_18(dir.path());
        fs::remove_file(dir.path().join("emoji-test.txt")).expect("remove fixture");

        let error = read_inputs(dir.path()).expect_err("missing emoji-test");
        assert!(matches!(
            error,
            InputError::Missing {
                file: "emoji-test.txt",
                ..
            }
        ));
    }

    #[test]
    fn allows_missing_named_sequences_before_unicode_4() {
        let dir = tempfile::tempdir().expect("temp dir");
        write(
            dir.path(),
            "DerivedName.txt",
            "# DerivedName-3.2.0.txt\n0041; LATIN CAPITAL LETTER A\n",
        );
        write(
            dir.path(),
            "DerivedGeneralCategory.txt",
            "# DerivedGeneralCategory-3.2.0.txt\n0041; Lu\n",
        );
        write(
            dir.path(),
            "NameAliases.txt",
            "# NameAliases-3.2.0.txt\n0000; NULL; control\n",
        );
        write(
            dir.path(),
            "emoji-test.txt",
            "# emoji-test.txt\n# Version: 3.2\n1F600 ; fully-qualified # x E1.0 grinning face\n",
        );

        let inputs = read_inputs(dir.path()).expect("inputs");
        assert_eq!(inputs.version, (3, 2, 0));
        assert_eq!(inputs.sequences, "");
        assert_eq!(inputs.paths.sequences, None);
    }

    #[test]
    fn requires_named_sequences_from_unicode_4_onward() {
        let dir = tempfile::tempdir().expect("temp dir");
        flat_18(dir.path());
        fs::remove_file(dir.path().join("NamedSequences.txt")).expect("remove fixture");

        let error = read_inputs(dir.path()).expect_err("missing NamedSequences");
        assert!(matches!(
            error,
            InputError::Missing {
                file: "NamedSequences.txt",
                ..
            }
        ));
    }

    #[test]
    fn parses_a_bom_prefixed_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        flat_18(dir.path());
        write(
            dir.path(),
            "DerivedName.txt",
            "\u{feff}# DerivedName-18.0.0.txt\n0041; LATIN CAPITAL LETTER A\n",
        );

        let inputs = read_inputs(dir.path()).expect("BOM-prefixed inputs");
        assert_eq!(inputs.version, (18, 0, 0));
        let parsed: Vec<_> = records(&inputs.names).collect();
        assert_eq!(parsed, vec![(2, "0041; LATIN CAPITAL LETTER A")]);
    }

    #[test]
    fn read_version_matches_the_full_reader() {
        let dir = tempfile::tempdir().expect("temp dir");
        flat_18(dir.path());
        assert_eq!(
            read_version(dir.path()).expect("version"),
            read_inputs(dir.path()).expect("inputs").version
        );
    }
}
