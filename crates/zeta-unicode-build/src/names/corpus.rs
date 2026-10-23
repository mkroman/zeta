//! The name corpus: `DerivedName.txt` plus the folded `NameAliases.txt` names.

/// `U+AC00`, the first Hangul syllable; syllables are derived, never stored.
pub const HANGUL_START: u32 = 0xAC00;
/// `U+D7A3`, the last Hangul syllable.
pub const HANGUL_END: u32 = 0xD7A3;
/// Hangul syllables in Unicode 18.0.0, asserted while parsing.
pub const HANGUL_COUNT: usize = 11_172;

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
/// The corpus is every code point that carries a `Name` value of its own, minus
/// the Hangul syllables and minus every `*` pattern (19 ranges plus the
/// single-code-point one, U+18CFF).  All three are reconstructed at lookup time.
#[must_use]
pub fn read_corpus(names_text: &str, aliases_text: &str) -> Corpus {
    let unicode = header_version(names_text);
    let mut entries: Vec<(u32, String)> = Vec::with_capacity(35_400);
    let mut patterns: Vec<Pattern> = Vec::new();
    let mut hangul = 0usize;
    let mut single_patterns = 0usize;

    for (number, line) in names_text.lines().enumerate() {
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
    for line in aliases_text
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
