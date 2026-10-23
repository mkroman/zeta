//! The vendored UCD inputs and their version headers.

use std::path::Path;

/// The five UCD texts, and the version their headers agree on.
pub struct Inputs {
    /// `DerivedName.txt`: every individually named code point and `*` pattern.
    pub names: String,
    /// `DerivedGeneralCategory.txt`.
    pub gc: String,
    /// `NameAliases.txt`.
    pub aliases: String,
    /// `NamedSequences.txt`.
    pub sequences: String,
    /// `emoji-test.txt`.
    pub emoji: String,
    /// The `(major, minor, patch)` version every header advertises.
    pub version: (u8, u8, u8),
}

/// Reads the five vendored UCD files from `data_dir` and validates their version headers.
///
/// # Errors
///
/// Returns [`Error`](crate::Error) when a vendored file cannot be read. A file whose version
/// header is missing or disagrees with the others is an invariant violation and panics: the
/// tables would silently mix releases.
pub fn read_inputs(data_dir: &Path) -> Result<Inputs, crate::Error> {
    let names = crate::read_to_string(data_dir.join("DerivedName.txt"))?;
    let gc = crate::read_to_string(data_dir.join("DerivedGeneralCategory.txt"))?;
    let aliases = crate::read_to_string(data_dir.join("NameAliases.txt"))?;
    let sequences = crate::read_to_string(data_dir.join("NamedSequences.txt"))?;
    let emoji = crate::read_to_string(data_dir.join("emoji-test.txt"))?;

    // Every vendored file must advertise the same version, or the tables would silently mix
    // releases.
    let version = header_version("DerivedName", &names);
    for (stem, text) in [
        ("DerivedGeneralCategory", &gc),
        ("NameAliases", &aliases),
        ("NamedSequences", &sequences),
    ] {
        assert_eq!(
            header_version(stem, text),
            version,
            "{stem}.txt is not Unicode {version:?}"
        );
    }
    assert_eq!(
        emoji_test_version(&emoji),
        (version.0, version.1),
        "emoji-test.txt is not Unicode {version:?}"
    );

    Ok(Inputs {
        names,
        gc,
        aliases,
        sequences,
        emoji,
        version,
    })
}

/// Reads the `# <Stem>-<major>.<minor>.<patch>.txt` header shared by the UCD files.
fn header_version(stem: &str, text: &str) -> (u8, u8, u8) {
    for line in text.lines().take(8) {
        let Some(rest) = line.strip_prefix("# ") else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(stem).and_then(|r| r.strip_prefix('-')) else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(".txt") else {
            continue;
        };
        let mut parts = rest.split('.');
        let parse = |component: Option<&str>| -> u8 {
            component
                .expect("version component")
                .parse()
                .expect("numeric version component")
        };
        let version = (
            parse(parts.next()),
            parse(parts.next()),
            parse(parts.next()),
        );
        assert!(
            parts.next().is_none(),
            "malformed version header: {stem}-{rest}"
        );
        return version;
    }
    panic!("no version header found for {stem}");
}

/// Reads the `# Version: <major>.<minor>` header of `emoji-test.txt`.
fn emoji_test_version(text: &str) -> (u8, u8) {
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("# Version:") else {
            continue;
        };
        let mut parts = rest.trim().split('.');
        let major = parts
            .next()
            .expect("major version")
            .parse()
            .expect("numeric major version");
        let minor = parts
            .next()
            .expect("minor version")
            .parse()
            .expect("numeric minor version");
        return (major, minor);
    }
    panic!("no version header found for emoji-test");
}
