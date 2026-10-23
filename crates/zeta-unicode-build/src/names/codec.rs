//! The PPMd8-I codec wrappers and the standalone-section framing.

use std::io::Write as _;

use super::SECTION_PREFIX_LEN;

/// PPMd8-I at `order`/`mem_mib` through `ppmd-rust`, the same crate the
/// runtime decoder uses.
pub fn ppmd8i(data: &[u8], order: u32, mem_mib: u32) -> Vec<u8> {
    let mut encoder = ppmd_rust::Ppmd8Encoder::new(
        Vec::new(),
        order,
        mem_mib << 20,
        ppmd_rust::RestoreMethod::Restart,
    )
    .expect("ppmd-rust accepts the configured order and memory");
    encoder.write_all(data).expect("in-memory write");
    encoder.finish(false).expect("ppmd-rust finish")
}

/// A standalone section: `u32 raw length`, `u8 order`, `u8 memory MiB`, then the PPMd8-I stream.
///
/// The codec parameters live in the section rather than only in the rules file, so a decoder
/// cannot silently disagree with whatever the rules were mined for.
pub fn section_bytes(raw: &[u8], order: u32, mem_mib: u32) -> Vec<u8> {
    let compressed = ppmd8i(raw, order, mem_mib);
    let mut out = Vec::with_capacity(SECTION_PREFIX_LEN + compressed.len());
    out.extend_from_slice(
        &u32::try_from(raw.len())
            .expect("raw length fits in u32")
            .to_le_bytes(),
    );
    out.push(u8::try_from(order).expect("PPMd order fits in a byte"));
    out.push(u8::try_from(mem_mib).expect("PPMd memory fits in a byte"));
    out.extend_from_slice(&compressed);
    out
}

/// The codec parameters a section was written with, read back from its prefix.
#[must_use]
pub fn section_codec(bytes: &[u8]) -> (u32, u32) {
    assert!(bytes.len() > SECTION_PREFIX_LEN, "truncated section");
    (
        u32::from(bytes[SECTION_PREFIX_LEN - 2]),
        u32::from(bytes[SECTION_PREFIX_LEN - 1]),
    )
}

/// The codec parameters a standalone section was written with, needed to decode
/// it.  Both sections use fixed parameters, recorded in the rules file.
#[derive(Clone, Copy, Debug, Default)]
pub struct SectionCodec {
    /// `PPMd` model order.
    pub order: u32,
    /// `PPMd` memory limit, in MiB.
    pub mem_mib: u32,
}

/// LEB128: seven payload bits per byte, high bit set on every byte but the last.
pub use zeta_unicode_format::write_varint as varint;
