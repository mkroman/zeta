//! The UCD inputs consumed by the Nomina trainer and its tooling.
//!
//! One table records every input's canonical file name, the candidate paths under a data root
//! (both the flat vendored layout and an extracted Unicode release are accepted) and the path
//! the fetch task writes. The build resolver, the manifest, `cargo xtask fetch-ucd` and the
//! runtime oracle test all read this table, so they cannot disagree about which files exist or
//! where they may live.

/// One Unicode Character Database input file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputSpec {
    /// The canonical UCD file name, e.g. `DerivedName.txt`.
    pub file: &'static str,
    /// Candidate paths relative to the data root, in probe order.
    pub candidates: &'static [&'static str],
    /// The file's path under a published Unicode release root.
    ///
    /// The fetch task downloads this path and the trainer probes it as one of the
    /// [`candidates`](Self::candidates), so the two can never disagree about where a fetched
    /// release is read from.
    pub fetch_path: &'static str,
}

impl InputSpec {
    /// The candidates probed before [`fetch_path`](Self::fetch_path), in order.
    ///
    /// A file at one of these paths would shadow the one the fetch task writes.
    #[must_use]
    pub fn candidates_before_fetch(&self) -> &'static [&'static str] {
        let candidates = self.candidates;
        let fetched = candidates
            .iter()
            .position(|candidate| *candidate == self.fetch_path)
            .unwrap_or(candidates.len());
        &candidates[..fetched]
    }
}

/// `DerivedName.txt`: every individually named code point and `*` pattern.
pub const DERIVED_NAME: InputSpec = InputSpec {
    file: "DerivedName.txt",
    candidates: &[
        "DerivedName.txt",
        "extracted/DerivedName.txt",
        "ucd/DerivedName.txt",
        "ucd/extracted/DerivedName.txt",
    ],
    fetch_path: "ucd/extracted/DerivedName.txt",
};

/// `DerivedGeneralCategory.txt`: the general-category runs.
pub const GENERAL_CATEGORY: InputSpec = InputSpec {
    file: "DerivedGeneralCategory.txt",
    candidates: &[
        "DerivedGeneralCategory.txt",
        "extracted/DerivedGeneralCategory.txt",
        "ucd/DerivedGeneralCategory.txt",
        "ucd/extracted/DerivedGeneralCategory.txt",
    ],
    fetch_path: "ucd/extracted/DerivedGeneralCategory.txt",
};

/// `NameAliases.txt`: the formal aliases used when the `Name` property is empty.
pub const NAME_ALIASES: InputSpec = InputSpec {
    file: "NameAliases.txt",
    candidates: &[
        "NameAliases.txt",
        "extracted/NameAliases.txt",
        "ucd/NameAliases.txt",
        "ucd/extracted/NameAliases.txt",
    ],
    fetch_path: "ucd/NameAliases.txt",
};

/// `NamedSequences.txt`: the named sequences, absent before Unicode 4.1.
pub const NAMED_SEQUENCES: InputSpec = InputSpec {
    file: "NamedSequences.txt",
    candidates: &[
        "NamedSequences.txt",
        "extracted/NamedSequences.txt",
        "ucd/NamedSequences.txt",
        "ucd/extracted/NamedSequences.txt",
    ],
    fetch_path: "ucd/NamedSequences.txt",
};

/// `emoji-test.txt`: the emoji test data, published in both the UCD and Emoji trees.
pub const EMOJI_TEST: InputSpec = InputSpec {
    file: "emoji-test.txt",
    candidates: &[
        "emoji-test.txt",
        "emoji/emoji-test.txt",
        "ucd/emoji/emoji-test.txt",
        "ucd/emoji-test.txt",
    ],
    fetch_path: "emoji/emoji-test.txt",
};

/// Every UCD input, in the order the fetch task downloads and the manifest iterates them.
pub const INPUTS: &[InputSpec] = &[
    DERIVED_NAME,
    GENERAL_CATEGORY,
    NAME_ALIASES,
    NAMED_SEQUENCES,
    EMOJI_TEST,
];

/// The parsed `<stem>-<major>.<minor>.<patch>.txt` release header of a UCD text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UcdVersionHeader {
    /// The first 32 lines carry no header for the requested stem.
    Missing,
    /// A header matched the stem but its version is malformed; the line is verbatim.
    Malformed(String),
    /// The Unicode version the file advertises.
    Found((u8, u8, u8)),
}

/// The parsed `# Version: <major>.<minor>` header of `emoji-test.txt`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmojiVersionHeader {
    /// The first 32 lines carry no `# Version:` header.
    Missing,
    /// A header matched but its version is malformed; the line is verbatim.
    Malformed(String),
    /// The Emoji version the file advertises.
    Found((u8, u8)),
}

