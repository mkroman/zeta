//! The generated table: its header, section directory and bounds-checked readers.
//!
//! `zeta-unicode-build` writes one binary file into `OUT_DIR` and the crate links it in with
//! `include_bytes!`, so the table costs no filesystem or startup work of its own.

use zeta_unicode_format::{TABLE_ENTRY_LEN, TABLE_HEADER_LEN, TABLE_MAGIC};

/// The generated table, linked in at compile time.
const TABLE_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/zeta_unicode_tables.bin"));

/// The generated table, linked in at compile time.
pub static TABLE: &[u8] = TABLE_BYTES;

/// Reads a little-endian `u16` at a fixed offset for the compile-time header check.
const fn header_u16(at: usize) -> u16 {
    u16::from_le_bytes([TABLE_BYTES[at], TABLE_BYTES[at + 1]])
}

// The version check is a build invariant: a stale table must fail the build, not the first
// lookup. `build.rs` regenerates the table whenever its inputs change, so this can only trip
// when `UNICODE_VERSION` is bumped without regenerating.
const _: () = {
    assert!(
        TABLE_BYTES.len() >= TABLE_HEADER_LEN,
        "the generated table is truncated"
    );
    assert!(
        TABLE_BYTES[0] == TABLE_MAGIC[0]
            && TABLE_BYTES[1] == TABLE_MAGIC[1]
            && TABLE_BYTES[2] == TABLE_MAGIC[2]
            && TABLE_BYTES[3] == TABLE_MAGIC[3],
        "the generated table is not a zeta-unicode table"
    );
    assert!(
        header_u16(4) == crate::UNICODE_VERSION.0 as u16
            && header_u16(6) == crate::UNICODE_VERSION.1 as u16
            && header_u16(8) == crate::UNICODE_VERSION.2 as u16,
        "the generated table was built from a different Unicode release"
    );
};

/// The Unicode version the table was built from, for the version test.
#[cfg(test)]
pub fn version() -> (u8, u8, u8) {
    let (major, minor, patch) = (
        u16_at(TABLE, 4).expect("version in the header"),
        u16_at(TABLE, 6).expect("version in the header"),
        u16_at(TABLE, 8).expect("version in the header"),
    );
    (
        u8::try_from(major).expect("major version fits in a byte"),
        u8::try_from(minor).expect("minor version fits in a byte"),
        u8::try_from(patch).expect("patch version fits in a byte"),
    )
}

/// Returns the payload of section `id`.
///
/// Returns `None` for an id the table does not have, which keeps every reader total: a table
/// built by a different generator answers "no data" instead of panicking on a lookup.
#[must_use]
pub fn section(id: u32) -> Option<&'static [u8]> {
    let count = usize::from(u16_at(TABLE, 10)?);
    for index in 0..count {
        let record = TABLE_HEADER_LEN + index * TABLE_ENTRY_LEN;
        let section_id = u32_at(TABLE, record)?;
        if section_id != id {
            continue;
        }
        let offset = usize::try_from(u32_at(TABLE, record + 4)?).ok()?;
        let length = usize::try_from(u32_at(TABLE, record + 8)?).ok()?;
        return bytes_at(TABLE, offset, length);
    }
    None
}

/// Reads a `u16` at `offset`, failing rather than wrapping.
#[must_use]
pub fn u16_at(buf: &[u8], offset: usize) -> Option<u16> {
    let bytes = buf.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// Reads a `u32` at `offset`, failing rather than wrapping.
#[must_use]
pub fn u32_at(buf: &[u8], offset: usize) -> Option<u32> {
    let bytes = buf.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Reads a `usize` from the `u32` at `offset`.
#[must_use]
pub fn usize_at(buf: &[u8], offset: usize) -> Option<usize> {
    usize::try_from(u32_at(buf, offset)?).ok()
}

/// Borrows `length` bytes at `offset`, failing rather than wrapping.
#[must_use]
pub fn bytes_at(buf: &[u8], offset: usize, length: usize) -> Option<&[u8]> {
    buf.get(offset..offset.checked_add(length)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_reject_truncated_input() {
        assert_eq!(u16_at(&[0], 0), None);
        assert_eq!(u16_at(&[0, 1], 1), None);
        assert_eq!(u32_at(&[0; 3], 0), None);
        assert_eq!(usize_at(&[0; 3], 0), None);
        assert_eq!(bytes_at(b"abc", 2, 2), None);
        assert_eq!(bytes_at(b"abc", usize::MAX, 1), None);
    }

    #[test]
    fn readers_decode_little_endian() {
        assert_eq!(u16_at(&[0x34, 0x12], 0), Some(0x1234));
        assert_eq!(u32_at(&[0x78, 0x56, 0x34, 0x12], 0), Some(0x1234_5678));
        assert_eq!(usize_at(&[0x78, 0x56, 0x34, 0x12], 0), Some(0x1234_5678));
        assert_eq!(bytes_at(b"abcd", 1, 2), Some(b"bc".as_slice()));
    }

    #[test]
    fn missing_sections_answer_none() {
        assert_eq!(section(0), None);
        assert_eq!(section(u32::MAX), None);
    }

    #[test]
    fn the_table_reports_its_build_version() {
        assert_eq!(version(), crate::UNICODE_VERSION);
    }
}
