//! Deterministic, dependency-light builder for the `zeta-unicode` name table.
//!
//! This module is the generator side of approach #6 ("names as code").  The
//! checked-in rules file (`data/derived-names.rules`) lists, per code-point range, how a
//! Unicode character name can be *reconstructed* from its code point, so those
//! names never enter the compressed streams.  Everything else is front-coded and
//! compressed with PPMd8-I through `ppmd-rust`, the same pure-Rust codec the
//! runtime decoder uses.
//!
//! Nothing is derived here beyond what the rules state: [`emit`] parses the
//! rules, replays every arm against the vendored `DerivedName.txt`, and
//! **panics unless every reconstructed name is byte-identical to the file**.  A
//! rules file that has drifted from Unicode is a build failure, not a warning.
//!
//! # Inputs
//!
//! * `data/DerivedName.txt` — the ground truth: every individually named code
//!   point, plus the `*`-pattern ranges (which stay algorithmic at lookup time).
//! * `data/NameAliases.txt` — the control names are always folded in, which
//!   reproduces the incumbent behaviour (35,416 entries instead of 35,351).
//! * `data/derived-names.rules` — the rules file (grammar below).
//!
//! # Rules-file grammar
//!
//! Line oriented, ASCII, no dependencies.  Blank lines and whole-line `#`
//! comments are ignored — an inline `#` is *not* a comment, it never appears in
//! a valid directive, atom or recipe — `@` lines are directives, `A<dddd>`
//! lines are atoms, and everything else is an arm.
//!
//! ```text
//! @unicode 18.0.0
//! @corpus 35351
//! @derived 6677
//! @arms 5286
//! @atoms 324
//! @stream <label> <transform> <order> <mem_mib>
//! @runs <order> <mem_mib>
//! @patterns <order> <mem_mib>
//!
//! A0000 <token>
//! ...
//!
//! 0041:005A fga+65
//! ```
//!
//! `@unicode`, `@arms` and `@atoms` are informational; `@corpus` and `@derived`
//! are cross-checked against the parsed corpus and the arms.  The arm in the
//! example covers U+0041..=U+005A.  `<label>` is one of the [`STREAMS`] names,
//! `<transform>` is `raw` or `delta8` — the middle stream only supports `raw`,
//! because the decoder refuses to undelta it.
//!
//! An **atom** is one space-delimited token of a name, trailing space included.
//! The line is `A`, four decimal digits, one space, then the token — so the
//! token may itself start or end with a space (a lone space is `A0003` +
//! two spaces).  Atoms are numbered in the order they appear in the
//! arms, most-used first, and the recipe codes refer to those numbers.  An atom
//! is a literal the generated tree pushes, i.e. `.rodata`.
//!
//! An **arm** is `<cp_lo>` or `<cp_lo>:<cp_hi>` in hex (no `U+` prefix) followed
//! by a space and a recipe: push-atom codes and **exactly one hole**, the single
//! value that depends on the code point.
//!
//! | recipe token | meaning |
//! |---|---|
//! | `a`..`z`, `A`..`Z` | push atom 0..51 |
//! | `'<letter>` | push atom 52..103 |
//! | `''<letter><letter>` | push atom 104..323 |
//! | `+<n>` / `-<n>` | hole: push the byte `cp - cp_lo + n` (decimal, signed) |
//! | `=<hex>[,<width>]` | hole: push uppercase hex of `cp - cp_lo + <hex>`, zero-padded to `<width>` (absent = no padding) |
//!
//! Hole values are stated relative to the arm's first code point because that is
//! what keeps them short (`+65` rather than `-917568`); the emitter converts them
//! back to absolute offsets, so the generated code is unaffected.  Arms are
//! sorted by `cp_lo` and must not overlap.
//!
//! # Outputs
//!
//! [`emit`] writes into the request's `out_dir`:
//!
//! * `names.bin` — the `UNNM` container: a 50-byte header and three PPMd8-I
//!   streams (`plen`, `slen`, `mids`).
//! * `runs.bin` — the name index (`cp` → `name_id`), varint-coded, then
//!   PPMd8-I.
//! * `patterns.bin` — the `*`-pattern ranges, varint-coded, then PPMd8-I. This is a
//!   diagnostic check; the table ships the plain [`pattern_section`] layout.
//! * `expected.txt` — the corpus as `<hex cp>\t<name>`, the decoder's ground truth. Both
//!   files are diagnostics and are not consumed further.
//! * the generated derivation tree (`.text` + `.rodata`) under the
//!   caller-chosen file name, for `include!` from the consumer's `src/`.
//!
//! and returns a [`Ledger`] with every size, so the caller can assert against
//! its budget.  Each of `runs.bin` and `patterns.bin` is a `u32` raw length, a
//! `u8` model order and a `u8` memory limit followed by the compressed stream;
//! the name blob carries those lengths in its header instead, which is why only
//! the two sections pay the 6 B prefix.

