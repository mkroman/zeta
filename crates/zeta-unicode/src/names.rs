//! The stored character names, decoded once from the table's three streams.
//!
//! The generator front-codes every name against its code point predecessor and splits the
//! result into three PPMd8-I streams: the shared prefix lengths, the shared suffix lengths and
//! the NUL-terminated middles. Names that generated code can rebuild (see `zeta-unicode-build`,
//! which turns `data/derived-names.rules` into the tree `include!`d below) are left out of the
//! streams but keep their id, so they stay valid predecessors.
//!
//! Decoding happens once, on the first name lookup, and produces a flat `String` of
//! concatenated names plus a `Vec<u32>` of offsets. A lookup then binary-searches the run table
//! for the name id and slices the text: no per-lookup decompression, no allocation, and no
//! repeated UTF-8 validation.

use std::io::Read;
use std::sync::OnceLock;

use zeta_unicode_format::{
    CODEC_PPMD8I, NAMES_HEADER_LEN, NAMES_MAGIC, NAMES_VERSION, SECTION_PREFIX_LEN, STREAM_COUNT,
    STREAM_DESC_LEN, TRANSFORM_DELTA8, TRANSFORM_RAW, read_varint, undelta8,
};

use crate::sections;
use crate::tables;

/// The decoded names.
struct Names {
    /// Every stored name, concatenated and validated as UTF-8.
    text: String,
    /// Start offset of each name, with the total length appended.
    offsets: Vec<u32>,
    /// Runs of consecutive named code points, sorted by start.
    runs: Vec<Run>,
}

/// One run of consecutive named code points: `(start, end, first name id)`.
#[derive(Clone, Copy)]
struct Run {
    start: u32,
    end: u32,
    id: u32,
}

/// The decoded table, built on first use.
static NAMES: OnceLock<Names> = OnceLock::new();

/// Decodes the name streams now, so the first lookup does not pay for it.
///
/// Idempotent and cheap once the table exists.
pub fn init() {
    let _ = NAMES.get_or_init(decode);
}

/// Returns the stored name of `cp`, if it has one.
///
/// Answering a name costs a binary search over the run table plus two `u32` loads.
#[must_use]
pub fn name_for(cp: u32) -> Option<&'static str> {
    let names = NAMES.get_or_init(decode);
    let index = names.runs.partition_point(|run| run.end <= cp);
    let run = names.runs.get(index)?;
    if cp < run.start {
        return None;
    }
    let id = run.id.checked_add(cp - run.start)?;
    let id = usize::try_from(id).ok()?;
    let start = usize::try_from(*names.offsets.get(id)?).ok()?;
    let end = usize::try_from(*names.offsets.get(id.checked_add(1)?)?).ok()?;
    names.text.get(start..end)
}

/// Reads the run table into fixed-width records.
///
/// The section is a `u32` raw length followed by a PPMd8-I stream of `varint(len), varint(gap)`
/// pairs in ascending code point order, where `gap` is the distance from the end of the previous
/// run, so a walk yields every code point that has a name and the name id it maps to.
fn parse_runs(section: &[u8]) -> Vec<Run> {
    let buf = decompress(section);
    let mut runs = Vec::new();
    let mut offset = 0usize;
    let mut start = 0u32;
    let mut id = 0u32;
    while offset < buf.len() {
        let length = varint(&buf, &mut offset).expect("a varint per run");
        let gap = varint(&buf, &mut offset).expect("a varint per run");
        assert!(length > 0, "an empty name run");
        start = start.checked_add(gap).expect("run starts fit in u32");
        let end = start.checked_add(length).expect("run ends fit in u32");
        runs.push(Run { start, end, id });
        id = id.checked_add(length).expect("name ids fit in u32");
        start = end;
    }
    runs
}

/// Reads an LEB128 varint, advancing `offset` past it.
fn varint(buf: &[u8], offset: &mut usize) -> Option<u32> {
    let (value, used) = read_varint(buf.get(*offset..)?)?;
    *offset += used;
    Some(value)
}

/// The parsed and validated `UNNM` container header.
struct Header {
    /// The number of name entries, derived names included.
    entries: u32,
    /// The number of entries rebuilt by the generated rule tree.
    derived: u32,
    /// The transform id of each of the three streams.
    transforms: [u8; STREAM_COUNT],
}

