//! Indefinite article selection.
//!
//! Ported from the MIT-licensed `indefinite` crate, version 0.1.11. The rule set and the
//! irregular-word table are unchanged; the regex predicates are plain string operations so the
//! bot does not link a regex engine to choose between two articles.
//!
//! The full MIT notice for the ported code, covering this module only, is in
//! [`article.LICENSE`](article.LICENSE) next to this file.

/// The 122 words whose spelling does not follow the leading-letter rule.
///
/// Byte-identical to `indefinite` 0.1.11's table.
const IRREGULAR: [&str; 122] = [
    // Nouns: eu like y
    "eunuch",
    "eucalyptus",
    "eugenics",
    "eulogy",
    "euphemism",
    "euphony",
    "euphoria",
    "eureka",
    // Adjectives: eu like y
    "euro",
    "european",
    "euphemistic",
    "euphonic",
    "euphoric",
    // Adverbs: eu like y
    "euphemistically",
    "euphonically",
    "euphorically",
    // Nouns: silent h
    "heir",
    "heiress",
    "herb",
    "homage",
    "honesty",
    "honor",
    "honour",
    "honoree",
    "hour",
    // Adjectives: silent h
    "honest",
    "honorous",
    "honorific",
    "honorable",
    "honourable",
    // Adverbs: silent h
    "honestly",
    "hourly",
    // Nouns: o like w
    "one",
    "ouija",
    // Adjectives: o like w
    "once",
    // Nouns: u like y
    "ubiquity",
    "udometer",
    "ufo",
    "uke",
    "ukulele",
    "ululate",
    "unicorn",
    "unicycle",
    "uniform",
    "unify",
    "union",
    "unison",
    "unit",
    "unity",
    "universe",
    "university",
    "upas",
    "ural",
    "uranium",
    "urea",
    "ureter",
    "urethra",
    "urine",
    "urologist",
    "urology",
    "urus",
    "usage",
    "use",
    "user",
    "usual",
    "usurp",
    "usurper",
    "usury",
    "utensil",
    "uterus",
    "utility",
    "utopia",
    "utricle",
    "uvarovite",
    "uvea",
    "uvula",
    "utah",
    "utahn",
    // Adjectives: u like y
    "ubiquitous",
    "ugandan",
    "ukrainian",
    "unanimous",
    "unicameral",
    "unified",
    "unique",
    "unisex",
    "universal",
    "urinal",
    "urological",
    "useful",
    "useless",
    "usurious",
    "utilitarian",
    "utopic",
    // Adverbs: u like y
    "ubiquitously",
    "unanimously",
    "unicamerally",
    "uniquely",
    "universally",
    "urologically",
    "usefully",
    "uselessly",
    "usuriously",
    // Nouns: y like i
    "yttria",
    "yggdrasil",
    "ylem",
    "yperite",
    "ytterbia",
    "ytterbium",
    "yttrium",
    // Adjectives: y like i
    "ytterbous",
    "ytterbic",
    "yttric",
    // Single letters
    "f",
    "h",
    "l",
    "m",
    "n",
    "r",
    "s",
    "u",
    "x",
];

/// Returns the indefinite article, `"a"` or `"an"`, that precedes the first word of `phrase`.
///
/// The first word ends at the first whitespace, `'` or `-`. Numbers, acronyms, and the
/// irregular words in the vendored table each follow their own rule.
///
/// # Examples
///
/// ```
/// # use zeta_utils::text::indefinite_article;
/// assert_eq!(indefinite_article("honor"), "an");
/// assert_eq!(indefinite_article("euro"), "a");
/// assert_eq!(indefinite_article("FFA prodigy"), "an");
/// assert_eq!(indefinite_article("8-bit"), "an");
/// ```
#[must_use]
pub fn indefinite_article(phrase: &str) -> &'static str {
    let word = first_word(phrase);

    if starts_with_number(word) {
        number_article(word)
    } else if is_acronym(word) {
        acronym_article(word)
    } else {
        other_article(word)
    }
}

/// Returns the first word of `phrase`: the prefix before any whitespace, `'` or `-`.
fn first_word(phrase: &str) -> &str {
    phrase
        .split(|c: char| c.is_whitespace() || c == '\'' || c == '-')
        .next()
        .unwrap_or_default()
}

/// Reports whether `word` starts with an ASCII vowel.
fn starts_with_vowel(word: &str) -> bool {
    word.as_bytes().first().is_some_and(|byte| {
        matches!(
            byte,
            b'a' | b'e' | b'i' | b'o' | b'u' | b'A' | b'E' | b'I' | b'O' | b'U'
        )
    })
}

/// Reports whether `word` starts with a digit, comma or dot.
fn starts_with_number(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_digit() || c == ',' || c == '.')
}

