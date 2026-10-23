//! Deterministic, dependency-light builder for the `zeta-unicode` name table.
//!
//! This module is the `build.rs` side of approach #6 ("names as code").  The
//! checked-in rules file (`data/derived-names.rules`) lists, per code-point range, how a
//! Unicode character name can be *reconstructed* from its code point, so those
//! names never enter the compressed streams.  Everything else is front-coded and
//! compressed with PPMd8-I through `ppmd-rust`, the same pure-Rust codec the
//! runtime decoder uses.
//!
//! Nothing is derived here beyond what the rules state: [`emit`] parses the
//! rules, replays every arm against `data/DerivedName.txt`, and **panics unless
//! every reconstructed name is byte-identical to the file**.  A rules file that
//! has drifted from Unicode is a build failure, not a warning.
//!
//! # Inputs
//!
//! * `data/DerivedName.txt` — the ground truth: every individually named code
//!   point, plus the `*`-pattern ranges (which stay algorithmic at lookup time).
//! * `data/NameAliases.txt` — only with [`Request::fold_aliases`], which
//!   reproduces the incumbent behaviour of folding control names in (35,417
//!   entries instead of 35,351).
//! * `data/derived-names.rules` — the rules file (grammar below).
//!
//! # Rules-file grammar
//!
//! Line oriented, ASCII, no dependencies.  Blank lines and `#` comments are
//! ignored, `@` lines are directives, `A<dddd>` lines are atoms, and everything
//! else is an arm.
//!
//! ```text
//! @unicode 18.0.0                                # informational, cross-checked
//! @corpus 35351                                  # entries the rules were mined against
//! @derived 6677                                  # entries the arms reconstruct
//! @arms 5286                                     # informational
//! @atoms 324                                     # informational
//! @stream <label> <transform> <order> <mem_mib>  # one per name stream
//! @runs <order> <mem_mib>                        # PPMd params for the index section
//! @patterns <order> <mem_mib>                    # PPMd params for the pattern section
//!
//! A0000 <token>                                  # atom 0, verbatim
//! ...
//!
//! 0041:005A fga+65                               # arm over U+0041..=U+005A
//! ```
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
//! [`emit`] writes into `out_dir`:
//!
//! * `names.bin` — the `UNNM` container: a 50-byte header and three PPMd8-I
//!   streams (`plen`, `slen`, `mids`).
//! * `runs.bin` — the name index (`cp` → `name_id`), varint-coded, then
//!   PPMd8-I.
//! * `patterns.bin` — the `*`-pattern ranges, varint-coded, then PPMd8-I. This is a
//!   diagnostic check; the table ships the plain [`pattern_section`] layout.
//! * `expected.txt` — the corpus as `<hex cp>\t<name>`, the decoder's ground truth.
//! * `gen.rs` — the generated derivation tree (`.text` + `.rodata`), for
//!   `include!` from `src/`.
//!
//! and returns a [`Ledger`] with every size, so `build.rs` can assert against
//! [`Request::budget`].  Each of `runs.bin` and `patterns.bin` is a `u32` raw
//! length followed by the compressed stream; the name blob carries those
//! lengths in its header instead, which is why only the two sections pay 4 B.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

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
/// `u32` raw-length prefix on the two standalone sections.
const SECTION_PREFIX_LEN: usize = 6;
/// `U+AC00`, the first Hangul syllable; syllables are derived, never stored.
const HANGUL_START: u32 = 0xAC00;
/// `U+D7A3`, the last Hangul syllable.
const HANGUL_END: u32 = 0xD7A3;
/// Hangul syllables in Unicode 18.0.0, asserted while parsing.
const HANGUL_COUNT: usize = 11_172;
/// Lowest code point the pattern section can describe (the CJK plane start).
const PATTERN_BASE: u32 = 0x3400;
/// Budget the name sections must fit into unless the caller says otherwise.
pub const DEFAULT_BUDGET: usize = 73_304;

/// The three name streams, in container order.
pub const STREAMS: [&str; 3] = ["plen", "slen", "mids"];

// -------------------------------------------------------------------- input

/// Everything [`emit`] needs, plus the knobs it must not decide itself.
#[derive(Clone, Debug)]
pub struct Request {
    /// Directory holding the vendored UCD files (`data/`).
    pub data_dir: PathBuf,
    /// The checked-in rules file.
    pub rules: PathBuf,
    /// Directory for the emitted artifacts; created if missing.
    pub out_dir: PathBuf,
    /// Where to write the generated derivation tree; `None` skips it.
    pub gen_rs: Option<PathBuf>,
    /// Fold `NameAliases.txt` control names into the corpus, as the incumbent
    /// `build.rs` does (35,417 entries instead of 35,351).
    pub fold_aliases: bool,
    /// Round-trip the emitted artifact in-process before returning.
    pub verify: bool,
    /// Fail if the shipped names + index + patterns exceed this many bytes.
    pub budget: usize,
    /// Print the ledger to stderr.
    pub report: bool,
}

impl Request {
    /// The defaults the crate wants: aliases folded, verified, budget-checked,
    /// reporting on, and the tree written next to the other artifacts.
    #[must_use]
    pub fn new(
        data_dir: impl Into<PathBuf>,
        rules: impl Into<PathBuf>,
        out_dir: impl Into<PathBuf>,
    ) -> Self {
        let out_dir = out_dir.into();
        Self {
            data_dir: data_dir.into(),
            rules: rules.into(),
            gen_rs: Some(out_dir.join("gen.rs")),
            out_dir,
            fold_aliases: true,
            verify: true,
            budget: DEFAULT_BUDGET,
            report: true,
        }
    }
}

// ------------------------------------------------------------------- output

/// Every size one build produces, in bytes, plus the `*`-pattern records the caller emits.
#[derive(Clone, Debug, Default)]
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
    /// The name index section.
    pub runs: SectionSize,
    /// The compressed diagnostic pattern stream (`patterns.bin`); the table ships
    /// [`Self::pattern_section`] instead.
    pub patterns: SectionSize,
    /// Consecutive-code-point runs in the index.
    pub runs_count: usize,
    /// `*`-pattern ranges.
    pub patterns_count: usize,
    /// The plain `*`-pattern section the table actually ships, exactly as
    /// [`pattern_section`] encodes it.
    pub pattern_section: Vec<u8>,
    /// `blob + runs + pattern_section`, the number the budget is about.
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
    /// Bytes the codec produced, excluding the `u32` raw-length prefix.
    pub compressed: usize,
    /// `PPMd` model order.
    pub order: u32,
    /// `PPMd` memory limit, in MiB.
    pub mem_mib: u32,
}

// ------------------------------------------------------------------- rules