mod codec;
mod corpus;
mod derive;
mod rules;
mod verify;

use std::fmt::Write as _;

use corpus::Corpus;
use rules::Rules;

// ---------------------------------------------------------------- constants

/// Container magic for the name blob.
const MAGIC: &[u8; 4] = b"UNNM";
/// Container version understood by this module and by the decoder.
const BLOB_VERSION: u8 = 1;
/// Codec id: PPMd8-I (`ppmd-rust`), the only codec this build emits.
const CODEC_PPMD8I: u8 = 1;
/// Transform id: cumulative (delta) sum of `u8`s.
const TRANSFORM_DELTA8: u8 = 1;
/// Length of the fixed container header.
const HEADER_LEN: usize = 4 + 1 + 1 + 3 + 4 + 4 + 3 * 11;
/// Length of one stream descriptor inside the header.
const STREAM_DESC_LEN: usize = 11;
/// `u32` raw length, `u8` model order and `u8` memory limit on the two standalone sections.
const SECTION_PREFIX_LEN: usize = 6;
/// Lowest code point the pattern section can describe (the CJK plane start).
const PATTERN_BASE: u32 = 0x3400;

/// The three name streams, in container order.
pub const STREAMS: [&str; 3] = ["plen", "slen", "mids"];

// ------------------------------------------------------------------- output

/// Every size one build produces, in bytes, plus the packed section payloads the caller
/// ships in the table.
#[derive(Clone, Debug)]
pub struct Ledger {
    /// Names in the corpus (index space).
    pub entries: usize,
    /// Names reconstructed by an arm instead of stored.
    pub derived: usize,
    /// Names stored (front-coded into the streams).
    pub stored: usize,
    /// Arms in the rules file.
    pub arms: usize,
    /// Literal tokens the generated tree pushes.
    pub atoms: usize,
    /// Sum of the atom texts: the `.rodata` the tree needs.
    pub atom_bytes: usize,
    /// Concatenated name text, for reference.
    pub text_bytes: usize,
    /// Per-stream sizes, in container order.
    pub streams: [StreamSize; 3],
    /// `names.bin` as written.
    pub blob: usize,
    /// The three compressed streams alone, i.e. `blob` without the header.
    pub streams_total: usize,
    /// The packed name blob the table ships, byte-identical to the `names.bin` artifact.
    pub names_section: Vec<u8>,
    /// The packed name index the table ships, byte-identical to the `runs.bin` artifact.
    pub runs_section: Vec<u8>,
    /// The name index section.
    pub runs: SectionSize,
    /// The compressed diagnostic pattern stream (`patterns.bin`); the table ships
    /// `pattern_section` instead.
    pub patterns: SectionSize,
    /// Consecutive-code-point runs in the index.
    pub runs_count: usize,
    /// `*`-pattern ranges.
    pub patterns_count: usize,
    /// The plain `*`-pattern section the table actually ships, exactly as the private
    /// `pattern_section` encoder writes it.
    pub pattern_section: Vec<u8>,
    /// `blob + runs + pattern_section`, the number the caller's budget is about.
    pub table: usize,
    /// `table + atom_bytes`: the strict reading that also charges the merged
    /// literals to the table.
    pub table_strict: usize,
}

/// One compressed name stream.
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamSize {
    /// Bytes handed to the codec.
    pub raw: usize,
    /// Bytes the codec produced.
    pub compressed: usize,
    /// `PPMd` model order.
    pub order: u32,
    /// `PPMd` memory limit, in MiB.
    pub mem_mib: u32,
    /// `true` when the stream is cumulative-sum coded before compression.
    pub delta8: bool,
}

/// One compressed section (the name index or the pattern list).
#[derive(Clone, Copy, Debug, Default)]
pub struct SectionSize {
    /// Bytes handed to the codec.
    pub raw: usize,
    /// Bytes the codec produced, excluding the six-byte section prefix.
    pub compressed: usize,
    /// `PPMd` model order.
    pub order: u32,
    /// `PPMd` memory limit, in MiB.
    pub mem_mib: u32,
}

