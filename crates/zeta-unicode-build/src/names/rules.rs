//! The `data/derived-names.rules` file: directives, atoms and derivation arms.

use std::fs;
use std::path::Path;

use super::STREAMS;

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

/// The 52 single-character atom codes, in atom-id order, mirroring the writer in
/// `mk_rules.py`.
const ALPHABET: &[u8; 52] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

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