/// One op of an arm's recipe, with hole values already made absolute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Push a literal token.
    Push(u32),
    /// Push the byte `cp + delta`.
    Char(i64),
    /// Push uppercase hex of `cp - off`, zero-padded to `width`.
    Hex {
        /// Subtracted from the code point.
        off: i64,
        /// Minimum digit count; `0` means no padding.
        width: u32,
    },
}

/// One arm: a code-point range plus the recipe every code point in it shares.
#[derive(Clone, Debug)]
pub struct Arm {
    /// First code point, which is also the first code point of its entry id.
    pub cp_lo: u32,
    /// Last code point, inclusive.
    pub cp_hi: u32,
    /// The recipe; exactly one op is a hole.
    pub ops: Vec<Op>,
}

/// `PPMd` parameters for one stream.
#[derive(Clone, Copy, Debug)]
pub struct StreamParams {
    /// Whether the stream is cumulative-sum coded before compression.
    pub delta8: bool,
    /// Model order.
    pub order: u32,
    /// Memory limit in MiB.
    pub mem_mib: u32,
}

/// A parsed rules file.
#[derive(Clone, Debug)]
pub struct Rules {
    /// Literal tokens, in file order.
    pub atoms: Vec<String>,
    /// Arms, sorted by `cp_lo`, disjoint.
    pub arms: Vec<Arm>,
    /// `PPMd` parameters per name stream, in container order.
    pub streams: [StreamParams; 3],
    /// `PPMd` parameters for the name index.
    pub runs: (u32, u32),
    /// `PPMd` parameters for the pattern section.
    pub patterns: (u32, u32),
    /// `@unicode`, informational.
    pub unicode: String,
    /// `@corpus`, checked against the parsed corpus.
    pub corpus: usize,
    /// `@derived`, checked against the arms.
    pub derived: usize,
}

/// The 52 single-character atom codes, in atom-id order.
const ALPHABET: &[u8; 52] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// The recipe code for atom `id`, mirroring the writer in `mk_rules.py`.  Used
/// by [`expand`] and by the verification that the file round-trips.
#[must_use]
pub fn atom_code(id: usize) -> String {
    if id < 52 {
        String::from_utf8_lossy(&ALPHABET[id..=id]).into_owned()
    } else if id < 104 {
        format!("'{}", String::from_utf8_lossy(&ALPHABET[id - 52..id - 51]))
    } else {
        let i = id - 104;
        format!(
            "''{}{}",
            String::from_utf8_lossy(&ALPHABET[(i / 52)..=(i / 52)]),
            String::from_utf8_lossy(&ALPHABET[(i % 52)..=(i % 52)])
        )
    }
}

/// Expands a rules file into its readable form: one arm per line with the
/// resolved code-point range and the tokens it pushes.  This is the review aid
/// for a recipe syntax that is deliberately terse; the build itself never calls
/// it.
#[must_use]
pub fn expand(rules: &Rules, limit: usize) -> String {
    let mut out = String::new();
    for (n, arm) in rules.arms.iter().enumerate() {
        if n >= limit {
            let _ = writeln!(out, "... and {} more arms", rules.arms.len() - limit);
            break;
        }
        let mut recipe = String::new();
        for op in &arm.ops {
            match *op {
                Op::Push(id) => {
                    let _ = write!(
                        recipe,
                        "{}={:?} ",
                        atom_code(usize::try_from(id).unwrap_or(0)),
                        rules.atoms[usize::try_from(id).unwrap_or(0)]
                    );
                }
                Op::Char(delta) => {
                    let _ = write!(recipe, "[cp{delta:+}] ");
                }
                Op::Hex { off, width } => {
                    let _ = write!(recipe, "[hex(cp-{off})/{width}] ");
                }
            }
        }
        let range = if arm.cp_hi == arm.cp_lo {
            format!("U+{:04X}", arm.cp_lo)
        } else {
            format!("U+{:04X}..=U+{:04X}", arm.cp_lo, arm.cp_hi)
        };
        let _ = writeln!(out, "{range:<24} {recipe}");
    }
    out
}

/// Parses the rules file.  Panics with `path:line` on any error.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one directive arm per line, and each has its own message"
)]
pub fn read_rules(path: &Path) -> Rules {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read rules {}: {e}", path.display()));
    let mut atoms: Vec<String> = Vec::new();
    let mut arms: Vec<Arm> = Vec::new();
    let mut streams: [Option<StreamParams>; 3] = [None; 3];
    let mut runs = None;
    let mut patterns = None;
    let mut unicode = String::new();
    let mut corpus = 0usize;
    let mut derived = 0usize;

    for (number, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let where_ = format!("{}:{}", path.display(), number + 1);
        if let Some(directive) = line.strip_prefix('@') {
            let mut fields = directive.split_whitespace();
            let key = fields.next().unwrap_or_default();
            let args: Vec<&str> = fields.collect();
            match key {
                "unicode" => unicode = args.join(" "),
                "corpus" => {
                    corpus = args
                        .first()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or_else(|| panic!("{where_}: @corpus needs a count"));
                }
                "derived" => {
                    derived = args
                        .first()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or_else(|| panic!("{where_}: @derived needs a count"));
                }
                "arms" | "atoms" => {}
                "stream" => {
                    let [label, transform, order, mem] = args[..] else {
                        panic!("{where_}: @stream needs <label> <transform> <order> <mem_mib>");
                    };
                    let slot = STREAMS
                        .iter()
                        .position(|s| *s == label)
                        .unwrap_or_else(|| panic!("{where_}: unknown stream {label}"));
                    assert!(
                        streams[slot].is_none(),
                        "{where_}: stream {label} configured twice"
                    );
                    streams[slot] = Some(StreamParams {
                        delta8: match transform {
                            "raw" => false,
                            "delta8" => true,
                            other => panic!("{where_}: unknown transform {other}"),
                        },
                        order: order
                            .parse()
                            .unwrap_or_else(|_| panic!("{where_}: bad order {order}")),
                        mem_mib: mem
                            .parse()
                            .unwrap_or_else(|_| panic!("{where_}: bad mem_mib {mem}")),
                    });
                }
                "runs" | "patterns" => {
                    let [order, mem] = args[..] else {
                        panic!("{where_}: @{key} needs <order> <mem_mib>");
                    };
                    let pair = (
                        order
                            .parse()
                            .unwrap_or_else(|_| panic!("{where_}: bad order {order}")),
                        mem.parse()
                            .unwrap_or_else(|_| panic!("{where_}: bad mem_mib {mem}")),
                    );
                    if key == "runs" {
                        runs = Some(pair);
                    } else {
                        patterns = Some(pair);
                    }
                }
                other => panic!("{where_}: unknown directive @{other}"),
            }
            continue;
        }
        if line.len() > 5
            && line.starts_with('A')
            && line.as_bytes()[1..5].iter().all(u8::is_ascii_digit)
            && line.as_bytes()[5] == b' '
        {
            let id: usize = line[1..5]
                .parse()
                .unwrap_or_else(|_| panic!("{where_}: bad atom id"));
            assert_eq!(
                id,
                atoms.len(),
                "{where_}: atom ids must be dense and ordered"
            );
            let token = line[6..].to_owned();
            assert!(!token.is_empty(), "{where_}: atom {id} is empty");
            atoms.push(token.clone());
            continue;
        }
        arms.push(parse_arm(line, &where_));
    }

    for (slot, params) in streams.iter().enumerate() {
        assert!(
            params.is_some(),
            "no @stream directive for {}",
            STREAMS[slot]
        );
    }
    let streams = [
        streams[0].expect("checked"),
        streams[1].expect("checked"),
        streams[2].expect("checked"),
    ];
    for arm in arms.windows(2) {
        assert!(arm[0].cp_lo <= arm[0].cp_hi, "arm range is reversed");
        assert!(
            arm[0].cp_hi < arm[1].cp_lo,
            "arms overlap or are out of order"
        );
    }
    let covered: usize = arms
        .iter()
        .map(|a| usize::try_from(a.cp_hi - a.cp_lo + 1).expect("arm fits"))
        .sum();
    if derived != 0 {
        assert_eq!(
            covered, derived,
            "@derived says {derived}, arms cover {covered}"
        );
    }
    Rules {
        atoms,
        arms,
        streams,
        runs: runs.unwrap_or((10, 1)),
        patterns: patterns.unwrap_or((10, 1)),
        unicode,
        corpus,
        derived,
    }
}

