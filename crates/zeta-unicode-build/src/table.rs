//! The packed table container: header, section directory and payload packing.

use zeta_unicode_format::{TABLE_ENTRY_LEN, TABLE_HEADER_LEN, TABLE_MAGIC};

pub use zeta_unicode_format::{
    SECTION_EMOJI, SECTION_GC, SECTION_NAME_PATTERNS, SECTION_NAME_RUNS, SECTION_NAMES,
};

/// One section of the table: its id and its payload bytes.
pub struct Section<'a> {
    /// The section id, as the decoder's `sections` module names it.
    pub id: u32,
    /// The payload bytes; offsets inside a section are relative to it.
    pub data: &'a [u8],
}

/// Writes the header, the section directory and the payloads.
#[must_use]
pub fn pack(sections: &[Section<'_>], version: (u8, u8, u8)) -> Vec<u8> {
    let directory_len = TABLE_HEADER_LEN + sections.len() * TABLE_ENTRY_LEN;
    let payload_len: usize = sections.iter().map(|section| section.data.len()).sum();
    let mut table = Vec::with_capacity(directory_len + payload_len);
    table.extend_from_slice(TABLE_MAGIC);
    push_u16(&mut table, u16::from(version.0));
    push_u16(&mut table, u16::from(version.1));
    push_u16(&mut table, u16::from(version.2));
    push_u16(
        &mut table,
        u16::try_from(sections.len()).expect("fewer than 65536 sections"),
    );

    let mut offset = u32::try_from(directory_len).expect("directory fits in u32");
    for section in sections {
        let length = u32::try_from(section.data.len()).expect("section fits in u32");
        push_u32(&mut table, section.id);
        push_u32(&mut table, offset);
        push_u32(&mut table, length);
        offset = offset.checked_add(length).expect("offsets fit in u32");
    }
    for section in sections {
        table.extend_from_slice(section.data);
    }
    table
}

/// Appends `value` little-endian.
pub fn push_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

/// Appends `value` little-endian.
pub fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

/// Appends `value` as a `u16`, naming `what` when it does not fit.
pub fn push_len_u16(buf: &mut Vec<u8>, value: usize, what: &str) {
    push_u16(
        buf,
        u16::try_from(value).unwrap_or_else(|_| panic!("{what} ({value}) does not fit in u16")),
    );
}

/// Appends `value` as a `u32`, naming `what` when it does not fit.
pub fn push_len_u32(buf: &mut Vec<u8>, value: usize, what: &str) {
    push_u32(
        buf,
        u32::try_from(value).unwrap_or_else(|_| panic!("{what} ({value}) does not fit in u32")),
    );
}

// The section ids are shared with the decoder through `zeta-unicode-format`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_the_header_and_directory() {
        let table = pack(
            &[Section {
                id: SECTION_GC,
                data: &[1, 2, 3],
            }],
            (18, 0, 0),
        );
        assert_eq!(&table[..4], TABLE_MAGIC);
        assert_eq!(u16::from_le_bytes([table[4], table[5]]), 18);
        assert_eq!(u16::from_le_bytes([table[10], table[11]]), 1);
        let offset = u32::from_le_bytes(table[16..20].try_into().expect("offset")) as usize;
        assert_eq!(offset, TABLE_HEADER_LEN + TABLE_ENTRY_LEN);
        assert_eq!(&table[offset..], [1, 2, 3]);
    }
}
