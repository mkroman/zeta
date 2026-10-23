//! The generated table: its header, section directory and bounds-checked readers.
//!
//! `zeta-unicode-build` writes one binary file into `OUT_DIR` and the crate links it in with
//! `include_bytes!`, so the table costs no filesystem or startup work of its own.

/// The generated table, linked in at compile time.
pub static TABLE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/zeta_unicode_tables.bin"));

/// `ZUCD`, the table magic.
const MAGIC: &[u8; 4] = b"ZUCD";
/// Bytes before the section directory: magic plus three version components and a count.
const HEADER_LEN: usize = 4 + 2 * 3 + 2;
/// One directory entry: id, file offset, payload length.
const ENTRY_LEN: usize = 12;

/// The Unicode version the table was built from.
///
/// The decoder compares it with the crate's `UNICODE_VERSION`, so a table built from a different
/// UCD release cannot slip through unnoticed.
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
        let record = HEADER_LEN + index * ENTRY_LEN;
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

/// Checks the table magic and returns its Unicode version.
///
/// A table built from a different generator or a different UCD release is a build mistake, not a
/// runtime condition, so this runs while the name streams are decoded rather than on a lookup.
///
/// # Panics
///
/// When the magic does not match, or the table was built from another Unicode release than the
/// crate declares.
pub fn validate() -> (u8, u8, u8) {
    assert_eq!(
        TABLE.get(..4),
        Some(MAGIC.as_slice()),
        "the generated table is not a zeta-unicode table"
    );
    version()
}