/// Parses one `<cp_lo>[:<cp_hi>] <recipe>` line into an [`Arm`].
fn parse_arm(line: &str, where_: &str) -> Arm {
    let (head, recipe) = line
        .split_once(' ')
        .unwrap_or_else(|| panic!("{where_}: arm has no recipe: {line}"));
    let (cp_lo, cp_hi) = if let Some((lo, hi)) = head.split_once(':') {
        (hex_u32(lo, where_), hex_u32(hi, where_))
    } else {
        let cp = hex_u32(head, where_);
        (cp, cp)
    };
    assert!(cp_lo <= cp_hi, "{where_}: reversed range {head}");

    let bytes = recipe.as_bytes();
    let mut i = 0usize;
    let mut ops: Vec<Op> = Vec::new();
    let mut holes = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                let (id, used) = read_atom_code(recipe, i, where_);
                ops.push(Op::Push(u32::try_from(id).expect("fewer than 2^32 atoms")));
                i += used;
            }
            b'+' | b'-' => {
                let (value, used) = read_signed(recipe, i, where_);
                // The file states the byte at `cp_lo`; make it absolute.
                ops.push(Op::Char(value - i64::from(cp_lo)));
                holes += 1;
                i += used;
            }
            b'=' => {
                let (value, used) = read_hex_value(recipe, i + 1, where_);
                let (width, extra) = if recipe.as_bytes().get(i + used + 1) == Some(&b',') {
                    let (w, n) = read_decimal(recipe, i + used + 2, where_);
                    (u32::try_from(w).expect("width fits in u32"), n + 2)
                } else {
                    (0, 0)
                };
                // The arm's smallest hex value is the one at `cp_lo`, and that is
                // exactly `value`; `read_hex_value` cannot produce a negative, so
                // every code point in the arm gives the hex writer a non-negative
                // argument.  `off` may be negative (e.g. `ARABIC LETTER -DAD`).
                let off = i64::from(cp_lo) - value;
                debug_assert!(value >= 0 && off <= i64::from(cp_lo));
                ops.push(Op::Hex { off, width });
                holes += 1;
                i += used + extra;
            }
            letter => {
                let id = ALPHABET
                    .iter()
                    .position(|c| *c == letter)
                    .unwrap_or_else(|| {
                        panic!("{where_}: unexpected byte {letter:?} in recipe {recipe:?}")
                    });
                ops.push(Op::Push(u32::try_from(id).expect("fewer than 2^32 atoms")));
                i += 1;
            }
        }
    }
    assert_eq!(
        holes, 1,
        "{where_}: an arm needs exactly one hole, found {holes}"
    );
    Arm { cp_lo, cp_hi, ops }
}

fn hex_u32(text: &str, where_: &str) -> u32 {
    u32::from_str_radix(text, 16)
        .unwrap_or_else(|e| panic!("{where_}: bad code point {text:?}: {e}"))
}

fn read_atom_code(recipe: &str, at: usize, where_: &str) -> (usize, usize) {
    let bytes = recipe.as_bytes();
    assert_eq!(bytes[at], b'\'', "{where_}: escaped atom expected");
    let letter = |offset: usize| -> usize {
        bytes
            .get(at + offset)
            .and_then(|b| ALPHABET.iter().position(|c| c == b))
            .unwrap_or_else(|| panic!("{where_}: truncated escaped atom in {recipe:?}"))
    };
    if bytes.get(at + 1) == Some(&b'\'') {
        (104 + letter(2) * 52 + letter(3), 4)
    } else {
        (52 + letter(1), 2)
    }
}

fn read_signed(text: &str, at: usize, where_: &str) -> (i64, usize) {
    let (magnitude, used) = read_decimal(text, at + 1, where_);
    if text.as_bytes()[at] == b'-' {
        (-magnitude, used + 1)
    } else {
        (magnitude, used + 1)
    }
}

fn read_decimal(text: &str, at: usize, where_: &str) -> (i64, usize) {
    let mut value = 0i64;
    let mut n = 0usize;
    for &byte in &text.as_bytes()[at..] {
        if !byte.is_ascii_digit() {
            break;
        }
        value = value * 10 + i64::from(byte - b'0');
        n += 1;
    }
    assert!(n > 0, "{where_}: number expected at byte {at} of {text:?}");
    (value, n)
}

fn read_hex_value(text: &str, at: usize, where_: &str) -> (i64, usize) {
    let mut value = 0i64;
    let mut n = 0usize;
    for &byte in &text.as_bytes()[at..] {
        let digit = match byte {
            b'0'..=b'9' => i64::from(byte - b'0'),
            b'A'..=b'F' => i64::from(byte - b'A') + 10,
            b'a'..=b'f' => i64::from(byte - b'a') + 10,
            _ => break,
        };
        value = value * 16 + digit;
        n += 1;
    }
    assert!(
        n > 0,
        "{where_}: hex value expected at byte {at} of {text:?}"
    );
    (value, n)
}

// ------------------------------------------------------------------ corpus

/// One `*`-pattern range: `prefix + hex(cp) + suffix`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    /// First code point.
    pub lo: u32,
    /// Last code point, inclusive.
    pub hi: u32,
    /// Text before the `*`.
    pub prefix: String,
    /// Text after the `*`.
    pub suffix: String,
}

