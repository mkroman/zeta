//! Section ids of the generated table, shared with `zeta-unicode-build`.
//!
//! They are internal: nothing outside the crate names a section, and every reader tolerates a
//! missing one by answering "no data" rather than panicking.

/// The name index: `varint(len), varint(gap)` per run of consecutive named code points, behind a
/// `u32` raw length, a `u8` model order and a `u8` memory limit in MiB, PPMd8-I compressed.
/// Maps a code point to a stored name id.
pub const NAME_RUNS: u32 = 1;
/// The `*`-pattern ranges, stored plainly: `u32 lo`, `u32 hi`, `u16 prefix length`, the prefix,
/// `u16 suffix length`, the suffix. There are twenty of them and a name miss scans them on every
/// call, so they are not behind a codec.
pub const NAME_PATTERNS: u32 = 2;
/// General category: a page directory (`u16` per 2,048-code-point page) over packed `u16`
/// records (page-relative range start in the top 11 bits, category id in the low 5).
pub const GC: u32 = 3;
/// Emoji entries: key and name blobs, `u16` offset tables and packed 3-bit tone rules.
pub const EMOJI: u32 = 4;
/// The name streams: the `UNNM` container with its three PPMd8-I streams.
pub const NAMES: u32 = 5;
