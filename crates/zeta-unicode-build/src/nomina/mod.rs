//! The Nomina image: one flat, zero-copy Unicode table.
//!
//! The image is a 64-byte header followed by four 8-byte-aligned sections:
//!
//! | id | section | owner |
//! |---:|---|---|
//! | 0 | stored character names | this module ([`names`]) |
//! | 1 | emoji entries | `emoji` |
//! | 2 | general category | `gc` |
//! | 3 | `*` name patterns | `patterns` |
//!
//! The header records the magic `NOM1`, the Unicode version as three `u16`s, the section
//! count, the name block shift, the general-category page shift and one `(offset, length)`
//! pair per section. [`image::pack`] writes it; the runtime's `Nomina` reads it.

#![allow(
    dead_code,
    reason = "the module keeps the full encoder surface for training, measurement and tests"
)]

pub mod emoji;
pub mod gc;
pub mod image;
pub mod names;
pub mod patterns;
