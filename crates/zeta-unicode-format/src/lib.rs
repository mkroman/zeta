//! Layout constants and the Unicode data tables shared by `zeta-unicode` and its tooling.
//!
//! The runtime crate, the offline trainer and `cargo xtask` share the wire-format constants,
//! the UCD input table, the byte primitives, the general-category encoding, the Hangul and
//! emoji tone tables, the name-pattern hex formatter and the emoji name grammar. The generator
//! has to reproduce the runtime's wording exactly, because it selects the rule that the runtime
//! later replays; keeping one implementation means the two can never drift.
//!
//! This crate is deliberately dependency-free: it is linked into the bot through
//! `zeta-unicode`, into the build script through `zeta-unicode-build`, and into workspace
//! automation through `xtask`.
//!
//! The modules group the parts by concern:
//!
//! - [`layout`] — image header parsing, section ids, name-table constants and the Hangul name
//!   generator;
//! - [`decode`] — the front-coding name decoder shared by the runtime and the trainer;
//! - [`gc`] — the general-category variants, their id order and both name tables;
//! - [`inputs`] — the UCD files the trainer reads, their accepted paths and their release
//!   headers;
//! - [`names`] — the name section's fixed index and Huffman model area;
//! - [`primitives`] — varints, hex placeholders, UTF-8 code-point encoding and packed bit
//!   fields;
//! - [`emoji`] — the rule ids, tone tables, name grammar helpers and the rule renderer;
//! - [`huffman`] — the canonical Huffman codec used by every compressed section.
//!
//! The items the runtime, trainer and tooling share are re-exported at the crate root; the
//! module paths remain available for everything else.

pub mod decode;
pub mod emoji;
pub mod gc;
pub mod huffman;
pub mod inputs;
pub mod layout;
pub mod names;
pub mod primitives;

pub use decode::NameDecoder;
pub use emoji::{
    MAX_CPS, MAX_KEY, MAX_RULE_ARGUMENT, Rule, RuleContext, TONE_NAMES, TONE_RULE_PRIORITY, TONES,
    VARIATION_SELECTOR_16, VARIATION_SELECTOR_16_UTF8, ZWJ, canonical_emoji_cps, clock_name,
    family_word, format_toned, is_tone, join_family, keycap_word, pair_phrase, plural, rule_name,
    rule_needs_sequence, swap_person, tone_indices, tone_phrase, tone_positions, ucd_mask_name,
};
pub use gc::{ALL, GC_SHORT_NAMES, GeneralCategory};
pub use huffman::{BitReader, Code, Codebook, Model};
pub use inputs::{
    DERIVED_NAME, EMOJI_TEST, EmojiVersionHeader, GENERAL_CATEGORY, INPUTS, InputSpec,
    NAME_ALIASES, NAMED_SEQUENCES, UcdVersionHeader, emoji_version_header, ucd_version_header,
};
pub use layout::{
    DIRECTORY_AT, DIRECTORY_ENTRY_LEN, EMOJI, EMOJI_DESCRIPTOR_LEN, EMOJI_STREAM_COUNT,
    EMOJI_TONE_CODE_BITS, EMOJI_TONE_MASK_BITS, GC, GC_ID_BITS, GC_ID_MASK, GC_PAGE_SHIFT,
    GC_PAGE_SHIFT_AT, GC_PAGES, GC_SECTION_HEADER_LEN, GC_SHIFT, HANGUL_COUNT, HANGUL_END,
    HANGUL_START, HEADER_LEN, Header, MAGIC, MAX_CODE_BITS, MAX_NAME_BLOCK_SHIFT, MAX_NAME_BYTES,
    MAX_NAME_MODELS, MIN_NAME_BLOCK_SHIFT, NAME_BLOCK_SHIFT_AT, NAME_INDEX_HEADER_LEN,
    NAME_LAYOUT_AT, NAME_MODEL_DIRECTORY_ENTRY_LEN, NAME_PAGE_COUNT, NAME_PAGE_DIRECTORY_ENTRY_LEN,
    NAME_PAGE_SHIFT, NAME_RUN_LEN, NAMES, NameLayout, PATTERNS, SECTION_COUNT, SECTION_COUNT_AT,
    UNICODE_END, VERSION_AT, write_hangul,
};
pub use names::{NameIndex, NameModels};
pub use primitives::{
    encode_cps, pack_bits, read_varint, unpack_bits, write_pattern_name, write_varint,
};

/// The Nomina image size budget, in bytes.
///
/// This is the measured random-access budget, not the smaller full-decode budget: the Nomina
/// image trades table bytes for a tiny decoder and a sub-100 KB heap. It is a product
/// constraint rather than a tuning knob, so the trainer and the runtime share this one value:
/// the trainer defaults to it, the build script refuses to embed an image over it, and the
/// runtime tests the linked image against it.
pub const TABLE_BUDGET: usize = 200_000;