/// The name corpus: every individually named code point, in ascending order.
#[derive(Clone, Debug)]
pub struct Corpus {
    /// `(code point, name)`, ascending by code point.
    pub entries: Vec<(u32, String)>,
    /// The `*`-pattern ranges, which stay algorithmic at lookup time.
    pub patterns: Vec<Pattern>,
    /// Hangul syllables skipped while parsing.
    pub hangul: usize,
    /// Single-code-point `*` patterns skipped (U+18CFF in Unicode 18.0.0).
    pub single_patterns: usize,
    /// Aliases folded in (0 unless [`Request::fold_aliases`]).
    pub aliases: usize,
    /// The file's Unicode version header.
    pub unicode: String,
    /// `cp` → entry id, or `None`.
    id_of: Vec<u32>,
}

impl Corpus {
    /// Index of `cp` in the name corpus, if it has one.
    #[must_use]
    pub fn id_of(&self, cp: u32) -> Option<u32> {
        let id = self.id_of.get(usize::try_from(cp).ok()?).copied()?;
        (id != u32::MAX).then_some(id)
    }
}

/// Reads `DerivedName.txt` (and optionally `NameAliases.txt`) into a [`Corpus`].
///
/// The corpus is every code point that carries a `Name` value of its own, minus
/// the Hangul syllables and minus every `*` pattern (19 ranges plus the
/// single-code-point one, U+18CFF).  All three are reconstructed at lookup time.
#[must_use]
pub fn read_corpus(data_dir: &Path, fold_aliases: bool) -> Corpus {
    let text = fs::read_to_string(data_dir.join("DerivedName.txt"))
        .unwrap_or_else(|e| panic!("cannot read DerivedName.txt: {e}"));
    let unicode = header_version(&text);
    let mut entries: Vec<(u32, String)> = Vec::with_capacity(35_400);
    let mut patterns: Vec<Pattern> = Vec::new();
    let mut hangul = 0usize;
    let mut single_patterns = 0usize;

    for (number, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (lhs, rhs) = line
            .split_once(';')
            .unwrap_or_else(|| panic!("DerivedName.txt:{}: no field separator", number + 1));
        let lhs = lhs.trim();
        let name = rhs.trim();
        if let Some((lo, hi)) = lhs.split_once("..") {
            let (prefix, suffix) = name
                .split_once('*')
                .unwrap_or_else(|| panic!("DerivedName.txt:{}: pattern without '*'", number + 1));
            patterns.push(Pattern {
                lo: parse_cp(lo, number),
                hi: parse_cp(hi, number),
                prefix: prefix.to_owned(),
                suffix: suffix.to_owned(),
            });
            continue;
        }
        let cp = parse_cp(lhs, number);
        if (HANGUL_START..=HANGUL_END).contains(&cp) {
            assert!(
                name.starts_with("HANGUL SYLLABLE "),
                "unexpected Hangul name for U+{cp:04X}: {name}"
            );
            hangul += 1;
            continue;
        }
        if name.contains('*') {
            // The single-code-point `*` pattern (U+18CFF in Unicode 18.0.0).
            // It is not a spelled-out name, so it stays out of NAMES -- but it
            // does get a PATTERNS entry, which the incumbent's range-only
            // `parse_patterns` misses, leaving `name(0x18CFF)` answering
            // "...CHARACTER-*" with a literal asterisk.
            let (prefix, suffix) = name
                .split_once('*')
                .unwrap_or_else(|| panic!("DerivedName.txt:{}: pattern without '*'", number + 1));
            patterns.push(Pattern {
                lo: cp,
                hi: cp,
                prefix: prefix.to_owned(),
                suffix: suffix.to_owned(),
            });
            single_patterns += 1;
            continue;
        }
        assert!(name.len() <= 255, "name for U+{cp:04X} exceeds 255 bytes");
        entries.push((cp, name.to_owned()));
    }
    assert_eq!(
        hangul, HANGUL_COUNT,
        "unexpected number of Hangul syllables"
    );
    patterns.sort_by_key(|p| p.lo);

    let aliases = if fold_aliases {
        fold_aliases_into(&mut entries, data_dir)
    } else {
        0
    };

    let mut id_of = vec![u32::MAX; 0x0011_0000];
    for (id, (cp, _)) in entries.iter().enumerate() {
        let slot = &mut id_of[usize::try_from(*cp).expect("code point fits")];
        assert_eq!(*slot, u32::MAX, "duplicate code point U+{cp:04X}");
        *slot = u32::try_from(id).expect("fewer than 2^32 names");
    }
    Corpus {
        entries,
        patterns,
        hangul,
        single_patterns,
        aliases,
        unicode,
        id_of,
    }
}

/// Preference order for character name aliases: the long `control` name wins over
/// the `abbreviation` Unicode also records for the same code point.
fn alias_rank(kind: &str) -> u8 {
    match kind {
        "control" => 0,
        "correction" => 1,
        "alternate" => 2,
        "figment" => 3,
        "abbreviation" => 4,
        _ => 5,
    }
}

/// Adds `NameAliases.txt` names for code points that have none, keeping the list
/// sorted by code point.  Returns how many were added.
fn fold_aliases_into(entries: &mut Vec<(u32, String)>, data_dir: &Path) -> usize {
    let text = fs::read_to_string(data_dir.join("NameAliases.txt"))
        .unwrap_or_else(|e| panic!("cannot read NameAliases.txt: {e}"));
    let mut best: Vec<(u32, u8, &str)> = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let mut fields = line.split(';');
        let cp = parse_cp(fields.next().unwrap_or_default(), 0);
        let alias = fields.next().expect("alias").trim();
        let kind = fields.next().expect("alias kind").trim();
        let rank = alias_rank(kind);
        match best.iter_mut().find(|(c, _, _)| *c == cp) {
            Some(slot) if rank < slot.1 => {
                slot.1 = rank;
                slot.2 = alias;
            }
            Some(_) => {}
            None => best.push((cp, rank, alias)),
        }
    }
    best.sort_by_key(|(cp, _, _)| *cp);

    let mut added = 0usize;
    let mut cursor = 0usize;
    for (cp, _, alias) in best {
        while cursor < entries.len() && entries[cursor].0 < cp {
            cursor += 1;
        }
        if cursor < entries.len() && entries[cursor].0 == cp {
            continue;
        }
        entries.insert(cursor, (cp, alias.to_owned()));
        added += 1;
    }
    added
}

fn parse_cp(text: &str, number: usize) -> u32 {
    u32::from_str_radix(text.trim(), 16).unwrap_or_else(|e| {
        panic!(
            "DerivedName.txt:{}: bad code point {text:?}: {e}",
            number + 1
        )
    })
}

/// The `# DerivedName-18.0.0.txt` header.
fn header_version(text: &str) -> String {
    for line in text.lines().take(8) {
        if let Some(rest) = line.strip_prefix("# DerivedName-")
            && let Some(version) = rest.strip_suffix(".txt")
        {
            return version.to_owned();
        }
    }
    panic!("no version header in DerivedName.txt");
}