/// Chooses the article for a number: `8`, and `11`/`18` followed by a multiple-of-three
/// count of further digits, take `an`.
fn number_article(word: &str) -> &'static str {
    // The rule ignores one leading comma or dot when counting digits, but still inspects the
    // original word for the `8`/`11`/`18` prefixes.
    let number = word
        .strip_prefix(',')
        .or_else(|| word.strip_prefix('.'))
        .unwrap_or(word);

    if word.starts_with('8')
        || ((word.starts_with("11") || word.starts_with("18"))
            && (number.len() - 2).is_multiple_of(3))
    {
        "an"
    } else {
        "a"
    }
}

/// Reports whether `word` reads as an acronym: uppercasing it changes nothing.
fn is_acronym(word: &str) -> bool {
    word.to_uppercase().eq(word)
}

/// Chooses the article for an acronym, where the leading letter names itself.
fn acronym_article(word: &str) -> &'static str {
    let leading_letter_names_itself = word.as_bytes().first().is_some_and(|byte| {
        matches!(
            byte,
            b'U' | b'F' | b'H' | b'L' | b'M' | b'N' | b'R' | b'S' | b'X'
        )
    });

    if leading_letter_names_itself == starts_with_vowel(word) {
        "a"
    } else {
        "an"
    }
}

/// Reports whether `word` is one of the words in [`IRREGULAR`], after stripping a common
/// inflection.
fn is_irregular(word: &str) -> bool {
    let mut word = word.to_string();

    if word.len() > 1 {
        for end in ["es", "ed", "s"] {
            if word.ends_with(end) {
                // The vendored rule replaces *all* occurrences of the ending, not only the
                // trailing one, and gives up when the result is shorter than the ending.
                word = word.replace(end, "");

                if word.len() <= end.len() + 1 {
                    return false;
                }
            }
        }
    }

    IRREGULAR.contains(&word.as_str())
}

/// Chooses the article for anything that is not a number or an acronym.
fn other_article(word: &str) -> &'static str {
    let word = &word.to_lowercase();
    let is_irregular = is_irregular(word);
    let starts_with_vowel = starts_with_vowel(word);

    // A leading vowel takes "an" unless the word is irregular, and an irregular word takes
    // "an" even when it starts with a consonant. Exactly one of the two must hold.
    if (starts_with_vowel || is_irregular) && !(starts_with_vowel && is_irregular) {
        "an"
    } else {
        "a"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_articles_for_documented_words() {
        let cases = [
            ("honor", "an"),
            ("honor-bound", "an"),
            ("heir's", "an"),
            ("euro", "a"),
            ("ukulele", "a"),
            ("ouija board", "a"),
            ("FFA prodigy", "an"),
            ("Hour", "an"),
            ("hours", "an"),
            ("heiresses", "an"),
            ("honored", "an"),
            ("red rum", "a"),
            ("Umbrella", "an"),
            ("apple", "an"),
            ("pear", "a"),
            ("", "a"),
            (" ", "a"),
        ];

        for (phrase, expected) in cases {
            assert_eq!(indefinite_article(phrase), expected, "phrase {phrase:?}");
        }
    }

    #[test]
    fn picks_articles_for_numbers() {
        let cases = [
            ("8", "an"),
            ("80", "an"),
            ("800000", "an"),
            ("11", "an"),
            ("110", "a"),
            ("1100", "a"),
            ("11000", "an"),
            ("1800", "a"),
            ("18000", "an"),
            ("11000000000000", "an"),
            ("110000000000000", "a"),
            ("18000000", "an"),
            ("180000000", "a"),
            ("17", "a"),
            ("14", "a"),
            ("1,100", "a"),
            (",8", "a"),
        ];

        for (phrase, expected) in cases {
            assert_eq!(indefinite_article(phrase), expected, "phrase {phrase:?}");
        }
    }

    #[test]
    fn picks_articles_for_single_letters() {
        let cases = [
            ("a", "an"),
            ("b", "a"),
            ("u", "a"),
            ("f", "an"),
            ("h", "an"),
            ("l", "an"),
            ("m", "an"),
            ("n", "an"),
            ("r", "an"),
            ("s", "an"),
            ("x", "an"),
        ];

        for (phrase, expected) in cases {
            assert_eq!(indefinite_article(phrase), expected, "phrase {phrase:?}");
        }
    }

    #[test]
    fn picks_articles_for_acronyms() {
        let cases = [
            ("NASA", "an"),
            ("NHL", "an"),
            ("SSI", "an"),
            ("URSSAF", "a"),
            ("EULA", "an"),
            ("XLM", "an"),
            ("CMYK", "a"),
            ("IOU", "an"),
            ("UFO", "a"),
            ("CEO", "a"),
        ];

        for (phrase, expected) in cases {
            assert_eq!(indefinite_article(phrase), expected, "phrase {phrase:?}");
        }
    }
}