/// Parses the `# <stem>-<major>.<minor>.<patch>.txt` release header of a UCD text.
///
/// Only the first 32 lines are scanned, and the first line that starts with `# `, carries
/// `stem` and ends in `.txt` decides the result: a bad version on that line is
/// [`UcdVersionHeader::Malformed`], not a reason to keep looking. A leading byte-order mark is
/// ignored, so a BOM-prefixed file parses like a plain one. `stem` is the file name without
/// `.txt`, for example `DerivedName`.
#[must_use]
pub fn ucd_version_header(text: &str, stem: &str) -> UcdVersionHeader {
    for line in text.lines().take(32) {
        let line = line.trim_prefix('\u{feff}').trim_end();
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
            return UcdVersionHeader::Malformed(line.to_owned());
        }
        let mut version = [0u8; 3];
        for (slot, part) in version.iter_mut().zip(&parts) {
            let Ok(part) = part.parse() else {
                return UcdVersionHeader::Malformed(line.to_owned());
            };
            *slot = part;
        }
        return UcdVersionHeader::Found(version.into());
    }
    UcdVersionHeader::Missing
}

/// Parses the `# Version: <major>.<minor>` header of `emoji-test.txt`.
///
/// `emoji-test.txt` does not use the `<stem>-<version>.txt` form and has no patch component.
/// Only the first 32 lines are scanned; a leading byte-order mark is ignored.
#[must_use]
pub fn emoji_version_header(text: &str) -> EmojiVersionHeader {
    for line in text.lines().take(32) {
        let line = line.trim_prefix('\u{feff}').trim_end();
        let Some(rest) = line.strip_prefix("# Version:") else {
            continue;
        };
        let mut parts = rest.trim().split('.');
        let (Some(major), Some(minor), None) = (parts.next(), parts.next(), parts.next()) else {
            return EmojiVersionHeader::Malformed(line.to_owned());
        };
        let (Ok(major), Ok(minor)) = (major.parse(), minor.parse()) else {
            return EmojiVersionHeader::Malformed(line.to_owned());
        };
        return EmojiVersionHeader::Found((major, minor));
    }
    EmojiVersionHeader::Missing
}

#[cfg(test)]
mod tests {
    use super::{
        EmojiVersionHeader, INPUTS, InputSpec, UcdVersionHeader, emoji_version_header,
        ucd_version_header,
    };

    #[test]
    fn every_fetch_path_is_an_accepted_candidate() {
        for spec in INPUTS {
            assert!(
                spec.candidates.contains(&spec.fetch_path),
                "{} fetches to {:?}, which the trainer does not probe",
                spec.file,
                spec.fetch_path
            );
        }
    }

    #[test]
    fn candidates_before_fetch_stop_at_the_fetched_path() {
        let spec = InputSpec {
            file: "example.txt",
            candidates: &["a/example.txt", "b/example.txt", "c/example.txt"],
            fetch_path: "b/example.txt",
        };
        assert_eq!(spec.candidates_before_fetch(), ["a/example.txt"]);

        let missing = InputSpec {
            file: "missing.txt",
            candidates: &["a/missing.txt"],
            fetch_path: "b/missing.txt",
        };
        assert_eq!(missing.candidates_before_fetch(), ["a/missing.txt"]);
    }

    #[test]
    fn ucd_headers_parse_every_outcome() {
        assert_eq!(
            ucd_version_header("0041; LATIN CAPITAL LETTER A\n", "DerivedName"),
            UcdVersionHeader::Missing
        );
        assert_eq!(
            ucd_version_header("\u{feff}# DerivedName-18.0.0.txt\n", "DerivedName"),
            UcdVersionHeader::Found((18, 0, 0))
        );
        assert_eq!(
            ucd_version_header("# NameAliases-18.0.0.txt\n", "DerivedName"),
            UcdVersionHeader::Missing,
            "a header for another stem is not this file's header"
        );
        assert_eq!(
            ucd_version_header("# DerivedName-18.0.txt\n", "DerivedName"),
            UcdVersionHeader::Malformed("# DerivedName-18.0.txt".to_owned())
        );
        assert_eq!(
            ucd_version_header("# DerivedName-18.x.0.txt\n", "DerivedName"),
            UcdVersionHeader::Malformed("# DerivedName-18.x.0.txt".to_owned())
        );
        assert_eq!(
            ucd_version_header(
                "# DerivedName-17.0.0.txt\n# DerivedName-18.0.0.txt\n",
                "DerivedName"
            ),
            UcdVersionHeader::Found((17, 0, 0)),
            "the first matching header decides"
        );
    }

    #[test]
    fn emoji_test_headers_parse_every_outcome() {
        assert_eq!(
            emoji_version_header("# emoji-test.txt\n"),
            EmojiVersionHeader::Missing
        );
        assert_eq!(
            emoji_version_header("\u{feff}# Version: 16.0  \r\n"),
            EmojiVersionHeader::Found((16, 0))
        );
        assert_eq!(
            emoji_version_header("# Version: 16.0.1\n"),
            EmojiVersionHeader::Malformed("# Version: 16.0.1".to_owned())
        );
        assert_eq!(
            emoji_version_header("# Version: 16\n"),
            EmojiVersionHeader::Malformed("# Version: 16".to_owned())
        );
    }
}