/// Parses the container header and validates the codec, version and transforms.
fn parse_header(section: &[u8]) -> Header {
    assert!(
        section.len() >= NAMES_HEADER_LEN,
        "the name section is truncated"
    );
    assert_eq!(
        section.get(..4),
        Some(NAMES_MAGIC.as_slice()),
        "the name section is not a UNNM container"
    );
    assert_eq!(
        section.get(4).copied(),
        Some(NAMES_VERSION),
        "unsupported container version"
    );
    assert_eq!(
        section.get(5).copied(),
        Some(CODEC_PPMD8I),
        "unsupported name codec"
    );

    let transforms = [section[6], section[7], section[8]];
    for (index, &transform) in transforms.iter().enumerate() {
        assert!(
            matches!(transform, TRANSFORM_RAW | TRANSFORM_DELTA8),
            "unknown transform {transform} on name stream {index}"
        );
    }
    assert_eq!(
        transforms[2], TRANSFORM_RAW,
        "the middle stream is not expected to be transformed"
    );

    let entries = read_u32(section, 9);
    let derived = read_u32(section, 13);
    assert!(derived <= entries, "more derived names than entries");

    Header {
        entries,
        derived,
        transforms,
    }
}

/// Rebuilds every name from the header and the three decoded streams.
fn rebuild_names(header: &Header, streams: [Vec<u8>; STREAM_COUNT]) -> (String, Vec<u32>) {
    let (entries, derived) = (header.entries, header.derived);
    let [prefixes, suffixes, middles] = streams;

    let mut text: Vec<u8> = Vec::with_capacity(920_000);
    let mut derived_name = String::new();
    let mut offsets: Vec<u32> = Vec::with_capacity(entries as usize + 1);
    // The previous name, copied out because it is the only source both of the prefix and of the
    // suffix, and `text` is being appended to while it is read.
    let mut previous: Vec<u8> = Vec::new();
    let mut middle = 0usize;
    // The length streams carry one record per *stored* name: derived names take no record, so
    // they are skipped rather than consuming one.
    let mut stored = 0usize;
    let mut derived_seen = 0u32;

    for id in 0..entries {
        let start = text.len();
        offsets.push(u32::try_from(start).expect("names fit in u32"));

        // A derived name is not stored, but it is still the predecessor the next name is coded
        // against, so `previous` is updated either way.
        derived_name.clear();
        if derive::derive_at(id, &mut derived_name) {
            derived_seen += 1;
            text.extend_from_slice(derived_name.as_bytes());
        } else {
            let prefix = usize::from(*prefixes.get(stored).expect("a prefix per stored name"));
            let suffix = usize::from(*suffixes.get(stored).expect("a suffix per stored name"));
            stored += 1;
            let length = middles
                .get(middle..)
                .and_then(|rest| rest.iter().position(|&byte| byte == 0))
                .expect("every middle is NUL terminated");
            let mid = middles
                .get(middle..middle + length)
                .expect("a middle inside the stream");
            middle += length + 1;

            text.extend_from_slice(
                previous
                    .get(..prefix)
                    .expect("the shared prefix is inside the previous name"),
            );
            text.extend_from_slice(mid);
            text.extend_from_slice(
                previous
                    .get(previous.len().saturating_sub(suffix)..)
                    .expect("the shared suffix is inside the previous name"),
            );
        }

        previous.clear();
        previous.extend_from_slice(&text[start..]);
    }
    offsets.push(u32::try_from(text.len()).expect("names fit in u32"));
    assert_eq!(middle, middles.len(), "every middle must be consumed");
    assert_eq!(stored, prefixes.len(), "every prefix must be consumed");
    assert_eq!(stored, suffixes.len(), "every suffix must be consumed");
    assert_eq!(
        derived_seen, derived,
        "the header's derived count must match the rebuilt names"
    );

    (
        String::from_utf8(text).expect("the generated names are UTF-8"),
        offsets,
    )
}

/// Decodes the streams and rebuilds every name.
fn decode() -> Names {
    let section = tables::section(sections::NAMES).expect("the table has a name section");
    let header = parse_header(section);
    let streams = decode_streams(section, header.transforms);
    let (text, offsets) = rebuild_names(&header, streams);
    let runs = parse_runs(tables::section(sections::NAME_RUNS).expect("the table has a run table"));
    Names {
        text,
        offsets,
        runs,
    }
}

