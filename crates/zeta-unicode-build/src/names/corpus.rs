//! The name corpus: `DerivedName.txt` plus the folded `NameAliases.txt` names.

use zeta_unicode_format::{HANGUL_COUNT, HANGUL_END, HANGUL_START};

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
    /// Aliases folded in from `NameAliases.txt`.
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

/// Parses `DerivedName.txt` (the `names_text`) and folds in the `NameAliases.txt` names
/// (`aliases_text`).
///
/// `version` is the version `ucd::read_inputs` already validated across every vendored file.
/// The corpus is every code point that carries a `Name` value of its own, minus the Hangul
/// syllables and minus every `*` pattern (19 ranges plus the single-code-point one, U+18CFF).
/// All three are reconstructed at lookup time.
#[must_use]
pub fn read_corpus(names_text: &str, aliases_text: &str, version: (u8, u8, u8)) -> Corpus {
    let unicode = format!("{}.{}.{}", version.0, version.1, version.2);
    let ParsedNames {
        mut entries,
        patterns,
        hangul,
        single_patterns,
    } = parse_names(names_text);
    assert_eq!(
        hangul, HANGUL_COUNT,
        "unexpected number of Hangul syllables"
    );

    let aliases = fold_aliases_into(&mut entries, aliases_text);

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

/// The `DerivedName.txt` records split into entries, patterns and the two skipped counts.
struct ParsedNames {
    entries: Vec<(u32, String)>,
    patterns: Vec<Pattern>,
    hangul: usize,
    single_patterns: usize,
}

/// Parses every record of `DerivedName.txt`, leaving the Hangul-count assertion to the caller.
fn parse_names(text: &str) -> ParsedNames {
    let mut entries: Vec<(u32, String)> = Vec::with_capacity(35_400);
    let mut patterns: Vec<Pattern> = Vec::new();
    let mut hangul = 0usize;
    let mut single_patterns = 0usize;

    for (line, record) in crate::ucd::records(text) {
        let source = Source {
            file: "DerivedName.txt",
            line,
        };
        let (lhs, rhs) = record
            .split_once(';')
            .unwrap_or_else(|| panic!("{source}: no field separator"));
        let lhs = lhs.trim();
        let name = rhs.trim();
        if let Some((lo, hi)) = lhs.split_once("..") {
            let (prefix, suffix) = name
                .split_once('*')
                .unwrap_or_else(|| panic!("{source}: pattern without '*'"));
            patterns.push(Pattern {
                lo: parse_cp(lo, source),
                hi: parse_cp(hi, source),
                prefix: prefix.to_owned(),
                suffix: suffix.to_owned(),
            });
            continue;
        }
        let cp = parse_cp(lhs, source);
        if (HANGUL_START..=HANGUL_END).contains(&cp) {
            assert!(
                name.starts_with("HANGUL SYLLABLE "),
                "{source}: unexpected Hangul name for U+{cp:04X}: {name}"
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
                .unwrap_or_else(|| panic!("{source}: pattern without '*'"));
            patterns.push(Pattern {
                lo: cp,
                hi: cp,
                prefix: prefix.to_owned(),
                suffix: suffix.to_owned(),
            });
            single_patterns += 1;
            continue;
        }
        assert!(
            name.len() <= 255,
            "{source}: name for U+{cp:04X} exceeds 255 bytes"
        );
        entries.push((cp, name.to_owned()));
    }
    patterns.sort_by_key(|p| p.lo);
    ParsedNames {
        entries,
        patterns,
        hangul,
        single_patterns,
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
///
/// Aliases of the same kind keep the first row in `NameAliases.txt` order, which
/// the runtime test suite pins: U+0009 must answer `CHARACTER TABULATION`, the
/// file's first `control` alias, not its second.
fn fold_aliases_into(entries: &mut Vec<(u32, String)>, aliases_text: &str) -> usize {
    let mut best: Vec<(u32, u8, &str)> = Vec::new();
    for (line, record) in crate::ucd::records(aliases_text) {
        let source = Source {
            file: "NameAliases.txt",
            line,
        };
        let mut fields = record.split(';');
        let cp = parse_cp(fields.next().unwrap_or_default(), source);
        let alias = fields
            .next()
            .unwrap_or_else(|| panic!("{source}: missing alias"))
            .trim();
        let kind = fields
            .next()
            .unwrap_or_else(|| panic!("{source}: missing alias kind"))
            .trim();
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

/// A UCD file name and 1-based physical line number, for diagnostics.
#[derive(Clone, Copy)]
struct Source<'a> {
    file: &'a str,
    line: usize,
}

impl std::fmt::Display for Source<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.file, self.line)
    }
}

fn parse_cp(text: &str, source: Source<'_>) -> u32 {
    let text = text.trim();
    // `ucd::hex_u32` cannot name the record it came from, so reject a bad value here
    // with the source location before handing the known-valid text to the shared parser.
    match u32::from_str_radix(text, 16) {
        Ok(_) => crate::ucd::hex_u32(text),
        Err(error) => panic!("{source}: bad code point {text:?}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsed_records_skip_comments_and_blank_lines() {
        let parsed = parse_names(
            "\
# DerivedName-18.0.0.txt

0020; SPACE
# 0041; LATIN CAPITAL LETTER A
3400..3401; CJK UNIFIED IDEOGRAPH-*
",
        );

        assert_eq!(parsed.entries, vec![(0x20, "SPACE".to_owned())]);
        assert_eq!(parsed.patterns.len(), 1);
        assert_eq!(parsed.patterns[0].lo, 0x3400);
        assert_eq!(parsed.patterns[0].hi, 0x3401);
        assert_eq!(parsed.hangul, 0);
        assert_eq!(parsed.single_patterns, 0);
    }

    #[test]
    #[should_panic(expected = "DerivedName.txt:4: bad code point \"ZZZZ\"")]
    fn a_malformed_name_code_point_names_derived_name() {
        parse_names("# header\n\n0020; SPACE\nZZZZ; BROKEN\n");
    }

    #[test]
    #[should_panic(expected = "NameAliases.txt:3: bad code point \"NOTHEX\"")]
    fn a_malformed_alias_code_point_names_name_aliases() {
        fold_aliases_into(&mut Vec::new(), "# header\n\nNOTHEX; NULL; control\n");
    }

    #[test]
    fn a_single_code_point_and_a_one_code_point_range_parse_alike() {
        let single = parse_names("3400; CJK UNIFIED IDEOGRAPH-*\n");
        let range = parse_names("3400..3400; CJK UNIFIED IDEOGRAPH-*\n");

        assert_eq!(single.patterns, range.patterns);
        assert_eq!(single.entries, range.entries);
        assert_eq!(single.hangul, range.hangul);
        assert_eq!(single.single_patterns, 1);
        assert_eq!(range.single_patterns, 0);
    }
}
