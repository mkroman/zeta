//! The offline generator for the `zeta-unicode` packed Unicode table.
//!
//! This crate parses Unicode 18.0.0's `DerivedName`, `DerivedGeneralCategory`, `NameAliases`,
//! `NamedSequences` and `emoji-test` files, derives everything that Unicode defines
//! algorithmically (Hangul syllables, the `*` name patterns), folds character name aliases into
//! the name table, and writes one binary table plus the generated derivation tree into the
//! consumer's `OUT_DIR`:
//!
//! ```text
//! magic "ZUCD" | u16 major, minor, patch | u16 section_count
//! section_count × (u32 id, u32 file_offset, u32 length)
//! section payloads (offsets inside a section are relative to that section)
//! ```
//!
//! The names are emitted by the vendored encoder in the `names` module: front-coded against
//! each code point predecessor and split into three PPMd8-I streams, with the names that
//! `data/derived-names.rules` can rebuild left as generated Rust instead of table bytes. The
//! encoder round-trips every name against `DerivedName.txt` while it builds.
//!
//! The general category table is the measured two-level design: a page directory (`u16`
//! first-record index per 2,048-code-point page) over `u16` records packing the
//! page-relative range start (11 bits) and the category id (5 bits, 1..=30 in
//! `GC_SHORT_NAMES` order).
//!
//! Emoji keys are canonicalized: `U+FE0F` is stripped, so fully-qualified and text-presentation
//! inputs hit the same key, and every skin-tone variant is *synthesized* at lookup from its
//! base name plus a 3-bit formatting rule — 1,944 stored entries answer for all 5,244.
//!
//! # Panics
//!
//! I/O failures — reading a vendored input or writing an artifact — are returned as [`Error`].
//! Malformed or inconsistent vendored data is an invariant violation and panics with a message
//! naming the offending entry: this is build-time code generation, so a bad byte is a build
//! failure, not a runtime condition. The version-header asserts live in `ucd::read_inputs`,
//! keeping them out of [`generate`]'s own body.

mod emoji;
mod error;
mod gc;
mod names;
mod table;
mod ucd;

use std::path::{Path, PathBuf};

pub use emoji::{EmojiStats, MAX_KEY};
pub use error::Error;
pub use names::{Ledger, SectionSize, StreamSize};

/// Everything [`generate`] needs, decided by the caller.
#[derive(Clone, Debug)]
pub struct Request {
    /// Directory holding the vendored UCD files.
    pub data_dir: PathBuf,
    /// Path to the checked-in name-derivation rules file.
    pub rules: PathBuf,
    /// Directory the generated artifacts are written to (the consumer's `OUT_DIR`).
    pub out_dir: PathBuf,
    /// File name of the packed table written into `out_dir`.
    pub table_file: String,
    /// File name of the generated derivation tree source written into `out_dir`.
    pub derive_file: String,
    /// Print a size report to stderr.
    pub report: bool,
}

/// What one [`generate`] run produced.
#[derive(Debug)]
pub struct Output {
    /// Ledger of the generated name sections.
    pub names: Ledger,
    /// Number of general-category ranges packed into the table.
    pub gc_ranges: usize,
    /// Emoji section summary.
    pub emoji: EmojiStats,
    /// Size of the packed table file.
    pub table_bytes: usize,
}

/// Generates the packed Unicode table and the derivation tree into `request.out_dir`.
///
/// The five UCD files are read from `request.data_dir` and their version headers are checked
/// against each other; the packed table lands in `request.out_dir` under `request.table_file`
/// and the generated derivation tree under `request.derive_file`. The generator round-trips
/// every name against the vendored data before returning.
///
/// # Errors
///
/// Returns [`Error::Read`] when a vendored input cannot be read and [`Error::Write`] when an
/// artifact cannot be written. Malformed or inconsistent vendored data is an invariant
/// violation and panics with a message naming the offending entry (see the crate docs); the
/// version asserts live in `ucd::read_inputs`, so this body contains none.
pub fn generate(request: &Request) -> Result<Output, Error> {
    let inputs = ucd::read_inputs(&request.data_dir)?;

    let gc = gc::parse_general_category(&inputs.gc);
    let (emoji, emoji_stats) = emoji::emoji_section(&inputs.emoji, &inputs.sequences);
    let ledger = names::emit(request, &inputs)?;

    let sections = vec![
        table::Section {
            id: table::SECTION_NAME_RUNS,
            data: ledger.runs_section.clone(),
        },
        table::Section {
            id: table::SECTION_NAME_PATTERNS,
            data: ledger.pattern_section.clone(),
        },
        table::Section {
            id: table::SECTION_GC,
            data: gc::general_category_section(&gc),
        },
        table::Section {
            id: table::SECTION_EMOJI,
            data: emoji,
        },
        table::Section {
            id: table::SECTION_NAMES,
            data: ledger.names_section.clone(),
        },
    ];
    let table = table::pack(sections, inputs.version);
    let table_bytes = table.len();
    write(request.out_dir.join(&request.table_file), &table)?;

    Ok(Output {
        names: ledger,
        gc_ranges: gc.len(),
        emoji: emoji_stats,
        table_bytes,
    })
}

/// Reads `path` fully into memory.
pub(crate) fn read(path: impl AsRef<Path>) -> Result<Vec<u8>, Error> {
    let path = path.as_ref();
    std::fs::read(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })
}

/// Reads `path` as UTF-8 text.
pub(crate) fn read_to_string(path: impl AsRef<Path>) -> Result<String, Error> {
    let path = path.as_ref();
    std::fs::read_to_string(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })
}

/// Writes `contents` to `path`, creating or truncating the file.
pub(crate) fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<(), Error> {
    let path = path.as_ref();
    std::fs::write(path, contents.as_ref()).map_err(|source| Error::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// Creates `path` and every missing parent, like [`std::fs::create_dir_all`].
pub(crate) fn create_dir_all(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = path.as_ref();
    std::fs::create_dir_all(path).map_err(|source| Error::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn generates_the_packed_table_from_the_vendored_data() {
        let data_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../zeta-unicode/data");
        let out = tempfile::tempdir().expect("temp dir");
        let output = generate(&Request {
            data_dir: data_dir.clone(),
            rules: data_dir.join("derived-names.rules"),
            out_dir: out.path().to_path_buf(),
            table_file: "zeta_unicode_tables.bin".into(),
            derive_file: "zeta_unicode_derive.rs".into(),
            report: false,
        })
        .expect("generation succeeds");

        assert!(output.names.entries > 35_000);
        assert!(output.names.table <= 73_304);
        assert!(output.table_bytes > 100_000);
        assert!(out.path().join("zeta_unicode_tables.bin").is_file());
        assert!(out.path().join("zeta_unicode_derive.rs").is_file());
    }
}