// ------------------------------------------------------------------- codec

/// PPMd8-I at `order`/`mem_mib` through `ppmd-rust`, the same crate the
/// runtime decoder uses.
fn ppmd8i(data: &[u8], order: u32, mem_mib: u32) -> Vec<u8> {
    let mut encoder = ppmd_rust::Ppmd8Encoder::new(
        Vec::new(),
        order,
        mem_mib << 20,
        ppmd_rust::RestoreMethod::Restart,
    )
    .expect("ppmd-rust accepts the configured order and memory");
    encoder.write_all(data).expect("in-memory write");
    encoder.finish(false).expect("ppmd-rust finish")
}

/// A standalone section: `u32 raw length`, `u8 order`, `u8 memory MiB`, then the PPMd8-I stream.
///
/// The codec parameters live in the section rather than only in the rules file, so a decoder
/// cannot silently disagree with whatever the rules were mined for.
fn section_bytes(raw: &[u8], order: u32, mem_mib: u32) -> Vec<u8> {
    let compressed = ppmd8i(raw, order, mem_mib);
    let mut out = Vec::with_capacity(SECTION_PREFIX_LEN + compressed.len());
    out.extend_from_slice(
        &u32::try_from(raw.len())
            .expect("raw length fits in u32")
            .to_le_bytes(),
    );
    out.push(u8::try_from(order).expect("PPMd order fits in a byte"));
    out.push(u8::try_from(mem_mib).expect("PPMd memory fits in a byte"));
    out.extend_from_slice(&compressed);
    out
}

/// The codec parameters a section was written with, read back from its prefix.
#[must_use]
pub fn section_codec(bytes: &[u8]) -> (u32, u32) {
    assert!(bytes.len() > SECTION_PREFIX_LEN, "truncated section");
    (
        u32::from(bytes[SECTION_PREFIX_LEN - 2]),
        u32::from(bytes[SECTION_PREFIX_LEN - 1]),
    )
}

/// The codec parameters a standalone section was written with, needed to decode
/// it.  Both sections use fixed parameters, recorded in the rules file.
#[derive(Clone, Copy, Debug, Default)]
pub struct SectionCodec {
    /// `PPMd` model order.
    pub order: u32,
    /// `PPMd` memory limit, in MiB.
    pub mem_mib: u32,
}

/// LEB128: seven payload bits per byte, high bit set on every byte but the last.
///
/// The `as u8` casts cannot truncate: `value` is below 128 inside the loop and below 128 after
/// it.
fn varint(mut value: u64, out: &mut Vec<u8>) {
    #[allow(clippy::cast_possible_truncation)]
    while value >= 128 {
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    #[allow(clippy::cast_possible_truncation)]
    out.push(value as u8);
}

// --------------------------------------------------------------- derivation

/// Appends the name of `cp`, built from `ops`, to `out`.
fn build_name(atoms: &[String], ops: &[Op], cp: u32, out: &mut Vec<u8>) {
    for op in ops {
        match *op {
            Op::Push(id) => {
                out.extend_from_slice(atoms[usize::try_from(id).expect("atom id fits")].as_bytes());
            }
            Op::Char(delta) => {
                let byte = i64::from(cp) + delta;
                out.push(u8::try_from(byte).unwrap_or_else(|_| {
                    panic!("char hole at U+{cp:04X} wants byte {byte}, which is not a byte")
                }));
            }
            Op::Hex { off, width } => {
                let value = i64::from(cp) - off;
                assert!(value >= 0, "hex hole at U+{cp:04X} is negative");
                let mut text = [0u8; 16];
                let mut n = text.len();
                let mut v = value;
                loop {
                    n -= 1;
                    let d = usize::try_from(v & 15).expect("nibble fits");
                    // `d` is a nibble, so both casts are in range.
                    #[allow(clippy::cast_possible_truncation)]
                    let digit = if d < 10 {
                        b'0' + d as u8
                    } else {
                        b'A' + (d as u8 - 10)
                    };
                    text[n] = digit;
                    v >>= 4;
                    if v == 0 {
                        break;
                    }
                }
                for _ in n..usize::try_from(width).expect("width fits") {
                    out.push(b'0');
                }
                out.extend_from_slice(&text[n..]);
            }
        }
    }
}

/// The arm covering `cp`, if any.  Arms are sorted and disjoint, so this is a
/// binary search; the shipped code uses the generated tree instead.
#[must_use]
pub fn arm_of(rules: &Rules, cp: u32) -> Option<&Arm> {
    let index = rules
        .arms
        .partition_point(|a| a.cp_lo <= cp)
        .checked_sub(1)?;
    let arm = &rules.arms[index];
    (cp <= arm.cp_hi).then_some(arm)
}

/// Replays every arm over the corpus and returns the derived flag per entry.
///
/// Panics on the first mismatch: a rule that no longer reproduces Unicode's name
/// byte-for-byte is a build failure, not a warning.
fn derive_all(rules: &Rules, corpus: &Corpus) -> Vec<bool> {
    let mut derived = vec![false; corpus.entries.len()];
    let mut replayed = 0usize;
    let mut name: Vec<u8> = Vec::with_capacity(255);
    for arm in &rules.arms {
        // Every code point of an arm is in the corpus and their ids are
        // consecutive, which is what lets the tree compute `cp_lo + (id - id_lo)`.
        let id_lo = corpus
            .id_of(arm.cp_lo)
            .unwrap_or_else(|| panic!("arm covers U+{:04X}, which has no name", arm.cp_lo));
        for (offset, cp) in (arm.cp_lo..=arm.cp_hi).enumerate() {
            let id = corpus
                .id_of(cp)
                .unwrap_or_else(|| panic!("arm covers U+{cp:04X}, which has no name"));
            assert_eq!(
                id,
                id_lo + u32::try_from(offset).expect("offset fits"),
                "U+{cp:04X} breaks the arm's id contiguity"
            );
            let want = &corpus.entries[usize::try_from(id).expect("id fits")].1;
            name.clear();
            build_name(&rules.atoms, &arm.ops, cp, &mut name);
            assert_eq!(
                name.as_slice(),
                want.as_bytes(),
                "arm U+{:04X}..=U+{:04X} rebuilds U+{cp:04X} as {:?}, want {want:?}",
                arm.cp_lo,
                arm.cp_hi,
                String::from_utf8_lossy(&name)
            );
            let slot = &mut derived[usize::try_from(id).expect("id fits")];
            assert!(!*slot, "two arms cover U+{cp:04X}");
            *slot = true;
            replayed += 1;
        }
    }
    assert_eq!(
        replayed, rules.derived,
        "the arms cover {replayed} entries but @derived says {}",
        rules.derived
    );
    if rules.corpus != 0 {
        // `@corpus` fingerprints the corpus the arms were mined against.  With
        // aliases folded in the corpus grows (and every id after the first
        // control character shifts), which is harmless because arms are keyed by
        // code point, not by id -- but then it is only a lower bound.
        if corpus.aliases == 0 {
            assert_eq!(
                rules.corpus,
                corpus.entries.len(),
                "@corpus says {} entries, DerivedName.txt gives {}",
                rules.corpus,
                corpus.entries.len()
            );
        } else {
            assert!(
                corpus.entries.len() > rules.corpus,
                "@corpus says {} entries, but only {} were parsed",
                rules.corpus,
                corpus.entries.len()
            );
        }
    }
    derived
}

// -------------------------------------------------------------- decompression

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
    let mut off = 17usize;
    // All three descriptors come first, then the three streams: read the
    // directory before slicing any payload.
    let mut descriptors = [(0usize, 0usize, 0u32, 0u32); 3];
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
            1,
            "stream {}: only PPMd8 variant I is supported",
            STREAMS[idx]
        );
        off += STREAM_DESC_LEN;
    }
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(3);
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
            let mut previous = 0u8;
            for byte in &mut buffer {
                previous = previous.wrapping_add(*byte);
                *byte = previous;
            }
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

