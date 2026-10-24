//! The Nomina runtime: one flat image, zero-copy lookups.
//!
//! [`Nomina`] parses the 64-byte image header once and exposes the four sections. Stored
//! names decode from a single front-coded block with a fixed 256-byte scratch buffer; Hangul and
//! `*` pattern names are composed on the spot. The emoji (`emoji`) and general-category
//! (`gc`) sections are decoded by their own modules.
//!
//! The image is built by `zeta_unicode_build::nomina`: section 0 is [`names`], section 1 is
//! `emoji`, section 2 is `gc` and section 3 is the pattern table parsed by [`image`]. [`crate`]
//! wraps one image in a thread-local name decoder and process-wide emoji and category tables.

#![allow(
    dead_code,
    reason = "the decoder keeps the full image surface for tests and measurement"
)]

pub mod emoji;
pub mod gc;
pub mod image;
pub mod names;
pub mod patterns;

pub use image::Nomina;
