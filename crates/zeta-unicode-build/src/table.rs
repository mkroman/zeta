//! The packed table container: header, section directory and payload packing.

/// Magic at the start of the generated table.
const MAGIC: &[u8; 4] = b"ZUCD";
/// Length of the fixed header: magic, three version components and a section count.
const HEADER_LEN: usize = 4 + 2 * 3 + 2;
/// Length of one section directory entry: id, file offset, payload length.
const ENTRY_LEN: usize = 12;

/// The name index section.
pub const SECTION_NAME_RUNS: u32 = 1;
/// The `*`-pattern ranges section.
pub const SECTION_NAME_PATTERNS: u32 = 2;
/// The general category section.
pub const SECTION_GC: u32 = 3;
/// The emoji section.
pub const SECTION_EMOJI: u32 = 4;
/// The name streams section.
pub const SECTION_NAMES: u32 = 5;

/// One section of the table: its id and its payload bytes.
pub struct Section {
    /// The section id, as the decoder's `sections` module names it.
    pub id: u32,
    /// The payload bytes; offsets inside a section are relative to it.
    pub data: Vec<u8>,
}

/// Writes the header, the section directory and the payloads.
pub fn pack(sections: Vec<Section>, version: (u8, u8, u8)) -> Vec<u8> {
    let directory_len = HEADER_LEN + sections.len() * ENTRY_LEN;
    let mut table = Vec::with_capacity(directory_len);
    table.extend_from_slice(MAGIC);
    push_u16(&mut table, u16::from(version.0));
    push_u16(&mut table, u16::from(version.1));
    push_u16(&mut table, u16::from(version.2));
    push_u16(
        &mut table,
        u16::try_from(sections.len()).expect("fewer than 65536 sections"),
    );

    let mut offset = u32::try_from(directory_len).expect("directory fits in u32");
    for section in &sections {
        let length = u32::try_from(section.data.len()).expect("section fits in u32");
        push_u32(&mut table, section.id);
        push_u32(&mut table, offset);
        push_u32(&mut table, length);
        offset = offset.checked_add(length).expect("offsets fit in u32");
    }
    for section in sections {
        table.extend_from_slice(&section.data);
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
