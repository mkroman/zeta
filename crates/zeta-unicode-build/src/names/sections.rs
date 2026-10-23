//! The two shipped section encoders: the name index and the `*`-pattern table.
//!
//! Both formats are varint- or length-prefixed and do not depend on the front-coded
//! name streams, so the encoders take the corpus slices they need instead of the
//! whole [`Corpus`](super::corpus::Corpus).

use crate::table::{push_len_u16, push_u32};

use super::codec;
use super::corpus::Pattern;

/// Lowest code point the pattern section can describe (the CJK plane start).
pub(super) const PATTERN_BASE: u32 = 0x3400;

/// The shipped name index, varint-coded: one `(len, gap)` pair per run of
/// consecutive code points.  The base name id is the running total of the lengths,
/// so nothing else has to be stored.
///
/// Returns the stream and the number of runs it encodes.
#[must_use]
pub(super) fn runs_stream(entries: &[(u32, String)]) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut run_count = 0usize;
    let mut run_start = 0usize;
    for id in 1..=entries.len() {
        let ends = id == entries.len() || entries[id].0 != entries[id - 1].0 + 1;
        if ends {
            let len = u32::try_from(id - run_start).expect("run fits in u32");
            let start = entries[run_start].0;
            let gap = if run_count == 0 {
                start
            } else {
                start - (entries[run_start - 1].0 + 1)
            };
            codec::varint(u64::from(len), &mut out);
            codec::varint(u64::from(gap), &mut out);
            run_count += 1;
            run_start = id;
        }
    }
    (out, run_count)
}

/// The shipped `*`-pattern section: `u32 lo`, `u32 hi`, `u16 prefix length`, the
/// prefix, `u16 suffix length`, the suffix — one record per range, in ascending
/// order.
///
/// The decoder scans these on every name miss, so they are stored plainly; the
/// compressed `patterns.bin` is a diagnostic artifact and a build check.
#[must_use]
pub(super) fn pattern_section(patterns: &[Pattern]) -> Vec<u8> {
    let mut out = Vec::new();
    for pattern in patterns {
        push_u32(&mut out, pattern.lo);
        push_u32(&mut out, pattern.hi);
        push_len_u16(&mut out, pattern.prefix.len(), "pattern prefix");
        out.extend_from_slice(pattern.prefix.as_bytes());
        push_len_u16(&mut out, pattern.suffix.len(), "pattern suffix");
        out.extend_from_slice(pattern.suffix.as_bytes());
    }
    out
}

/// The diagnostic pattern stream for `patterns.bin`: `varint(lo - 0x3400)
/// varint(len) prefix|suffix|` per `*`-pattern range.
#[must_use]
pub(super) fn patterns_stream(patterns: &[Pattern]) -> Vec<u8> {
    let mut out = Vec::new();
    for pattern in patterns {
        assert!(pattern.lo >= PATTERN_BASE, "a pattern starts below U+3400");
        codec::varint(u64::from(pattern.lo - PATTERN_BASE), &mut out);
        codec::varint(u64::from(pattern.hi - pattern.lo + 1), &mut out);
        out.extend_from_slice(pattern.prefix.as_bytes());
        out.push(b'|');
        out.extend_from_slice(pattern.suffix.as_bytes());
        out.push(b'|');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(pairs: &[(u32, &str)]) -> Vec<(u32, String)> {
        pairs
            .iter()
            .map(|(cp, name)| (*cp, (*name).to_owned()))
            .collect()
    }

    #[test]
    fn runs_end_at_every_code_point_gap() {
        let entries = entries(&[
            (0x41, "A"),
            (0x42, "B"),
            (0x45, "E"),
            (0x46, "F"),
            (0x47, "G"),
            (0x4A, "J"),
        ]);
        let (raw, count) = runs_stream(&entries);
        assert_eq!(count, 3);
        // Each pair is `varint(len) varint(gap)`; every value here is one byte.
        // The first gap is the absolute start, later gaps count the code points
        // skipped after the previous run's end.
        assert_eq!(raw, [2, 0x41, 3, 2, 1, 2]);
    }

    #[test]
    fn a_single_entry_is_one_run() {
        let (raw, count) = runs_stream(&entries(&[(0x20, "SPACE")]));
        assert_eq!(count, 1);
        assert_eq!(raw, [1, 0x20]);
    }

    #[test]
    fn pattern_records_use_the_shipped_layout() {
        let patterns = vec![
            Pattern {
                lo: 0x3400,
                hi: 0x4DBF,
                prefix: "CJK UNIFIED IDEOGRAPH-".to_owned(),
                suffix: String::new(),
            },
            Pattern {
                lo: 0x18CFF,
                hi: 0x18CFF,
                prefix: "KHITAN SMALL SCRIPT CHARACTER-".to_owned(),
                suffix: "!".to_owned(),
            },
        ];
        let section = pattern_section(&patterns);

        let mut want = Vec::new();
        for pattern in &patterns {
            want.extend_from_slice(&pattern.lo.to_le_bytes());
            want.extend_from_slice(&pattern.hi.to_le_bytes());
            want.extend_from_slice(
                &u16::try_from(pattern.prefix.len())
                    .expect("prefix fits")
                    .to_le_bytes(),
            );
            want.extend_from_slice(pattern.prefix.as_bytes());
            want.extend_from_slice(
                &u16::try_from(pattern.suffix.len())
                    .expect("suffix fits")
                    .to_le_bytes(),
            );
            want.extend_from_slice(pattern.suffix.as_bytes());
        }
        assert_eq!(section, want);
        let first_record = 8 + 2 + patterns[0].prefix.len() + 2 + patterns[0].suffix.len();
        let second_record = &section[first_record..];
        assert_eq!(&second_record[8..10], &30u16.to_le_bytes());
        assert_eq!(
            &second_record[10 + patterns[1].prefix.len()..12 + patterns[1].prefix.len()],
            &1u16.to_le_bytes()
        );
    }
}
