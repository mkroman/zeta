//! Round-trip verification: the emitter re-decodes its own artifacts and checks
//! every claim against the vendored UCD text.
//!
//! The decoder in this module is deliberately separate from the runtime decoder: it is an
//! independent check of the written bytes, while `zeta-unicode`'s own round-trip tests check the
//! runtime side against the same UCD files. Only the format constants are shared, through
//! `zeta-unicode-format`.

use std::path::Path;

use zeta_unicode_format::{
    CODEC_PPMD8I, HANGUL_END, HANGUL_START, NAMES_HEADER_LEN as HEADER_LEN, NAMES_MAGIC as MAGIC,
    NAMES_VERSION as BLOB_VERSION, SECTION_PREFIX_LEN, STREAM_COUNT, STREAM_DESC_LEN,
    TRANSFORM_DELTA8, undelta8,
};

use super::codec::{SectionCodec, section_codec};
use super::corpus::{Corpus, Pattern};
use super::rules::Rules;
use super::{PATTERN_BASE, STREAMS};
use crate::Error;

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

/// Decompresses a `names.bin` exactly as the runtime decoder does.
#[must_use]
pub fn decode_blob(blob: &[u8]) -> Streams {
    assert!(blob.len() >= HEADER_LEN, "names.bin is truncated");
    assert_eq!(&blob[..4], MAGIC, "names.bin magic");
    assert_eq!(blob[4], BLOB_VERSION, "names.bin version");
    assert_eq!(blob[5], CODEC_PPMD8I, "names.bin codec is not PPMd8-I");
    let transforms = [blob[6], blob[7], blob[8]];
    let entries = u32::from_le_bytes(blob[9..13].try_into().unwrap()) as usize;
    let derived = u32::from_le_bytes(blob[13..17].try_into().unwrap()) as usize;
    let mut off = HEADER_LEN - STREAM_COUNT * STREAM_DESC_LEN;
    // All three descriptors come first, then the three streams: read the
    // directory before slicing any payload.
    let mut descriptors = [(0usize, 0usize, 0u32, 0u32); STREAM_COUNT];
    for (idx, slot) in descriptors.iter_mut().enumerate() {
        let comp = u32::from_le_bytes(blob[off..off + 4].try_into().unwrap()) as usize;
        let raw = u32::from_le_bytes(blob[off + 4..off + 8].try_into().unwrap()) as usize;
        *slot = (
            comp,
            raw,
            u32::from(blob[off + 8]),
            u32::from(blob[off + 9]) << 20,
        );
        assert_eq!(
            blob[off + 10],
            CODEC_PPMD8I,
            "stream {}: only PPMd8 variant I is supported",
            STREAMS[idx]
        );
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
                transforms[idx]
            )
        });
        off += comp;
        if transforms[idx] == TRANSFORM_DELTA8 {
            undelta8(&mut buffer);
        } else {
            assert_eq!(transforms[idx], 0, "stream {name}: unknown transform");
        }
        out.push(buffer);
    }
    assert_eq!(off, blob.len(), "names.bin has trailing bytes");
    let mut streams = out.into_iter();
    Streams {
        plen: streams.next().expect("plen"),
        slen: streams.next().expect("slen"),
        mids: streams.next().expect("mids"),
        entries,
        derived,
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

/// What [`verify`] found.
#[derive(Clone, Debug, Default)]
pub struct Verify {
    /// Names compared against `DerivedName.txt`.
    pub compared: usize,
    /// Names reconstructed by an arm.
    pub derived: usize,
    /// Names taken from the streams.
    pub stored: usize,
    /// Code-point lookups that hit the index.
    pub index_hits: usize,
    /// Code points the pattern section covers.
    pub pattern_cps: usize,
    /// Pattern ranges.
    pub patterns: usize,
    /// Wall time of the round trip.
    pub millis: f64,
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
    let raw = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
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

/// Verifies the plain `*`-pattern section the table actually ships.
///
/// `verify` checks the compressed diagnostic `patterns.bin`, which is a different encoding from
/// the bytes the runtime reads. This walks the shipped layout (`u32 lo`, `u32 hi`, `u16 prefix
/// length`, prefix, `u16 suffix length`, suffix) and compares every record to the corpus.
pub fn verify_pattern_section(section: &[u8], patterns: &[Pattern]) {
    let mut at = 0usize;
    for (index, want) in patterns.iter().enumerate() {
        let lo = u32_at(section, at, "pattern start");
        let hi = u32_at(section, at + 4, "pattern end");
        let prefix_len = usize::from(u16_at(section, at + 8, "prefix length"));
        let prefix_at = at + 10;
        let prefix = section
            .get(prefix_at..prefix_at + prefix_len)
            .unwrap_or_else(|| panic!("pattern {index}: prefix outside the section"));
        let suffix_len_at = prefix_at + prefix_len;
        let suffix_len = usize::from(u16_at(section, suffix_len_at, "suffix length"));
        let suffix_at = suffix_len_at + 2;
        let suffix = section
            .get(suffix_at..suffix_at + suffix_len)
            .unwrap_or_else(|| panic!("pattern {index}: suffix outside the section"));
        assert_eq!(
            (lo, hi, prefix, suffix),
            (
                want.lo,
                want.hi,
                want.prefix.as_bytes(),
                want.suffix.as_bytes()
            ),
            "shipped pattern {index} differs from DerivedName.txt"
        );
        at = suffix_at + suffix_len;
    }
    assert_eq!(at, section.len(), "the pattern section has trailing bytes");
}

fn u32_at(buf: &[u8], at: usize, what: &str) -> u32 {
    let bytes = buf
        .get(at..at + 4)
        .unwrap_or_else(|| panic!("{what} outside the pattern section"));
    u32::from_le_bytes(bytes.try_into().expect("four bytes"))
}

fn u16_at(buf: &[u8], at: usize, what: &str) -> u16 {
    let bytes = buf
        .get(at..at + 2)
        .unwrap_or_else(|| panic!("{what} outside the pattern section"));
    u16::from_le_bytes(bytes.try_into().expect("two bytes"))
}

/// Round-trips the emitted artifact and checks every claim this build makes:
/// names byte-identical to `DerivedName.txt`, code points exact, index and
/// pattern sections consistent.  Panics on the first failure.
///
/// # Errors
///
/// Returns [`Error`] when the written artifacts cannot be read back; every
/// inconsistency in the artifacts themselves is an invariant violation and panics.
#[allow(
    clippy::too_many_lines,
    reason = "one assertion per claim, kept together so the order is readable"
)]
pub fn verify(out_dir: &Path, corpus: &Corpus, rules: &Rules) -> Result<Verify, Error> {
    let start = std::time::Instant::now();
    let blob = crate::read(out_dir.join("names.bin"))?;
    let streams = decode_blob(&blob);
    assert_eq!(
        streams.entries,
        corpus.entries.len(),
        "names.bin describes {} entries, the corpus has {}",
        streams.entries,
        corpus.entries.len()
    );

    // Derived names come from the rules; the shipped code uses the generated
    // tree, which the crate's round-trip test exercises separately.
    let mut seen = 0usize;
    let (text, offsets) = reconstruct(&streams, |id, out| {
        let cp = corpus.entries[usize::try_from(id).expect("id fits")].0;
        let Some(arm) = super::derive::arm_of(rules, cp) else {
            return false;
        };
        super::derive::build_name(&rules.atoms, &arm.ops, cp, out);
        seen += 1;
        true
    });
    assert_eq!(seen, streams.derived, "derived count in the header");
    assert_eq!(seen, rules.derived, "derived count from the rules");

    for (id, (cp, want)) in corpus.entries.iter().enumerate() {
        let got = &text[usize::try_from(offsets[id]).expect("offset fits")
            ..usize::try_from(offsets[id + 1]).expect("offset fits")];
        assert_eq!(
            got,
            want.as_bytes(),
            "U+{cp:04X} decodes as {:?}, want {want:?}",
            String::from_utf8_lossy(got)
        );
    }

    // ---- the name index, from the shipped varint section ------------------
    let runs = decode_runs(&decode_section(
        &crate::read(out_dir.join("runs.bin"))?,
        SectionCodec {
            order: rules.runs.0,
            mem_mib: rules.runs.1,
        },
    ));
    let covered: usize = runs
        .iter()
        .map(|(_, len, _)| usize::try_from(*len).expect("run fits"))
        .sum();
    assert_eq!(
        covered,
        corpus.entries.len(),
        "the index covers {covered} entries"
    );
    let mut cursor = 0usize;
    for (start, len, base) in &runs {
        assert_eq!(
            *base as usize, cursor,
            "run base id is not the running total"
        );
        assert_eq!(
            corpus.entries[cursor].0, *start,
            "run start does not match entry {cursor}"
        );
        cursor += usize::try_from(*len).expect("run fits");
    }
    for (id, (cp, name)) in corpus.entries.iter().enumerate() {
        let found =
            name_id(&runs, *cp).unwrap_or_else(|| panic!("U+{cp:04X} is missing from the index"));
        let want = u32::try_from(id).expect("id fits");
        assert_eq!(
            found, want,
            "the index maps U+{cp:04X} to {found}, want {want}"
        );
        let got = &text[usize::try_from(offsets[id]).expect("offset fits")
            ..usize::try_from(offsets[id + 1]).expect("offset fits")];
        assert_eq!(
            got,
            name.as_bytes(),
            "index lookup for U+{cp:04X} disagrees"
        );
    }
    for cp in [0x0D800u32, 0x0378, 0x2FA1E] {
        if corpus.id_of(cp).is_none() {
            assert!(
                name_id(&runs, cp).is_none(),
                "U+{cp:04X} must miss the index"
            );
        }
    }

    // ---- the pattern section ----------------------------------------------
    let patterns = decode_patterns(&decode_section(
        &crate::read(out_dir.join("patterns.bin"))?,
        SectionCodec {
            order: rules.patterns.0,
            mem_mib: rules.patterns.1,
        },
    ));
    assert_eq!(patterns.len(), corpus.patterns.len(), "pattern count");
    for pair in patterns.windows(2) {
        assert!(
            pair[0].hi < pair[1].lo,
            "patterns overlap near U+{:04X}",
            pair[1].lo
        );
    }
    let mut pattern_cps = 0usize;
    for (got, want) in patterns.iter().zip(&corpus.patterns) {
        assert_eq!(
            (got.lo, got.hi, &got.prefix, &got.suffix),
            (want.lo, want.hi, &want.prefix, &want.suffix),
            "pattern range differs from DerivedName.txt"
        );
        pattern_cps += usize::try_from(got.hi - got.lo + 1).expect("range fits");
        for cp in got.lo..=got.hi {
            assert!(
                corpus.id_of(cp).is_none(),
                "U+{cp:04X} is covered by a pattern and must not be stored"
            );
        }
    }
    // Unicode's algorithmic names are `prefix + uppercase hex + suffix`; if the
    // convention were wrong nothing else here would notice.
    for (cp, want) in [
        (0x4E00u32, "CJK UNIFIED IDEOGRAPH-4E00"),
        (0x9FFF, "CJK UNIFIED IDEOGRAPH-9FFF"),
        (0x20000, "CJK UNIFIED IDEOGRAPH-20000"),
        (0x2A700, "CJK UNIFIED IDEOGRAPH-2A700"),
        (0x2F800, "CJK COMPATIBILITY IDEOGRAPH-2F800"),
        (0x18CFF, "KHITAN SMALL SCRIPT CHARACTER-18CFF"),
        (0x18D00, "TANGUT IDEOGRAPH-18D00"),
        (0x1B170, "NUSHU CHARACTER-1B170"),
        (0x3D000, "SMALL SEAL CHARACTER-3D000"),
    ] {
        let pattern = patterns
            .iter()
            .find(|p| (p.lo..=p.hi).contains(&cp))
            .unwrap_or_else(|| panic!("U+{cp:04X} is in no pattern range"));
        assert_eq!(
            format!("{}{:X}{}", pattern.prefix, cp, pattern.suffix),
            want,
            "algorithmic name for U+{cp:04X}"
        );
    }
    for cp in (HANGUL_START..=HANGUL_END).step_by(1_357) {
        assert!(
            corpus.id_of(cp).is_none(),
            "U+{cp:04X} is a Hangul syllable and must not be stored"
        );
    }

    Ok(Verify {
        compared: corpus.entries.len(),
        derived: seen,
        stored: corpus.entries.len() - seen,
        index_hits: corpus.entries.len(),
        pattern_cps,
        patterns: patterns.len(),
        millis: start.elapsed().as_secs_f64() * 1e3,
    })
}
