//! Independent decoders for the emitted container and standalone sections.
//!
//! These functions read the bytes back without touching the runtime decoder, so a
//! disagreement between the writer and this module is caught during generation. The
//! verification checks that consume them live in the parent module.

use zeta_unicode_format::{
    CODEC_PPMD8I, NAMES_HEADER_LEN as HEADER_LEN, NAMES_MAGIC as MAGIC,
    NAMES_VERSION as BLOB_VERSION, SECTION_PREFIX_LEN, STREAM_COUNT, STREAM_DESC_LEN,
    TRANSFORM_DELTA8, TRANSFORM_RAW, undelta8,
};

use crate::names::STREAMS;
use crate::names::codec::{SectionCodec, section_codec};
use crate::names::corpus::Pattern;
use crate::names::sections::PATTERN_BASE;

/// The three decompressed name streams.
#[derive(Clone, Debug)]
pub struct Streams {
    /// Shared prefix length per stored name.
    pub plen: Vec<u8>,
    /// Shared suffix length per stored name.
    pub slen: Vec<u8>,
    /// NUL-terminated middles.
    pub mids: Vec<u8>,
    /// Number of entries the blob describes.
    pub entries: usize,
    /// Number of entries the header says are derived.
    pub derived: usize,
}

/// The fixed `UNNM` header, parsed once before the stream directory.
struct Header {
    /// Transform id per stream, in container order.
    transforms: [u8; STREAM_COUNT],
    /// Number of entries the blob describes.
    entries: usize,
    /// Number of entries the header says are derived.
    derived: usize,
}

impl Header {
    /// Parses and validates the fixed header; the descriptors and payloads follow it.
    fn parse(blob: &[u8]) -> Self {
        assert!(blob.len() >= HEADER_LEN, "names.bin is truncated");
        assert_eq!(&blob[..4], MAGIC, "names.bin magic");
        assert_eq!(blob[4], BLOB_VERSION, "names.bin version");
        assert_eq!(blob[5], CODEC_PPMD8I, "names.bin codec is not PPMd8-I");
        Self {
            transforms: [blob[6], blob[7], blob[8]],
            entries: u32_at(blob, 9, "names.bin entry count") as usize,
            derived: u32_at(blob, 13, "names.bin derived count") as usize,
        }
    }
}

/// Reads one stream descriptor at `off`:
/// `(compressed length, raw length, order, memory bytes)`.
fn stream_descriptor(blob: &[u8], off: usize, name: &str) -> (usize, usize, u32, u32) {
    let compressed = u32_at(blob, off, name) as usize;
    let raw = u32_at(blob, off + 4, name) as usize;
    assert_eq!(
        blob[off + 10],
        CODEC_PPMD8I,
        "stream {name}: only PPMd8 variant I is supported"
    );
    (
        compressed,
        raw,
        u32::from(blob[off + 8]),
        u32::from(blob[off + 9]) << 20,
    )
}

/// Decompresses a `names.bin` exactly as the runtime decoder does.
#[must_use]
pub fn decode_blob(blob: &[u8]) -> Streams {
    let header = Header::parse(blob);
    let mut off = HEADER_LEN - STREAM_COUNT * STREAM_DESC_LEN;
    // All three descriptors come first, then the three streams: read the
    // directory before slicing any payload.
    let mut descriptors = [(0usize, 0usize, 0u32, 0u32); STREAM_COUNT];
    for (idx, slot) in descriptors.iter_mut().enumerate() {
        *slot = stream_descriptor(blob, off, STREAMS[idx]);
        off += STREAM_DESC_LEN;
    }
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(STREAM_COUNT);
    for (idx, name) in STREAMS.iter().enumerate() {
        let (comp, raw, order, mem) = descriptors[idx];
        let mut buffer = vec![0u8; raw];
        let mut decoder = ppmd_rust::Ppmd8Decoder::new(
            &blob[off..off + comp],
            order,
            mem,
            ppmd_rust::RestoreMethod::Restart,
        )
        .unwrap_or_else(|e| panic!("stream {name}: ppmd decoder: {e:?}"));
        std::io::Read::read_exact(&mut decoder, &mut buffer).unwrap_or_else(|e| {
            panic!(
                "stream {name}: ppmd decode: {e} (comp {comp} raw {raw} order {order} \
                 mem {mem} transform {})",
                header.transforms[idx]
            )
        });
        off += comp;
        if header.transforms[idx] == TRANSFORM_DELTA8 {
            undelta8(&mut buffer);
        } else {
            assert_eq!(
                header.transforms[idx], TRANSFORM_RAW,
                "stream {name}: unknown transform"
            );
        }
        out.push(buffer);
    }
    assert_eq!(off, blob.len(), "names.bin has trailing bytes");
    let mut streams = out.into_iter();
    Streams {
        plen: streams.next().expect("plen"),
        slen: streams.next().expect("slen"),
        mids: streams.next().expect("mids"),
        entries: header.entries,
        derived: header.derived,
    }
}

