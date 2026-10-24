//! The "Genesis Lean" image: one flat, zero-copy Unicode table.
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
//! The header records the magic `ZGL1`, the Unicode version as three `u16`s, the section
//! count, the name block shift, the general-category page shift and one `(offset, length)`
//! pair per section. [`image::pack`] writes it; the runtime's `Genesis` reads it.
//!
//! This module is the generator side only. It deliberately does not replace the production
//! path in [`crate`]; the caller decides when the new image becomes the shipped table.

#![allow(
    dead_code,
    reason = "the Genesis image is not wired into the production path yet"
)]

pub mod emoji;
pub mod gc;
pub mod image;
pub mod names;
pub mod patterns;
