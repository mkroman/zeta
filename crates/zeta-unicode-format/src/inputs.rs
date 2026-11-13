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

#[cfg(test)]
mod tests {
    use super::{INPUTS, InputSpec};

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
}
