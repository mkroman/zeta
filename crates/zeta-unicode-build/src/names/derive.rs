//! Arm replay and the generated derivation tree.

use std::fmt::Write as _;

use super::corpus::Corpus;
use super::rules::{Arm, Op, Rules};

/// Appends the name of `cp`, built from `ops`, to `out`.
pub fn build_name(atoms: &[String], ops: &[Op], cp: u32, out: &mut Vec<u8>) {
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
                let value = u32::try_from(value)
                    .unwrap_or_else(|_| panic!("hex hole at U+{cp:04X} does not fit in u32"));
                let width = u8::try_from(width).expect("hex width fits in a byte");
                let mut digits = [0u8; 8];
                let used = zeta_unicode_format::write_hex(&mut digits, value, width);
                out.extend_from_slice(&digits[..used]);
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
pub fn derive_all(rules: &Rules, corpus: &Corpus) -> Vec<bool> {
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

/// Emits the derivation tree: one `&'static str` per atom and a balanced
/// compare tree over the arms.
///
/// The tree runs once per name when the table is constructed, never on a lookup,
/// so the arm count cannot affect lookup latency.  It is `include!`d from
/// `src/`; the atoms become `.rodata` and the tree `.text`, neither of which is
/// charged to the table budget by the incumbent accounting. Hex holes call the
/// consumer's `crate::names::push_hex_string`, so a rule and a `*` pattern are
/// rendered by the same formatter.
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
                    let _ = write!(
                        body,
                        " crate::names::push_hex_string(out, (c) - ({off}), {width});"
                    );
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
    let hits = (0..=max_id).filter(|id| tree_covers(&leaves, *id)).count();
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
fn tree_covers(leaves: &[TreeLeaf], id: u32) -> bool {
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
        tree_covers(&leaves[..k], id)
    } else {
        tree_covers(&leaves[k..], id)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_name_pads_hex_to_the_requested_width() {
        let mut out = Vec::new();
        build_name(&[], &[Op::Hex { off: 0, width: 4 }], 0x41, &mut out);
        assert_eq!(out, b"0041");

        out.clear();
        build_name(&[], &[Op::Hex { off: 0, width: 0 }], 0x41, &mut out);
        assert_eq!(out, b"41");
    }
}
