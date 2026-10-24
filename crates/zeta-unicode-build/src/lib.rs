//! Offline trainer and generator for the `zeta-unicode` Nomina image.
//!
//! The trainer reads the UCD's `DerivedName`, `DerivedGeneralCategory`, `NameAliases`,
//! `NamedSequences` and `emoji-test` files, measures the name-codebook candidates a
//! [`Profile`] requests, verifies every section, and writes one flat image plus a
//! deterministic manifest:
//!
//! ```text
//! magic "NOM1" | u16 major, minor, patch | section count, shifts, layout
//! four (u32 offset, u32 length) sections at 8-byte alignment
//! ```
//!
//! The name section stores every explicit name, front-coded against its code-point predecessor
//! and entropy-coded with canonical Huffman models. The global layout checkpoints each block
//! in the stream, so a lookup decodes one checkpoint and at most one block; the block-local
//! layout resets the predecessor at each block. Hangul syllables and `*`-pattern names are
//! generated from the code point. Emoji names are generated from the lowercase UCD name where a
//! grammar rule reproduces them exactly, with literal fallback; GC runs and patterns are
//! compact varint/Huffman streams. Every rule and stream is replayed at build time.
//!
//! [`train`] is the entry point; [`crate::manifest::write_manifest`] records the inputs, sizes and
//! chosen profile, and the `zeta-unicode-train` binary exposes the same pipeline from the
//! command line. The data directory may be a flat vendored directory or a versioned extracted
//! UCD release root; the version headers are validated by [`read_inputs`].
//!
//! # Panics
//!
//! I/O failures are returned as [`Error`]. Malformed records inside a present UCD file are an
//! invariant violation and panic with a message naming the offending entry: this is build-time
//! code generation, so a bad byte is a build failure, not a runtime condition.

use std::path::Path;

pub use error::Error;
pub use manifest::write_manifest;
pub use nomina::emoji::EmojiStats;
pub use nomina::gc::GcStats;
pub use nomina::patterns::PatternStats;
pub use profile::{
    BALANCED_MAX_SHIFT, ChosenProfile, DEFAULT_CANDIDATE_SHIFTS, DEFAULT_MAX_TABLE_BYTES,
    MAX_CANDIDATE_SHIFT, MIN_CANDIDATE_SHIFT, Priority, Profile,
};
pub use train::{
    BaselineComparison, Candidate, Error as TrainError, ImageLayout, NameStats, SectionRecord,
    TrainOutput, VERIFICATION_CHECKS, train, verify,
};
pub use ucd::{InputError, InputPaths, Inputs, read_inputs, read_version};

mod error;
mod manifest;
mod nomina;
mod profile;
mod train;
mod ucd;

/// Writes `contents` to `path`, creating or truncating the file.
pub(crate) fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<(), Error> {
    let path = path.as_ref();
    std::fs::write(path, contents.as_ref()).map_err(|source| Error::Write {
        path: path.to_path_buf(),
        source,
    })
}