// ------------------------------------------------------------------- verify

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
    let mut value = 0u32;
    let mut shift = 0u32;
    for (n, &byte) in bytes.iter().enumerate() {
        value |= u32::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return (value, n + 1);
        }
        shift += 7;
    }
    panic!("truncated varint");
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

/// Round-trips the emitted artifact and checks every claim this build makes:
/// names byte-identical to `DerivedName.txt`, code points exact, index and
/// pattern sections consistent.  Panics on the first failure.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one assertion per claim, kept together so the order is readable"
)]
pub fn verify(out_dir: &Path, corpus: &Corpus, rules: &Rules) -> Verify {
    let start = std::time::Instant::now();
    let blob = fs::read(out_dir.join("names.bin")).expect("read names.bin");
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
        let Some(arm) = arm_of(rules, cp) else {
            return false;
        };
        build_name(&rules.atoms, &arm.ops, cp, out);
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
        &fs::read(out_dir.join("runs.bin")).expect("read runs.bin"),
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
        &fs::read(out_dir.join("patterns.bin")).expect("read patterns.bin"),
        SectionCodec {
            order: rules.patterns.0,
            mem_mib: rules.patterns.1,
        },
    ));
    assert_eq!(patterns.len(), corpus.patterns.len(), "pattern count");
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
        let (index, syllable) = hangul_parts(cp);
        assert_eq!(index * 588 + syllable, cp - HANGUL_START, "Hangul split");
        assert!(
            corpus.id_of(cp).is_none(),
            "U+{cp:04X} is a Hangul syllable and must not be stored"
        );
    }

    Verify {
        compared: corpus.entries.len(),
        derived: seen,
        stored: corpus.entries.len() - seen,
        index_hits: corpus.entries.len(),
        pattern_cps,
        patterns: patterns.len(),
        millis: start.elapsed().as_secs_f64() * 1e3,
    }
}

/// Hangul syllable decomposition `(leading, vowel-trailing)`.
#[must_use]
pub const fn hangul_parts(cp: u32) -> (u32, u32) {
    const V_COUNT: u32 = 21;
    const T_COUNT: u32 = 28;
    const N_COUNT: u32 = V_COUNT * T_COUNT;
    let s = cp - HANGUL_START;
    (s / N_COUNT, s % N_COUNT)
}

// ------------------------------------------------------------------- emitter

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
    let derived = derive_all(rules, corpus);
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

/// Sweeps PPMd8-I parameters per stream and prints the table, so the rules file
/// can carry a measured choice instead of a guessed one.  Diagnostic only: the
/// build reads the parameters from the rules file.
pub fn sweep(corpus: &Corpus, rules: &Rules) {
    let (_, plen, slen, mids, _) = front_code(rules, corpus);
    let streams = [
        ("plen", vec![delta8(&plen), plen.clone()]),
        ("slen", vec![slen]),
        ("mids", vec![mids]),
    ];
    // ppmd-rust caps PPMd8 at order 16 (`PPMD8_MAX_ORDER`); the C encoder #6 used
    // allows 64, so the sweep stops where the decoder stops.
    let orders = [4u32, 6, 8, 10, 12, 16];
    let mems = [1u32, 4, 16, 64];
    for (name, variants) in streams {
        println!("{name}:");
        let mut best = (usize::MAX, String::new());
        for (index, variant) in variants.iter().enumerate() {
            let transform = if name == "plen" && index == 0 {
                "delta8"
            } else {
                "raw"
            };
            let mut rows: Vec<(usize, u32, u32, &str)> = Vec::new();
            for order in orders {
                for mem in mems {
                    rows.push((ppmd8i(variant, order, mem).len(), order, mem, transform));
                }
            }
            rows.sort_unstable();
            for (size, order, mem, transform) in rows.iter().take(6) {
                println!("    {transform:<7} {size:>7?}  o{order}/m{mem}M");
            }
            if rows[0].0 < best.0 {
                best = (
                    rows[0].0,
                    format!("o{}/m{}M/{}", rows[0].1, rows[0].2, rows[0].3),
                );
            }
        }
        println!("  best: {} = {best:?}", best.1);
    }
    for (label, raw) in [
        ("RUNS", runs_stream(corpus)),
        ("PATTERNS", patterns_stream(corpus)),
        (
            "PATTERNS without U+18CFF",
            patterns_stream_with(corpus, true),
        ),
    ] {
        println!("{label}: raw {} bytes", raw.len());
        let mut rows: Vec<(usize, u32, u32)> = Vec::new();
        for order in orders {
            for mem in mems {
                rows.push((ppmd8i(&raw, order, mem).len(), order, mem));
            }
        }
        rows.sort_unstable();
        for (size, order, mem) in rows.iter().take(6) {
            println!("    {size:>7?}  o{order}/m{mem}M");
        }
    }
}