// -------------------------------------------------------------------- build

/// Builds every artifact and returns the ledger.
///
/// # Errors
///
/// Returns [`Error`](crate::Error) when `out_dir` cannot be created or an artifact cannot be
/// written, or when the verification cannot read the artifacts back. Every inconsistency in the
/// vendored data or the written artifacts is an invariant violation and panics.
#[allow(
    clippy::too_many_lines,
    reason = "the five artifacts in the order they are written"
)]
pub fn emit(request: &crate::Request, inputs: &crate::ucd::Inputs) -> Result<Ledger, crate::Error> {
    let corpus = corpus::read_corpus(&inputs.names, &inputs.aliases);
    let rules = rules::read_rules(&request.rules);
    if !rules.unicode.is_empty() {
        assert_eq!(
            corpus.unicode, rules.unicode,
            "the rules are for a different Unicode release than DerivedName.txt"
        );
    }
    crate::create_dir_all(&request.out_dir)?;
    let (derived, plen, slen, mids, text_bytes) = front_code(&rules, &corpus);
    let derived_count = derived.iter().filter(|d| **d).count();
    let stored = corpus.entries.len() - derived_count;

    // ---- compress ----------------------------------------------------------
    // Header layout, exactly as the decoder reads it:
    //   0..4 magic | 4 version | 5 codec | 6..9 transform[3] | 9..13 entries |
    //   13..17 derived | 17..50 three stream descriptors | 50.. the streams
    let mut transforms: Vec<u8> = Vec::with_capacity(3);
    let mut descriptors: Vec<u8> = Vec::with_capacity(3 * STREAM_DESC_LEN);
    let mut sizes = [StreamSize::default(); 3];
    let mut streams: Vec<Vec<u8>> = Vec::with_capacity(3);
    for (idx, params) in rules.streams.iter().copied().enumerate() {
        transforms.push(u8::from(params.delta8));
        let source: Vec<u8> = match (idx, params.delta8) {
            (0, true) => delta8(&plen),
            (0, false) => plen.clone(),
            (1, true) => delta8(&slen),
            (1, false) => slen.clone(),
            (_, false) => mids.clone(),
            (_, true) => unreachable!("read_rules rejects a delta8-coded middle stream"),
        };
        let compressed = codec::ppmd8i(&source, params.order, params.mem_mib);
        descriptors.extend_from_slice(
            &u32::try_from(compressed.len())
                .expect("compressed length fits in u32")
                .to_le_bytes(),
        );
        descriptors.extend_from_slice(
            &u32::try_from(source.len())
                .expect("stream length fits in u32")
                .to_le_bytes(),
        );
        descriptors.push(u8::try_from(params.order).expect("order fits in a byte"));
        descriptors.push(u8::try_from(params.mem_mib).expect("memory fits in a byte"));
        descriptors.push(1); // PPMd8 variant I
        sizes[idx] = StreamSize {
            raw: source.len(),
            compressed: compressed.len(),
            order: params.order,
            mem_mib: params.mem_mib,
            delta8: params.delta8,
        };
        streams.push(compressed);
    }
    let mut header: Vec<u8> = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.push(BLOB_VERSION);
    header.push(CODEC_PPMD8I);
    header.extend_from_slice(&transforms);
    header.extend_from_slice(
        &u32::try_from(corpus.entries.len())
            .expect("fewer than 2^32 names")
            .to_le_bytes(),
    );
    header.extend_from_slice(
        &u32::try_from(derived_count)
            .expect("fewer than 2^32 names")
            .to_le_bytes(),
    );
    header.extend_from_slice(&descriptors);
    assert_eq!(header.len(), HEADER_LEN, "the header layout changed");

    let mut blob = header;
    for stream in &streams {
        blob.extend_from_slice(stream);
    }
    crate::write(request.out_dir.join("names.bin"), &blob)?;

    // ---- the name index ----------------------------------------------------
    let runs_raw = runs_stream(&corpus);
    let runs_bytes = codec::section_bytes(&runs_raw, rules.runs.0, rules.runs.1);
    crate::write(request.out_dir.join("runs.bin"), &runs_bytes)?;

    // ---- the pattern section ----------------------------------------------
    let patterns_raw = patterns_stream(&corpus);
    let patterns_bytes = codec::section_bytes(&patterns_raw, rules.patterns.0, rules.patterns.1);
    crate::write(request.out_dir.join("patterns.bin"), &patterns_bytes)?;

    // ---- ground truth and the generated tree -------------------------------
    let mut expected = String::with_capacity(1_140_000);
    for (cp, name) in &corpus.entries {
        let _ = writeln!(expected, "{cp:X}\t{name}");
    }
    crate::write(request.out_dir.join("expected.txt"), expected)?;
    let gen_rs = derive::emit_gen_rs(&rules, &corpus);
    crate::write(request.out_dir.join(&request.derive_file), gen_rs)?;

    let streams_total: usize = sizes.iter().map(|s| s.compressed).sum();
    let atom_bytes: usize = rules.atoms.iter().map(String::len).sum();
    let runs_size = runs_bytes.len();
    let blob_len = blob.len();
    let patterns_size = patterns_bytes.len();
    // The table ships the plain section, not the compressed diagnostic artifact, so
    // that is the size the budget has to charge.
    let pattern_section_data = pattern_section(&corpus.patterns);
    let pattern_section_len = pattern_section_data.len();
    let ledger = Ledger {
        entries: corpus.entries.len(),
        derived: derived_count,
        stored,
        arms: rules.arms.len(),
        atoms: rules.atoms.len(),
        atom_bytes,
        text_bytes,
        streams: sizes,
        blob: blob_len,
        streams_total,
        names_section: blob,
        runs_section: runs_bytes,
        runs: SectionSize {
            raw: runs_raw.len(),
            compressed: runs_size - SECTION_PREFIX_LEN,
            order: rules.runs.0,
            mem_mib: rules.runs.1,
        },
        patterns: SectionSize {
            raw: patterns_raw.len(),
            compressed: patterns_size - SECTION_PREFIX_LEN,
            order: rules.patterns.0,
            mem_mib: rules.patterns.1,
        },
        runs_count: run_count(&corpus),
        patterns_count: corpus.patterns.len(),
        pattern_section: pattern_section_data,
        table: blob_len + runs_size + pattern_section_len,
        table_strict: blob_len + runs_size + pattern_section_len + atom_bytes,
    };
    if request.report {
        report(&ledger, &corpus, &rules, request);
    }
    let result = verify::verify(&request.out_dir, &corpus, &rules)?;
    if request.report {
        eprintln!(
            "  verified {} names byte-identical ({} derived, {} stored), \
             {} index hits, {} pattern code points in {} ranges, {:.1} ms",
            result.compared,
            result.derived,
            result.stored,
            result.index_hits,
            result.pattern_cps,
            result.patterns,
            result.millis
        );
    }
    Ok(ledger)
}