/// Decompresses the three name streams: prefix lengths, suffix lengths and middles.
///
/// Each stream has an eleven-byte descriptor — compressed and raw lengths, model order, memory
/// limit, variant — and all three descriptors come before all three payloads. The declared
/// stream transforms are applied here, so every caller sees the decoded values.
fn decode_streams(section: &[u8], transforms: [u8; STREAM_COUNT]) -> [Vec<u8>; STREAM_COUNT] {
    let mut descriptor = NAMES_HEADER_LEN - STREAM_COUNT * STREAM_DESC_LEN;
    let mut described = [(0usize, 0usize, 0u32, 0u32); STREAM_COUNT];
    for slot in &mut described {
        let compressed = usize::try_from(read_u32(section, descriptor)).expect("length fits");
        let raw = usize::try_from(read_u32(section, descriptor + 4)).expect("length fits");
        let order = u32::from(section[descriptor + 8]);
        let mem = u32::from(section[descriptor + 9]) << 20;
        assert_eq!(
            section[descriptor + 10],
            CODEC_PPMD8I,
            "only PPMd8-I streams are supported"
        );
        descriptor += STREAM_DESC_LEN;
        *slot = (compressed, raw, order, mem);
    }
    assert_eq!(
        descriptor, NAMES_HEADER_LEN,
        "the container header is eleven bytes per stream"
    );

    let mut decoded: [Vec<u8>; STREAM_COUNT] = std::array::from_fn(|_| Vec::new());
    let mut cursor = descriptor;
    for (out, &(compressed, raw, order, mem)) in decoded.iter_mut().zip(&described) {
        let bytes = tables::bytes_at(section, cursor, compressed)
            .expect("the compressed stream is inside the section");
        cursor += compressed;
        *out = decode_stream(bytes, raw, order, mem);
    }
    assert_eq!(cursor, section.len(), "the name section has trailing bytes");
    for (stream, &transform) in decoded.iter_mut().zip(&transforms) {
        if transform == TRANSFORM_DELTA8 {
            undelta8(stream);
        }
    }
    decoded
}

/// Decompresses one PPMd8-I stream to exactly `raw` bytes.
fn decode_stream(bytes: &[u8], raw: usize, order: u32, mem: u32) -> Vec<u8> {
    /// A sanity bound for a corrupt table: the real streams total less than one megabyte.
    const MAX_RAW: usize = 64 * 1024 * 1024;
    assert!(raw <= MAX_RAW, "the stream raw length is implausible");
    let mut decoder =
        ppmd_rust::Ppmd8Decoder::new(bytes, order, mem, ppmd_rust::RestoreMethod::Restart)
            .expect("valid PPMd8 parameters");
    let mut out = vec![0u8; raw];
    decoder
        .read_exact(&mut out)
        .expect("the stream decodes to its recorded length");
    out
}

/// Decompresses a section shaped as `u32 raw length`, `u8 order` and `u8 memory in MiB`,
/// followed by a PPMd8-I stream: the encoding the generator uses for the run table.
fn decompress(section: &[u8]) -> Vec<u8> {
    let raw = tables::usize_at(section, 0).expect("the section records its raw length");
    let order = u32::from(
        section
            .get(4)
            .copied()
            .expect("the section records its order"),
    );
    let mem = u32::from(
        section
            .get(5)
            .copied()
            .expect("the section records its memory limit"),
    );
    decode_stream(
        section
            .get(SECTION_PREFIX_LEN..)
            .expect("the section records its codec prefix"),
        raw,
        order,
        mem << 20,
    )
}

/// Reads the `u32` at `offset` of the container.
fn read_u32(buf: &[u8], offset: usize) -> u32 {
    tables::u32_at(buf, offset).expect("the container header is well formed")
}

/// Appends the upper-case hex of `value` to `out`, zero-padded to `width`.
///
/// The generated derivation tree calls this instead of emitting its own copy of the hex
/// formatter, so a hole is filled the same way as a `*` pattern name.
pub fn push_hex_string(out: &mut String, value: i64, width: u32) {
    let value = u32::try_from(value).expect("a hex hole is non-negative");
    let width = u8::try_from(width).expect("a hex width fits in a byte");
    crate::push_hex(out, value, width);
}

/// The generated rule tree from `zeta-unicode-build`.
mod derive {
    #[allow(
        clippy::all,
        clippy::pedantic,
        clippy::nursery,
        missing_docs,
        unused,
        reason = "generated code is not hand-written"
    )]
    mod tree {
        include!(concat!(env!("OUT_DIR"), "/zeta_unicode_derive.rs"));
    }
    pub use tree::derive_at;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_are_little_endian_base_128() {
        let mut offset = 0;
        assert_eq!(varint(&[0x00], &mut offset), Some(0));
        assert_eq!(offset, 1);

        let mut offset = 0;
        assert_eq!(varint(&[0xAC, 0x02], &mut offset), Some(300));
        assert_eq!(offset, 2);

        let mut offset = 0;
        assert_eq!(
            varint(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F], &mut offset),
            Some(u32::MAX)
        );
        assert_eq!(offset, 5);
    }

    #[test]
    fn truncated_and_overlong_varints_are_rejected() {
        let mut offset = 0;
        assert_eq!(varint(&[], &mut offset), None);
        let mut offset = 0;
        assert_eq!(varint(&[0x80], &mut offset), None);
        // Six payload bytes carry more than 32 bits.
        let mut offset = 0;
        assert_eq!(
            varint(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01], &mut offset),
            None
        );
    }

    #[test]
    fn undelta8_reconstructs_the_cumulative_sum() {
        let mut values = [1, 1, 1];
        undelta8(&mut values);
        assert_eq!(values, [1, 2, 3]);

        let mut values = [0, 0, 0];
        undelta8(&mut values);
        assert_eq!(values, [0, 0, 0]);
    }
}