/// One-time table construction: walk the entry ids in ascending order, take
/// derived names from `derive` and front-decode the rest against the previous
/// name, which may itself have been derived.
///
/// This is the whole runtime contract: a flat `Vec<u8>` of name text plus one
/// `u32` offset per name.
pub fn reconstruct<F>(streams: &Streams, mut derive: F) -> (Vec<u8>, Vec<u32>)
where
    F: FnMut(u32, &mut Vec<u8>) -> bool,
{
    let n = streams.entries;
    let mut text: Vec<u8> = Vec::with_capacity(920_000);
    let mut offsets: Vec<u32> = Vec::with_capacity(n + 1);
    offsets.push(0);
    let (mut pi, mut si, mut mi) = (0usize, 0usize, 0usize);
    let (mut prev_start, mut prev_len) = (0usize, 0usize);
    for id in 0..n {
        let start = text.len();
        if derive(u32::try_from(id).expect("id fits"), &mut text) {
            // nothing to consume: a derived name is absent from the streams
        } else {
            let p = *streams
                .plen
                .get(pi)
                .unwrap_or_else(|| panic!("plen overrun at id {id}")) as usize;
            pi += 1;
            let s = *streams
                .slen
                .get(si)
                .unwrap_or_else(|| panic!("slen overrun at id {id}")) as usize;
            si += 1;
            let end = streams.mids[mi..]
                .iter()
                .position(|&b| b == 0)
                .map_or_else(|| panic!("unterminated middle at id {id}"), |p| p + mi);
            let prev = text[prev_start..prev_start + prev_len].to_vec();
            assert!(
                p <= prev.len() && s <= prev.len() - p,
                "bad front-code record at id {id}"
            );
            text.extend_from_slice(&prev[..p]);
            text.extend_from_slice(&streams.mids[mi..end]);
            text.extend_from_slice(&prev[prev.len() - s..]);
            mi = end + 1;
        }
        prev_start = start;
        prev_len = text.len() - start;
        offsets.push(u32::try_from(text.len()).expect("name text fits in u32"));
    }
    assert_eq!(pi, streams.plen.len(), "plen record count");
    assert_eq!(si, streams.slen.len(), "slen record count");
    assert_eq!(mi, streams.mids.len(), "mids record count");
    (text, offsets)
}

/// The name index: `(start, len, base_name_id)` per run of consecutive code
/// points.
pub type Runs = Vec<(u32, u32, u32)>;

/// Binary search over the runs: `cp` → name id.  The decoder's hot path.
#[must_use]
pub fn name_id(runs: &Runs, cp: u32) -> Option<u32> {
    let index = runs
        .partition_point(|(start, ..)| *start <= cp)
        .checked_sub(1)?;
    let (start, len, base) = runs[index];
    (cp < start + len).then_some(base + (cp - start))
}

/// Expands the shipped varint run stream into `(start, len, base)` triples.
#[must_use]
pub fn decode_runs(raw: &[u8]) -> Runs {
    let mut out: Runs = Vec::new();
    let mut cursor = 0u64;
    let mut at = 0usize;
    let mut previous_end = 0u64;
    while at < raw.len() {
        let (len, n) = read_varint(&raw[at..]);
        let (gap, m) = read_varint(&raw[at + n..]);
        at += n + m;
        assert!(len > 0, "zero-length run");
        let start = if out.is_empty() {
            u64::from(gap)
        } else {
            previous_end + u64::from(gap)
        };
        out.push((
            u32::try_from(start).expect("code point fits"),
            len,
            u32::try_from(cursor).expect("index fits"),
        ));
        cursor += u64::from(len);
        previous_end = start + u64::from(len);
    }
    for pair in out.windows(2) {
        assert!(
            pair[0].0 + pair[0].1 <= pair[1].0,
            "index runs overlap near U+{:04X}",
            pair[1].0
        );
    }
    out
}