/// Replays the arms, front-codes the corpus, and returns
/// `(derived, plen, slen, mids, text_bytes)`.
///
/// A derived name is absent from all three streams but stays a valid
/// predecessor: every record is taken against the previous entry exactly as
/// `DerivedName.txt` spells it, derived or not.  The decoder walks entry ids in
/// ascending order and rebuilds the same predecessor chain, so it needs no
/// per-entry knowledge of which names are derived.
#[allow(
    clippy::too_many_lines,
    reason = "the front-code loop plus its invariants"
)]
fn front_code(rules: &Rules, corpus: &Corpus) -> (Vec<bool>, Vec<u8>, Vec<u8>, Vec<u8>, usize) {
    let derived = derive::derive_all(rules, corpus);
    let mut plen: Vec<u8> = Vec::new();
    let mut slen: Vec<u8> = Vec::new();
    let mut mids: Vec<u8> = Vec::new();
    let mut prev: &[u8] = &[];
    let mut text_bytes = 0usize;
    for (id, (_, name)) in corpus.entries.iter().enumerate() {
        let name = name.as_bytes();
        text_bytes += name.len();
        let mut head = 0usize;
        while head < name.len().min(prev.len()) && name[head] == prev[head] {
            head += 1;
        }
        let mut tail = 0usize;
        while tail < (name.len() - head).min(prev.len() - head)
            && name[name.len() - 1 - tail] == prev[prev.len() - 1 - tail]
        {
            tail += 1;
        }
        if !derived[id] {
            plen.push(u8::try_from(head).expect("shared prefix fits in a byte"));
            slen.push(u8::try_from(tail).expect("shared suffix fits in a byte"));
            mids.extend_from_slice(&name[head..name.len() - tail]);
            mids.push(0);
        }
        prev = name;
    }
    (derived, plen, slen, mids, text_bytes)
}

