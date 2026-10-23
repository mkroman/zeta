//! The `UNNM` name container and the standalone section framing.
//!
//! [`compress_streams`] is the only place the three name streams are handed to the codec;
//! [`build_blob`] lays out the fixed header, the stream directory and the payloads, and
//! [`write_section`] frames and writes the index and diagnostic pattern sections.

use std::borrow::Cow;
use std::path::Path;

use zeta_unicode_format::{
    CODEC_PPMD8I, NAMES_HEADER_LEN as HEADER_LEN, NAMES_MAGIC as MAGIC,
    NAMES_VERSION as BLOB_VERSION, SECTION_PREFIX_LEN, STREAM_COUNT, STREAM_DESC_LEN,
    TRANSFORM_DELTA8, TRANSFORM_RAW, delta8,
};

use super::codec;
use super::rules::StreamParams;
use super::{SectionSize, StreamSize};
use crate::table::push_len_u32;

/// The three compressed name streams and the header pieces that describe them.
pub(super) struct CompressedStreams {
    transforms: Vec<u8>,
    descriptors: Vec<u8>,
    pub(super) sizes: [StreamSize; STREAM_COUNT],
    streams: Vec<Vec<u8>>,
}

/// Transforms (when configured) and compresses the three name streams.
pub(super) fn compress_streams(
    params: &[StreamParams; STREAM_COUNT],
    sources: [&[u8]; STREAM_COUNT],
) -> CompressedStreams {
    let mut transforms = Vec::with_capacity(STREAM_COUNT);
    let mut descriptors = Vec::with_capacity(STREAM_COUNT * STREAM_DESC_LEN);
    let mut sizes = [StreamSize::default(); STREAM_COUNT];
    let mut streams = Vec::with_capacity(STREAM_COUNT);

    for (idx, (params, source)) in params.iter().copied().zip(sources).enumerate() {
        transforms.push(if params.delta8 {
            TRANSFORM_DELTA8
        } else {
            TRANSFORM_RAW
        });
        let source = if params.delta8 {
            Cow::Owned(delta8(source))
        } else {
            Cow::Borrowed(source)
        };
        let compressed = codec::ppmd8i(&source, params.order, params.mem_mib);
        push_len_u32(
            &mut descriptors,
            compressed.len(),
            "compressed stream length",
        );
        push_len_u32(&mut descriptors, source.len(), "stream raw length");
        descriptors.push(u8::try_from(params.order).expect("order fits in a byte"));
        descriptors.push(u8::try_from(params.mem_mib).expect("memory fits in a byte"));
        descriptors.push(CODEC_PPMD8I);
        sizes[idx] = StreamSize {
            raw: source.len(),
            compressed: compressed.len(),
            order: params.order,
            mem_mib: params.mem_mib,
            delta8: params.delta8,
        };
        streams.push(compressed);
    }

    CompressedStreams {
        transforms,
        descriptors,
        sizes,
        streams,
    }
}

/// Writes the `UNNM` container: fixed header, the stream descriptors, then the payloads.
pub(super) fn build_blob(
    entries: usize,
    derived: usize,
    compressed: &CompressedStreams,
) -> Vec<u8> {
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.push(BLOB_VERSION);
    header.push(CODEC_PPMD8I);
    header.extend_from_slice(&compressed.transforms);
    push_len_u32(&mut header, entries, "name entry count");
    push_len_u32(&mut header, derived, "derived name count");
    header.extend_from_slice(&compressed.descriptors);
    assert_eq!(header.len(), HEADER_LEN, "the header layout changed");

    let mut blob = header;
    for stream in &compressed.streams {
        blob.extend_from_slice(stream);
    }
    blob
}

/// Writes one compressed standalone section and returns its bytes and its ledger entry.
pub(super) fn write_section(
    out_dir: &Path,
    file: &str,
    raw: &[u8],
    (order, mem_mib): (u32, u32),
) -> Result<(Vec<u8>, SectionSize), crate::Error> {
    let bytes = codec::section_bytes(raw, order, mem_mib);
    crate::write(out_dir.join(file), &bytes)?;
    let size = SectionSize {
        raw: raw.len(),
        compressed: bytes.len() - SECTION_PREFIX_LEN,
        order,
        mem_mib,
    };
    Ok((bytes, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(delta8: [bool; STREAM_COUNT]) -> [StreamParams; STREAM_COUNT] {
        let mut out = [StreamParams {
            delta8: false,
            order: 10,
            mem_mib: 1,
        }; STREAM_COUNT];
        for (slot, delta8) in out.iter_mut().zip(delta8) {
            slot.delta8 = delta8;
        }
        out
    }

    #[test]
    fn the_blob_header_describes_the_three_streams() {
        let params = params([true, false, false]);
        let sources = [b"abc".as_slice(), b"de".as_slice(), b"".as_slice()];
        let compressed = compress_streams(&params, sources);
        let blob = build_blob(42, 7, &compressed);

        assert_eq!(&blob[..4], MAGIC);
        assert_eq!(blob[4], BLOB_VERSION);
        assert_eq!(blob[5], CODEC_PPMD8I);
        assert_eq!(
            &blob[6..9],
            &[TRANSFORM_DELTA8, TRANSFORM_RAW, TRANSFORM_RAW]
        );
        assert_eq!(u32::from_le_bytes(blob[9..13].try_into().unwrap()), 42);
        assert_eq!(u32::from_le_bytes(blob[13..17].try_into().unwrap()), 7);

        let mut off = HEADER_LEN - STREAM_COUNT * STREAM_DESC_LEN;
        for (size, source) in compressed.sizes.iter().zip(sources) {
            assert_eq!(
                u32::from_le_bytes(blob[off..off + 4].try_into().unwrap()) as usize,
                size.compressed
            );
            assert_eq!(
                u32::from_le_bytes(blob[off + 4..off + 8].try_into().unwrap()) as usize,
                source.len()
            );
            assert_eq!(usize::from(blob[off + 8]), size.order as usize);
            assert_eq!(usize::from(blob[off + 9]), size.mem_mib as usize);
            assert_eq!(blob[off + 10], CODEC_PPMD8I);
            off += STREAM_DESC_LEN;
        }
        assert_eq!(off, HEADER_LEN);
        let payload: usize = compressed.streams.iter().map(Vec::len).sum();
        assert_eq!(blob.len(), HEADER_LEN + payload);
        assert_eq!(
            &blob[HEADER_LEN..HEADER_LEN + compressed.streams[0].len()],
            compressed.streams[0].as_slice()
        );
    }

    #[test]
    fn a_section_prefix_records_the_raw_length_and_codec() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (bytes, size) =
            write_section(dir.path(), "section.bin", b"abc", (9, 2)).expect("write");

        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 3);
        assert_eq!(bytes[SECTION_PREFIX_LEN - 2], 9);
        assert_eq!(bytes[SECTION_PREFIX_LEN - 1], 2);
        assert_eq!(size.raw, 3);
        assert_eq!(size.compressed, bytes.len() - SECTION_PREFIX_LEN);
        assert_eq!(
            std::fs::read(dir.path().join("section.bin")).unwrap(),
            bytes
        );
    }
}
