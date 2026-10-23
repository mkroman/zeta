//! The human-readable build report printed to stderr by `ZETA_UNICODE_STATS` builds.

use zeta_unicode_format::{NAMES_HEADER_LEN as HEADER_LEN, SECTION_PREFIX_LEN};

use super::corpus::Corpus;
use super::rules::Rules;
use super::{Ledger, STREAMS};

/// Prints the size report to stderr when the request asks for it.
pub(super) fn report(ledger: &Ledger, corpus: &Corpus, rules: &Rules, request: &crate::Request) {
    eprintln!(
        "corpus: {} names / {} text bytes ({} individual lines - {} Hangul - {} \
         single-code-point pattern + {} aliases), Unicode {}",
        ledger.entries,
        ledger.text_bytes,
        ledger.entries + corpus.hangul + corpus.single_patterns - corpus.aliases,
        corpus.hangul,
        corpus.single_patterns,
        corpus.aliases,
        corpus.unicode
    );
    eprintln!(
        "rules:  {} arms, {} atoms / {} B, Unicode {}, {} B on disk",
        ledger.arms,
        ledger.atoms,
        ledger.atom_bytes,
        rules.unicode,
        std::fs::metadata(&request.rules).map_or(0, |m| usize::try_from(m.len()).unwrap_or(0))
    );
    for (name, size) in STREAMS.iter().zip(ledger.streams) {
        eprintln!(
            "  {name:<5} raw {:>8?} -> {:>7?}  o{}/m{}M/{}",
            size.raw,
            size.compressed,
            size.order,
            size.mem_mib,
            if size.delta8 { "delta8" } else { "raw" }
        );
    }
    eprintln!(
        "  names.bin {:>8?} B  (header {} + streams {:?})",
        ledger.blob, HEADER_LEN, ledger.streams_total
    );
    eprintln!(
        "  RUNS      {:>8?} B  ({} runs, raw {:?} -> {:?} + 6, o{}/m{}M)",
        ledger.runs.compressed + SECTION_PREFIX_LEN,
        ledger.runs_count,
        ledger.runs.raw,
        ledger.runs.compressed,
        ledger.runs.order,
        ledger.runs.mem_mib
    );
    eprintln!(
        "  PATTERNS  {:>8?} B  ({} ranges, shipped plainly; {:?} B compressed diagnostic)",
        ledger.pattern_section.len(),
        ledger.patterns_count,
        ledger.patterns.compressed + SECTION_PREFIX_LEN
    );
    eprintln!("  TABLE     {:>8?} B", ledger.table);
    eprintln!(
        "  strict    {:>8?} B  (the same plus the {} B of atom literals)",
        ledger.table_strict, ledger.atom_bytes
    );
}