/// Builds every artifact and returns the ledger.
#[allow(
    clippy::too_many_lines,
    reason = "the five artifacts in the order they are written"
)]
pub fn emit(request: &Request) -> Ledger {
    let corpus = read_corpus(&request.data_dir, request.fold_aliases);
    let rules = read_rules(&request.rules);
    if !rules.unicode.is_empty() {
        assert_eq!(
            corpus.unicode, rules.unicode,
            "the rules are for a different Unicode release than DerivedName.txt"
        );
    }
    fs::create_dir_all(&request.out_dir).expect("create out_dir");
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
        let source: Vec<u8> = match idx {
            0 if params.delta8 => delta8(&plen),
            0 => plen.clone(),
            1 => slen.clone(),
            _ => mids.clone(),
        };
        let compressed = ppmd8i(&source, params.order, params.mem_mib);
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
    fs::write(request.out_dir.join("names.bin"), &blob).expect("write names.bin");

    // ---- the name index ----------------------------------------------------
    let runs_raw = runs_stream(&corpus);
    let runs_bytes = section_bytes(&runs_raw, rules.runs.0, rules.runs.1);
    fs::write(request.out_dir.join("runs.bin"), &runs_bytes).expect("write runs.bin");

    // ---- the pattern section ----------------------------------------------
    let patterns_raw = patterns_stream(&corpus);
    let patterns_bytes = section_bytes(&patterns_raw, rules.patterns.0, rules.patterns.1);
    fs::write(request.out_dir.join("patterns.bin"), &patterns_bytes).expect("write patterns.bin");

    // ---- ground truth and the generated tree -------------------------------
    let mut expected = String::with_capacity(1_140_000);
    for (cp, name) in &corpus.entries {
        let _ = writeln!(expected, "{cp:X}\t{name}");
    }
    fs::write(request.out_dir.join("expected.txt"), expected).expect("write expected.txt");
    let gen_rs = emit_gen_rs(&rules, &corpus);
    if let Some(path) = &request.gen_rs {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create the gen.rs directory");
        }
        fs::write(path, &gen_rs).expect("write gen.rs");
    }

    let streams_total: usize = sizes.iter().map(|s| s.compressed).sum();
    let atom_bytes: usize = rules.atoms.iter().map(String::len).sum();
    let runs_size = runs_bytes.len();
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
        blob: blob.len(),
        streams_total,
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
        table: blob.len() + runs_size + pattern_section_len,
        table_strict: blob.len() + runs_size + pattern_section_len + atom_bytes,
    };
    if request.report {
        report(&ledger, &corpus, &rules, request);
    }
    assert!(
        ledger.table <= request.budget,
        "names + index + patterns = {} B, over the {} B budget",
        ledger.table,
        request.budget
    );
    if request.verify {
        let result = verify(&request.out_dir, &corpus, &rules);
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
    }
    ledger
}

fn delta8(values: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len());
    let mut previous = 0u8;
    for &value in values {
        out.push(value.wrapping_sub(previous));
        previous = value;
    }
    out
}

fn report(ledger: &Ledger, corpus: &Corpus, rules: &Rules, request: &Request) {
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
        fs::metadata(&request.rules).map_or(0, |m| usize::try_from(m.len()).unwrap_or(0))
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
        "  RUNS      {:>8?} B  ({} runs, raw {:?} -> {:?} + 4, o{}/m{}M)",
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
    eprintln!(
        "  TABLE     {:>8?} B  (budget {})",
        ledger.table, request.budget
    );
    eprintln!(
        "  strict    {:>8?} B  (the same plus the {} B of atom literals)",
        ledger.table_strict, ledger.atom_bytes
    );
}

// ------------------------------------------------------- generated Rust tree

/// Emits the derivation tree: one `&'static str` per atom and a balanced
/// compare tree over the arms.
///
/// The tree runs once per name when the table is constructed, never on a lookup,
/// so the arm count cannot affect lookup latency.  It is `include!`d from
/// `src/`; the atoms become `.rodata` and the tree `.text`, neither of which is
/// charged to the table budget by the incumbent accounting.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one `writeln!` per line of generated Rust"
)]
pub fn emit_gen_rs(rules: &Rules, corpus: &Corpus) -> String {
    // Atoms in a stable order (longest first), so the table looks the same for
    // the same rules file.
    let mut order: Vec<usize> = (0..rules.atoms.len()).collect();
    order.sort_by(|a, b| {
        rules.atoms[*b]
            .len()
            .cmp(&rules.atoms[*a].len())
            .then_with(|| rules.atoms[*a].cmp(&rules.atoms[*b]))
    });
    let mut slot_of = vec![0usize; rules.atoms.len()];
    for (slot, id) in order.iter().enumerate() {
        slot_of[*id] = slot;
    }

    let mut leaves: Vec<TreeLeaf> = Vec::with_capacity(rules.arms.len());
    for arm in &rules.arms {
        let id_lo = corpus
            .id_of(arm.cp_lo)
            .unwrap_or_else(|| panic!("arm covers U+{:04X}, which has no name", arm.cp_lo));
        let id_hi = id_lo + (arm.cp_hi - arm.cp_lo);
        let mut body = format!("{{ let c = {}i64 + (id as i64 - {id_lo});", arm.cp_lo);
        for op in &arm.ops {
            match *op {
                Op::Push(id) => {
                    let slot = slot_of[usize::try_from(id).expect("atom id fits")];
                    let _ = write!(body, " out.push_str(A{slot});");
                }
                Op::Char(delta) => {
                    let _ = write!(body, " out.push((((c) + ({delta})) as u8) as char);");
                }
                Op::Hex { off, width } => {
                    let _ = write!(body, " write_hex(out, (c) - ({off}), {width});");
                }
            }
        }
        body.push_str(" true }");
        leaves.push(TreeLeaf {
            lo: id_lo,
            hi: id_hi,
            body,
        });
    }
    for pair in leaves.windows(2) {
        assert!(pair[0].hi < pair[1].lo, "two arms overlap in id space");
    }
    // The root range spans *every* entry id, including the stored ones after the
    // last arm.  Clamping it to the last arm's id would let the rightmost
    // subtree claim `leaf.lo == lo && leaf.hi == hi` and then derive the stored
    // ids above it -- the tree self-check below is what catches that.
    let max_id =
        u32::try_from(corpus.entries.len().saturating_sub(1)).expect("fewer than 2^32 names");
    assert!(
        leaves.last().is_none_or(|l| l.hi <= max_id),
        "an arm covers an id past the end of the corpus"
    );
    let covered: usize = leaves
        .iter()
        .map(|l| usize::try_from(l.hi - l.lo + 1).expect("arm fits"))
        .sum();

    // Self-check: simulate the tree exactly as the emitted Rust evaluates it, so
    // a dispatch bug cannot reach the compiler.
    let hits = (0..=max_id)
        .filter(|id| tree_covers(&leaves, (0, max_id), *id))
        .count();
    assert_eq!(
        hits, covered,
        "the tree dispatch covers {hits} ids, the arms cover {covered}"
    );

    let mut out = String::with_capacity(1_200_000);
    let _ = writeln!(
        out,
        "// @generated by build.rs from derived-names.rules -- do not edit."
    );
    let _ = writeln!(
        out,
        "// Every name reachable from here is CONSTRUCTED from its code point;"
    );
    let _ = writeln!(out, "// none of its text is in the table.");
    let _ = writeln!(out, "#[allow(clippy::all)] // @generated");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "/// Merged literals (`.rodata`; the linker shares byte-identical ones)."
    );
    for (slot, id) in order.iter().enumerate() {
        // Per-item allows, not `#![...]`: the file is `include!`d, and a macro
        // expansion cannot carry inner attributes.
        let _ = writeln!(out, "#[allow(clippy::all, dead_code)]");
        let _ = writeln!(
            out,
            "const A{slot}: &str = \"{}\";",
            escape(&rules.atoms[*id])
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "/// Upper-case hex, zero-padded to `width` (0 = no padding)."
    );
    let _ = writeln!(out, "#[allow(clippy::all)]");
    let _ = writeln!(out, "#[inline(never)]");
    let _ = writeln!(
        out,
        "fn write_hex(out: &mut String, mut v: i64, width: u32) {{"
    );
    let _ = writeln!(out, "    let mut buf = [0u8; 16];");
    let _ = writeln!(out, "    let mut n = 0usize;");
    let _ = writeln!(out, "    loop {{");
    let _ = writeln!(out, "        let d = (v & 15) as usize;");
    let _ = writeln!(
        out,
        "        buf[n] = if d < 10 {{ b'0' + d as u8 }} else {{ b'A' + (d as u8 - 10) }};"
    );
    let _ = writeln!(out, "        n += 1;");
    let _ = writeln!(out, "        v >>= 4;");
    let _ = writeln!(out, "        if v == 0 {{ break; }}");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "    for _ in n..width as usize {{ out.push('0'); }}");
    let _ = writeln!(
        out,
        "    while n > 0 {{ n -= 1; out.push(buf[n] as char); }}"
    );
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "/// Materialise the name of `id` (its rank in code-point order)."
    );
    let _ = writeln!(
        out,
        "/// Returns `false` when the entry is stored in the table instead."
    );
    let _ = writeln!(out, "#[allow(clippy::all)]");
    let _ = writeln!(
        out,
        "pub fn derive_at(id: u32, out: &mut String) -> bool {{"
    );
    let _ = writeln!(out, "    {}", tree(&leaves, (0, max_id)));
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let () = eprintln!("  gen.rs: {covered} ids over {} arms", leaves.len());
    out
}