/// Reads the shipped varint pattern stream back into [`Pattern`]s.
#[must_use]
pub fn decode_patterns(raw: &[u8]) -> Vec<Pattern> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < raw.len() {
        let (lo, n) = read_varint(&raw[at..]);
        let (len, m) = read_varint(&raw[at + n..]);
        at += n + m;
        assert!(len > 0, "pattern record has zero length");
        let tail = &raw[at..];
        let first = tail
            .iter()
            .position(|&b| b == b'|')
            .unwrap_or_else(|| panic!("pattern record without a separator"));
        let prefix = std::str::from_utf8(&tail[..first])
            .expect("pattern prefix is UTF-8")
            .to_owned();
        let rest = &tail[first + 1..];
        let second = rest
            .iter()
            .position(|&b| b == b'|')
            .unwrap_or_else(|| panic!("pattern record without a second separator"));
        let suffix = std::str::from_utf8(&rest[..second])
            .expect("pattern suffix is UTF-8")
            .to_owned();
        at += first + 1 + second + 1;
        out.push(Pattern {
            lo: PATTERN_BASE + lo,
            hi: PATTERN_BASE + lo + len - 1,
            prefix,
            suffix,
        });
    }
    out
}

fn read_varint(bytes: &[u8]) -> (u32, usize) {
    zeta_unicode_format::read_varint(bytes).unwrap_or_else(|| panic!("truncated varint"))
}

/// Decompresses a standalone section (`u32` raw length + PPMd8-I stream).
pub fn decode_section(bytes: &[u8], codec: SectionCodec) -> Vec<u8> {
    assert!(bytes.len() > SECTION_PREFIX_LEN, "truncated section");
    let raw = u32_at(bytes, 0, "section raw length") as usize;
    // The parameters are in the section; `codec` is only the caller's expectation, checked here
    // so a stale decoder fails loudly instead of producing garbage.
    let (order, mem_mib) = section_codec(bytes);
    assert_eq!(
        (order, mem_mib),
        (codec.order, codec.mem_mib),
        "section codec parameters"
    );
    let mut out = vec![0u8; raw];
    let mut decoder = ppmd_rust::Ppmd8Decoder::new(
        &bytes[SECTION_PREFIX_LEN..],
        order,
        mem_mib << 20,
        ppmd_rust::RestoreMethod::Restart,
    )
    .expect("ppmd decoder for a section");
    std::io::Read::read_exact(&mut decoder, &mut out).expect("ppmd section");
    out
}

/// The decoded name at `id`, from the flat text and its offset table.
pub(super) fn name_at<'a>(text: &'a [u8], offsets: &[u32], id: usize) -> &'a [u8] {
    &text[usize::try_from(offsets[id]).expect("offset fits")
        ..usize::try_from(offsets[id + 1]).expect("offset fits")]
}

/// Reads a little-endian `u32` at `at`, naming the field through `what`.
pub(super) fn u32_at(buf: &[u8], at: usize, what: &str) -> u32 {
    let bytes = buf
        .get(at..at + 4)
        .unwrap_or_else(|| panic!("{what} is outside the buffer"));
    u32::from_le_bytes(bytes.try_into().expect("four bytes"))
}

