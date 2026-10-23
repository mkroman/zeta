//! Round-trip verification: the emitter re-decodes its own artifacts and checks
//! every claim against the vendored UCD text.
//!
//! The decoder in [`decode`] is deliberately separate from the runtime decoder: it is an
//! independent check of the written bytes, while `zeta-unicode`'s own round-trip tests check the
//! runtime side against the same UCD files. Only the format constants are shared, through
//! `zeta-unicode-format`.

mod decode;

use std::path::Path;

use zeta_unicode_format::{HANGUL_END, HANGUL_START};

use self::decode::{
    Streams, decode_blob, decode_patterns, decode_runs, decode_section, name_at, name_id,
    reconstruct, u16_at, u32_at,
};
use super::codec::SectionCodec;
use super::corpus::{Corpus, Pattern};
use super::rules::Rules;
use crate::Error;

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

/// Round-trips the emitted artifact and checks every claim this build makes:
/// names byte-identical to `DerivedName.txt`, code points exact, index and
/// pattern sections consistent.  Panics on the first failure.
///
/// # Errors
///
/// Returns [`Error`] when the written artifacts cannot be read back; every
/// inconsistency in the artifacts themselves is an invariant violation and panics.
pub fn verify(out_dir: &Path, corpus: &Corpus, rules: &Rules) -> Result<Verify, Error> {
    let start = std::time::Instant::now();
    let blob = crate::read(out_dir.join("names.bin"))?;
    let streams = decode_blob(&blob);
    let derived = verify_names(&streams, corpus, rules);
    let index_hits = verify_runs(out_dir, corpus, rules)?;
    let coverage = verify_patterns(out_dir, corpus, rules)?;
    verify_hangul(corpus);

    Ok(Verify {
        compared: corpus.entries.len(),
        derived,
        stored: corpus.entries.len() - derived,
        index_hits,
        pattern_cps: coverage.cps,
        patterns: coverage.ranges,
        millis: start.elapsed().as_secs_f64() * 1e3,
    })
}

/// Reconstructs every name from the streams and compares it with the corpus.
///
/// Returns the number of names the rules rebuilt, i.e. the names that were absent from the
/// streams.
fn verify_names(streams: &Streams, corpus: &Corpus, rules: &Rules) -> usize {
    assert_eq!(
        streams.entries,
        corpus.entries.len(),
        "names.bin describes {} entries, the corpus has {}",
        streams.entries,
        corpus.entries.len()
    );

    // Derived names come from the rules; the shipped code uses the generated
    // tree, which the crate's round-trip test exercises separately.
    let mut derived = 0usize;
    let (text, offsets) = reconstruct(streams, |id, out| {
        let cp = corpus.entries[usize::try_from(id).expect("id fits")].0;
        let Some(arm) = super::derive::arm_of(rules, cp) else {
            return false;
        };
        super::derive::build_name(&rules.atoms, &arm.ops, cp, out);
        derived += 1;
        true
    });
    assert_eq!(derived, streams.derived, "derived count in the header");
    assert_eq!(derived, rules.derived, "derived count from the rules");

    for (id, (cp, want)) in corpus.entries.iter().enumerate() {
        let got = name_at(&text, &offsets, id);
        assert_eq!(
            got,
            want.as_bytes(),
            "U+{cp:04X} decodes as {:?}, want {want:?}",
            String::from_utf8_lossy(got)
        );
    }
    derived
}

/// Reads the shipped name index, checks every corpus lookup lands on its id, and returns the
/// number of lookups that hit it.
fn verify_runs(out_dir: &Path, corpus: &Corpus, rules: &Rules) -> Result<usize, Error> {
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

    // [`verify_names`] already proved the decoded text is the corpus name, so the
    // only remaining claim is that every code point maps to its own id.
    let mut hits = 0usize;
    for (id, (cp, _)) in corpus.entries.iter().enumerate() {
        let found =
            name_id(&runs, *cp).unwrap_or_else(|| panic!("U+{cp:04X} is missing from the index"));
        let want = u32::try_from(id).expect("id fits");
        assert_eq!(
            found, want,
            "the index maps U+{cp:04X} to {found}, want {want}"
        );
        hits += 1;
    }
    for cp in [0x0D800u32, 0x0378, 0x2FA1E] {
        if corpus.id_of(cp).is_none() {
            assert!(
                name_id(&runs, cp).is_none(),
                "U+{cp:04X} must miss the index"
            );
        }
    }
    Ok(hits)
}

/// What the shipped `*`-pattern ranges cover.
struct PatternCoverage {
    /// Code points across every range.
    cps: usize,
    /// Number of ranges.
    ranges: usize,
}

/// Reads the diagnostic `patterns.bin` and checks every range against the corpus.
fn verify_patterns(
    out_dir: &Path,
    corpus: &Corpus,
    rules: &Rules,
) -> Result<PatternCoverage, Error> {
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
    let mut cps = 0usize;
    for (got, want) in patterns.iter().zip(&corpus.patterns) {
        assert_eq!(
            (got.lo, got.hi, &got.prefix, &got.suffix),
            (want.lo, want.hi, &want.prefix, &want.suffix),
            "pattern range differs from DerivedName.txt"
        );
        cps += usize::try_from(got.hi - got.lo + 1).expect("range fits");
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
    Ok(PatternCoverage {
        cps,
        ranges: patterns.len(),
    })
}

/// Samples the Hangul syllable block, which must stay algorithmic rather than stored.
fn verify_hangul(corpus: &Corpus) {
    for cp in (HANGUL_START..=HANGUL_END).step_by(1_357) {
        assert!(
            corpus.id_of(cp).is_none(),
            "U+{cp:04X} is a Hangul syllable and must not be stored"
        );
    }
}
