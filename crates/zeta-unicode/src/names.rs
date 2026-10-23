//! The stored character names, decoded once from the table's three streams.
//!
//! The generator front-codes every name against its code point predecessor and splits the
//! result into three PPMd8-I streams: the shared prefix lengths, the shared suffix lengths and
//! the NUL-terminated middles. Names that generated code can rebuild (see `zeta-unicode-build`,
//! which turns `data/derived-names.rules` into the tree `include!`d below) are left out of the
//! streams but keep their id, so they stay valid predecessors.
//!
//! Decoding happens once, on the first name lookup, and produces a flat `Vec<u8>` of
//! concatenated names plus a `Vec<u32>` of offsets. A lookup then binary-searches the run table
//! for the name id and slices the text: no per-lookup decompression, no allocation.

use std::io::Read;
use std::sync::OnceLock;

use crate::sections;
use crate::tables;

/// The decoded names.
struct Names {
    /// Every stored name, concatenated.
    text: Vec<u8>,
    /// Start offset of each name, with the total length appended.
    offsets: Vec<u32>,
    /// Runs of consecutive named code points, sorted by start.
    runs: Vec<Run>,
}

/// One run of consecutive named code points: `(start, length, first name id)`.
#[derive(Clone, Copy)]
struct Run {
    start: u32,
    length: u32,
    id: u32,
}

/// The decoded table, built on first use.
static NAMES: OnceLock<Names> = OnceLock::new();

/// Returns the stored name of `cp`, if it has one.
///
/// Answering a name costs a binary search over the run table plus two `u32` loads.
#[must_use]
pub fn name_for(cp: u32) -> Option<&'static str> {
    let names = NAMES.get_or_init(decode);
    let index = names
        .runs
        .partition_point(|run| run.start + run.length <= cp);
    let run = names.runs.get(index)?;
    if cp < run.start {
        return None;
    }
    let id = run.id + (cp - run.start);
    let start = usize::try_from(names.offsets.get(id as usize).copied()?).ok()?;
    let end = usize::try_from(names.offsets.get(id as usize + 1).copied()?).ok()?;
    std::str::from_utf8(names.text.get(start..end)?).ok()
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
        start += gap;
        runs.push(Run { start, length, id });
        start += length;
        id += length;
    }
    runs
}

/// Reads an LEB128 varint, advancing `offset` past it.
fn varint(buf: &[u8], offset: &mut usize) -> Option<u32> {
    let mut value = 0u32;
    let mut shift = 0u32;
    loop {
        let byte = *buf.get(*offset)?;
        *offset += 1;
        value |= u32::from(byte & 0x7F).checked_shl(shift)?;
        if byte < 0x80 {
            return Some(value);
        }
        shift += 7;
        if shift > 28 {
            return None;
        }
    }
}

/// Decodes the streams and rebuilds every name.
fn decode() -> Names {
    let version = tables::validate();
    assert_eq!(
        version,
        crate::UNICODE_VERSION,
        "the generated table was built from Unicode {version:?}"
    );
    let section = tables::section(sections::NAMES).expect("the table has a name section");
    assert_eq!(
        section.get(..4),
        Some(b"UNNM".as_slice()),
        "the name section is not a UNNM container"
    );
    assert_eq!(
        section.get(4).copied(),
        Some(1),
        "unsupported container version"
    );

    let transform = [section[6], section[7], section[8]];
    let entries = read_u32(section, 9);
    let decoded = decode_streams(section);

    let mut prefixes = decoded[0].clone();
    let mut suffixes = decoded[1].clone();
    let middles = decoded[2].clone();
    if transform[0] == 1 {
        undelta8(&mut prefixes);
    }
    if transform[1] == 1 {
        undelta8(&mut suffixes);
    }
    assert_eq!(
        transform[2], 0,
        "the middle stream is not expected to be transformed"
    );

    let mut text: Vec<u8> = Vec::new();
    let mut derived_name = String::new();
    let mut offsets: Vec<u32> = Vec::with_capacity(entries as usize + 1);
    // The previous name, copied out because it is the only source both of the prefix and of the
    // suffix, and `text` is being appended to while it is read.
    let mut previous: Vec<u8> = Vec::new();
    let mut middle = 0usize;
    // The length streams carry one record per *stored* name: derived names take no record, so
    // they are skipped rather than consuming one.
    let mut stored = 0usize;

    for id in 0..entries {
        let start = text.len();
        offsets.push(u32::try_from(start).expect("names fit in u32"));

        // A derived name is not stored, but it is still the predecessor the next name is coded
        // against, so `previous` is updated either way.
        derived_name.clear();
        if derive::derive_at(id, &mut derived_name) {
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

            text.extend_from_slice(previous.get(..prefix.min(previous.len())).unwrap_or(&[]));
            text.extend_from_slice(mid);
            text.extend_from_slice(&previous[previous.len().saturating_sub(suffix)..]);
        }

        previous.clear();
        previous.extend_from_slice(&text[start..]);
    }
    offsets.push(u32::try_from(text.len()).expect("names fit in u32"));
    let runs = parse_runs(tables::section(sections::NAME_RUNS).expect("the table has a run table"));

    debug_assert_eq!(middle, middles.len(), "every middle must be consumed");
    Names {
        text,
        offsets,
        runs,
    }
}

/// Decompresses the three name streams: prefix lengths, suffix lengths and middles.
///
/// Each stream has an eleven-byte descriptor — compressed and raw lengths, model order, memory
/// limit, variant — and all three descriptors come before all three payloads.
fn decode_streams(section: &[u8]) -> [Vec<u8>; 3] {
    let mut descriptor = 17;
    let mut described = [(0usize, 0usize, 0u32, 0u32); 3];
    for slot in &mut described {
        let compressed = usize::try_from(read_u32(section, descriptor)).expect("length fits");
        let raw = usize::try_from(read_u32(section, descriptor + 4)).expect("length fits");
        let order = u32::from(section[descriptor + 8]);
        let mem = u32::from(section[descriptor + 9]) << 20;
        assert_eq!(
            section[descriptor + 10],
            1,
            "only PPMd8-I streams are supported"
        );
        descriptor += 11;
        *slot = (compressed, raw, order, mem);
    }
    assert_eq!(
        descriptor, 50,
        "the container header is eleven bytes per stream"
    );

    let mut decoded: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut cursor = descriptor;
    for (out, &(compressed, raw, order, mem)) in decoded.iter_mut().zip(&described) {
        let bytes = tables::bytes_at(section, cursor, compressed).expect("stream inside section");
        cursor += compressed;
        *out = decode_stream(bytes, raw, order, mem);
    }
    decoded
}

/// Decompresses one PPMd8-I stream to exactly `raw` bytes.
fn decode_stream(bytes: &[u8], raw: usize, order: u32, mem: u32) -> Vec<u8> {
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
    decode_stream(section.get(6..).unwrap_or_default(), raw, order, mem << 20)
}

/// Turns delta-8 coding back into the values it coded.
fn undelta8(values: &mut [u8]) {
    let mut previous = 0u8;
    for value in values.iter_mut() {
        *value = value.wrapping_add(previous);
        previous = *value;
    }
}

/// Reads the `u32` at `offset` of the container.
fn read_u32(buf: &[u8], offset: usize) -> u32 {
    tables::u32_at(buf, offset).expect("the container header is well formed")
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
