//! Layout constants and the emoji grammar shared by `zeta-unicode` and `zeta-unicode-build`.
//!
//! The runtime crate and the offline trainer share the wire-format constants, the byte
//! primitives, the general-category encoding, the Hangul and emoji tone tables, the name-pattern
//! hex formatter and the emoji name grammar. The generator has to reproduce the runtime's
//! wording exactly, because it selects the rule that the runtime later replays; keeping one
//! implementation means the two can never drift.
//!
//! This crate is deliberately dependency-free: it is linked into the bot through
//! `zeta-unicode` and into the build script through `zeta-unicode-build`.
//!
//! The modules group the parts by concern:
//!
//! - [`layout`] — image header offsets, section ids, name and category table constants and
//!   the Hangul name generator;
//! - [`primitives`] — varints, hex placeholders, UTF-8 code-point encoding and packed bit
//!   fields;
//! - [`emoji`] — the rule ids, tone tables and name-grammar helpers;
//! - [`huffman`] — the canonical Huffman codec used by every compressed section.
//!
//! Everything is re-exported at the crate root so callers can keep using the flat paths.

pub mod emoji;
pub mod huffman;
pub mod layout;
pub mod primitives;

pub use emoji::{
    MAX_CPS, MAX_KEY, MAX_RULE_ARGUMENT, Rule, TONE_NAMES, TONES, ZWJ, clock_name, family_word,
    format_toned, is_tone, join_family, keycap_word, pair_phrase, plural, swap_person,
    tone_indices, tone_phrase, tone_positions, ucd_mask_name,
};
pub use layout::{
    DIRECTORY_AT, DIRECTORY_ENTRY_LEN, EMOJI, GC, GC_ID_BITS, GC_ID_MASK, GC_PAGE_SHIFT,
    GC_PAGE_SHIFT_AT, GC_PAGES, GC_SHIFT, GC_SHORT_NAMES, HANGUL_COUNT, HANGUL_END, HANGUL_START,
    HEADER_LEN, MAGIC, MAX_CODE_BITS, MAX_NAME_BLOCK_SHIFT, MAX_NAME_BYTES, MAX_NAME_MODELS,
    MIN_NAME_BLOCK_SHIFT, NAME_BLOCK_SHIFT_AT, NAME_CONTEXT_MAP_LEN, NAME_INDEX_HEADER_LEN,
    NAME_LAYOUT_AT, NAME_MODEL_COUNT_LEN, NAME_MODEL_DIRECTORY_ENTRY_LEN, NAME_PAGE_COUNT,
    NAME_PAGE_DIRECTORY_ENTRY_LEN, NAME_PAGE_SHIFT, NAME_RUN_LEN, NAMES, NameLayout, PATTERNS,
    SECTION_COUNT, SECTION_COUNT_AT, UNICODE_END, UNICODE_LIMIT, UNICODE_START, VERSION_AT,
    write_hangul,
};
pub use primitives::{
    encode_cps, pack_bits, read_varint, unpack_bits, write_hex, write_pattern_name, write_varint,
};