/// Applies cumulative (delta-8) coding to `values`.
fn delta8(values: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len());
    let mut previous = 0u8;
    for &value in values {
        out.push(value.wrapping_sub(previous));
        previous = value;
    }
    out
}

fn report(ledger: &Ledger, corpus: &Corpus, rules: &Rules, request: &crate::Request) {
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

/// The shipped name index, varint-coded: one `(len, gap)` pair per run of
/// consecutive code points.  The base name id is the running total of the lengths,
/// so nothing else has to be stored.
fn runs_stream(corpus: &Corpus) -> Vec<u8> {
    let mut out = Vec::new();
    let mut run_count = 0usize;
    let mut run_start = 0usize;
    for id in 1..=corpus.entries.len() {
        let ends =
            id == corpus.entries.len() || corpus.entries[id].0 != corpus.entries[id - 1].0 + 1;
        if ends {
            let len = u32::try_from(id - run_start).expect("run fits in u32");
            let start = corpus.entries[run_start].0;
            let gap = if run_count == 0 {
                start
            } else {
                start - (corpus.entries[run_start - 1].0 + 1)
            };
            codec::varint(u64::from(len), &mut out);
            codec::varint(u64::from(gap), &mut out);
            run_count += 1;
            run_start = id;
        }
    }
    out
}

/// The shipped `*`-pattern section: `u32 lo`, `u32 hi`, `u16 prefix length`, the
/// prefix, `u16 suffix length`, the suffix — one record per range, in ascending
/// order.
///
/// The decoder scans these on every name miss, so they are stored plainly; the
/// compressed `patterns.bin` is a diagnostic artifact and a build check.
#[must_use]
fn pattern_section(patterns: &[corpus::Pattern]) -> Vec<u8> {
    let mut out = Vec::new();
    for pattern in patterns {
        crate::table::push_u32(&mut out, pattern.lo);
        crate::table::push_u32(&mut out, pattern.hi);
        crate::table::push_u16(
            &mut out,
            u16::try_from(pattern.prefix.len()).expect("prefix fits in u16"),
        );
        out.extend_from_slice(pattern.prefix.as_bytes());
        crate::table::push_u16(
            &mut out,
            u16::try_from(pattern.suffix.len()).expect("suffix fits in u16"),
        );
        out.extend_from_slice(pattern.suffix.as_bytes());
    }
    out
}

/// The diagnostic pattern stream for `patterns.bin`: `varint(lo - 0x3400)
/// varint(len) prefix|suffix|` per `*`-pattern range.
///
/// `skip_single` drops the single-code-point pattern (U+18CFF in Unicode 18.0.0).
/// It exists so the cost of answering it algorithmically — rather than storing
/// `KHITAN SMALL SCRIPT CHARACTER-*` with a literal asterisk, as the incumbent
/// does — can be measured; the build always ships it.
fn patterns_stream_with(corpus: &Corpus, skip_single: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for pattern in &corpus.patterns {
        if skip_single && pattern.lo == pattern.hi {
            continue;
        }
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

fn patterns_stream(corpus: &Corpus) -> Vec<u8> {
    patterns_stream_with(corpus, false)
}

/// The number of consecutive-code-point runs in the index.
#[must_use]
fn run_count(corpus: &Corpus) -> usize {
    let mut count = 0usize;
    for id in 1..corpus.entries.len() {
        if corpus.entries[id].0 != corpus.entries[id - 1].0 + 1 {
            count += 1;
        }
    }
    count + usize::from(!corpus.entries.is_empty())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    #[should_panic = "cannot be delta8-coded"]
    fn a_delta8_middle_stream_is_rejected() {
        // The decoder refuses to undelta the middle stream, so the rules parser has to
        // reject the configuration instead of emitting a table the runtime cannot read.
        let data_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../zeta-unicode/data");
        let text =
            std::fs::read_to_string(data_dir.join("derived-names.rules")).expect("rules file");
        // The vendored file already configures `mids`; swap its transform instead of
        // adding a second directive.
        let text = text
            .lines()
            .filter(|line| !line.starts_with("@stream mids"))
            .collect::<Vec<_>>()
            .join("\n");
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("derived-names.rules");
        std::fs::write(&path, format!("{text}\n@stream mids delta8 16 16\n")).expect("rules file");

        let _ = rules::read_rules(&path);
    }
}