/// One arm of the generated comparison tree, with its body already rendered.
struct TreeLeaf {
    /// First name id the arm covers.
    lo: u32,
    /// Last name id the arm covers.
    hi: u32,
    /// The rendered `{ let c = ...; ...; true }` block.
    body: String,
}

/// Renders the comparison tree over `leaves[lo_index..]`, whose search range is
/// `[range.0, range.1]`.
///
/// A subtree holding a single arm is emitted as a bare body only when the search
/// range is exactly that arm's range: the subtree is entered for *every* id in
/// the range's half-line, so a bare body would derive the stored ids around the
/// arm.  Otherwise the body gets an explicit bounds check.
#[allow(
    clippy::only_used_in_recursion,
    reason = "the range decides each leaf's bounds check"
)]
fn tree(leaves: &[TreeLeaf], range: (u32, u32)) -> String {
    if leaves.is_empty() {
        return "false".to_owned();
    }
    if leaves.len() == 1 {
        let leaf = &leaves[0];
        if leaf.lo == range.0 && leaf.hi == range.1 {
            return leaf.body.clone();
        }
        return format!(
            "if id < {} {{ false }} else if id <= {} {{ {} }} else {{ false }}",
            leaf.lo, leaf.hi, leaf.body
        );
    }
    let k = leaves.len() / 2;
    let mid = leaves[k].lo;
    format!(
        "if id < {mid} {{ {} }} else {{ {} }}",
        tree(&leaves[..k], (range.0, mid - 1)),
        tree(&leaves[k..], (mid, range.1))
    )
}

/// Whether the emitted tree would derive `id`: the same walk as [`tree`], used as
/// the build's self-check.
fn tree_covers(leaves: &[TreeLeaf], range: (u32, u32), id: u32) -> bool {
    if leaves.is_empty() {
        return false;
    }
    if leaves.len() == 1 {
        let leaf = &leaves[0];
        return leaf.lo <= id && id <= leaf.hi;
    }
    let k = leaves.len() / 2;
    let mid = leaves[k].lo;
    if id < mid {
        tree_covers(&leaves[..k], (range.0, mid - 1), id)
    } else {
        tree_covers(&leaves[k..], (mid, range.1), id)
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            c => {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
            }
        }
    }
    out
}

/// Writes the raw and compressed name streams to `dir`, so a codec problem can be
/// isolated from the container plumbing.  Diagnostic only.
pub fn dump_streams(corpus: &Corpus, rules: &Rules, dir: &Path) {
    let (_, plen, slen, mids, _) = front_code(rules, corpus);
    fs::create_dir_all(dir).expect("create dir");
    for (idx, raw) in [plen, slen, mids].into_iter().enumerate() {
        let params = rules.streams[idx];
        let source = if idx == 0 && params.delta8 {
            delta8(&raw)
        } else {
            raw
        };
        fs::write(dir.join(format!("{}.raw", STREAMS[idx])), &source).expect("raw");
        let compressed = ppmd8i(&source, params.order, params.mem_mib);
        fs::write(dir.join(format!("{}.bin", STREAMS[idx])), &compressed).expect("bin");
        eprintln!(
            "{}: raw {} -> {} (o{}/m{}M)",
            STREAMS[idx],
            source.len(),
            compressed.len(),
            params.order,
            params.mem_mib
        );
    }
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
            varint(u64::from(len), &mut out);
            varint(u64::from(gap), &mut out);
            run_count += 1;
            run_start = id;
        }
    }
    out
}

fn push_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

/// The shipped `*`-pattern section: `u32 lo`, `u32 hi`, `u16 prefix length`, the
/// prefix, `u16 suffix length`, the suffix — one record per range, in ascending
/// order.
///
/// The decoder scans these on every name miss, so they are stored plainly; the
/// compressed `patterns.bin` below is a diagnostic artifact and a build check.
#[must_use]
pub fn pattern_section(patterns: &[Pattern]) -> Vec<u8> {
    let mut out = Vec::new();
    for pattern in patterns {
        push_u32(&mut out, pattern.lo);
        push_u32(&mut out, pattern.hi);
        push_u16(
            &mut out,
            u16::try_from(pattern.prefix.len()).expect("prefix fits in u16"),
        );
        out.extend_from_slice(pattern.prefix.as_bytes());
        push_u16(
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
        varint(u64::from(pattern.lo - PATTERN_BASE), &mut out);
        varint(u64::from(pattern.hi - pattern.lo + 1), &mut out);
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
pub fn run_count(corpus: &Corpus) -> usize {
    let mut count = 0usize;
    for id in 1..corpus.entries.len() {
        if corpus.entries[id].0 != corpus.entries[id - 1].0 + 1 {
            count += 1;
        }
    }
    count + usize::from(!corpus.entries.is_empty())
}