/// Reads a little-endian `u16` at `at`, naming the field through `what`.
pub(super) fn u16_at(buf: &[u8], at: usize, what: &str) -> u16 {
    let bytes = buf
        .get(at..at + 2)
        .unwrap_or_else(|| panic!("{what} is outside the buffer"));
    u16::from_le_bytes(bytes.try_into().expect("two bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::codec::varint;
    use crate::names::container::{build_blob, compress_streams};
    use crate::names::rules::StreamParams;
    use crate::names::sections::{patterns_stream, runs_stream};

    fn stream_params() -> [StreamParams; STREAM_COUNT] {
        [StreamParams {
            delta8: false,
            order: 10,
            mem_mib: 1,
        }; STREAM_COUNT]
    }

    fn sample_blob() -> Vec<u8> {
        let compressed = compress_streams(&stream_params(), [b"", b"", b""]);
        build_blob(0, 0, &compressed)
    }

    fn entries(pairs: &[(u32, &str)]) -> Vec<(u32, String)> {
        pairs
            .iter()
            .map(|(cp, name)| (*cp, (*name).to_owned()))
            .collect()
    }

    #[test]
    #[should_panic = "names.bin magic"]
    fn rejects_bad_container_magic() {
        let mut blob = sample_blob();
        blob[0] ^= 0xFF;
        let _ = decode_blob(&blob);
    }

    #[test]
    #[should_panic = "names.bin version"]
    fn rejects_bad_container_version() {
        let mut blob = sample_blob();
        blob[4] += 1;
        let _ = decode_blob(&blob);
    }

    #[test]
    #[should_panic = "names.bin codec is not PPMd8-I"]
    fn rejects_bad_container_codec() {
        let mut blob = sample_blob();
        blob[5] ^= 0xFF;
        let _ = decode_blob(&blob);
    }

    #[test]
    #[should_panic = "only PPMd8 variant I is supported"]
    fn rejects_bad_stream_codec() {
        let mut blob = sample_blob();
        let first_descriptor = HEADER_LEN - STREAM_COUNT * STREAM_DESC_LEN;
        blob[first_descriptor + 10] ^= 0xFF;
        let _ = decode_blob(&blob);
    }

    #[test]
    #[should_panic = "unknown transform"]
    fn rejects_unknown_stream_transform() {
        let mut blob = sample_blob();
        blob[6] = 0xFF;
        let _ = decode_blob(&blob);
    }

    #[test]
    #[should_panic = "names.bin is truncated"]
    fn rejects_truncated_container() {
        let blob = sample_blob();
        let _ = decode_blob(&blob[..HEADER_LEN - 1]);
    }

    #[test]
    #[should_panic = "truncated section"]
    fn rejects_truncated_section() {
        let _ = decode_section(
            &[0u8; SECTION_PREFIX_LEN],
            SectionCodec {
                order: 10,
                mem_mib: 1,
            },
        );
    }

    #[test]
    fn runs_round_trip() {
        let entries = entries(&[
            (0x41, "A"),
            (0x42, "B"),
            (0x45, "E"),
            (0x46, "F"),
            (0x47, "G"),
        ]);
        let (raw, count) = runs_stream(&entries);
        assert_eq!(count, 2);
        assert_eq!(
            decode_runs(&raw),
            vec![(0x41, 2, 0), (0x45, 3, 2)],
            "the runs must carry absolute starts and running base ids"
        );
    }

    #[test]
    #[should_panic = "zero-length run"]
    fn rejects_zero_length_run() {
        let mut raw = Vec::new();
        varint(0, &mut raw);
        varint(0x41, &mut raw);
        let _ = decode_runs(&raw);
    }

    #[test]
    fn patterns_round_trip() {
        let patterns = vec![
            Pattern {
                lo: PATTERN_BASE,
                hi: PATTERN_BASE + 1,
                prefix: "CJK UNIFIED IDEOGRAPH-".to_owned(),
                suffix: String::new(),
            },
            Pattern {
                lo: 0x4E00,
                hi: 0x4E00,
                prefix: "PREFIX ".to_owned(),
                suffix: " SUFFIX".to_owned(),
            },
        ];
        let raw = patterns_stream(&patterns);
        assert_eq!(decode_patterns(&raw), patterns);
    }

    #[test]
    #[should_panic = "pattern record has zero length"]
    fn rejects_zero_length_pattern_record() {
        let mut raw = Vec::new();
        varint(0, &mut raw);
        varint(0, &mut raw);
        let _ = decode_patterns(&raw);
    }

    #[test]
    fn reconstructs_across_a_derived_predecessor() {
        let streams = Streams {
            plen: vec![0, 0],
            slen: vec![0, 3],
            mids: b"FOO BAR\0QUX \0".to_vec(),
            entries: 3,
            derived: 1,
        };
        let (text, offsets) = reconstruct(&streams, |id, out| {
            if id == 1 {
                out.extend_from_slice(b"FOO BAZ");
                true
            } else {
                false
            }
        });
        assert_eq!(
            [
                name_at(&text, &offsets, 0),
                name_at(&text, &offsets, 1),
                name_at(&text, &offsets, 2),
            ],
            [
                b"FOO BAR".as_slice(),
                b"FOO BAZ".as_slice(),
                b"QUX BAZ".as_slice()
            ]
        );
    }
}
