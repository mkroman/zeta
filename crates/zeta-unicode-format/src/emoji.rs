//! The emoji grammar shared by the image trainer and the runtime.
//!
//! Most emoji names are not stored. The trainer tries one of these rules against the published
//! CLDR name and stores the rule id (plus a small argument) when it reproduces the name exactly;
//! the runtime replays the same rule. The two sides therefore share the wording of every rule,
//! not just its wire id: [`swap_person`], [`join_family`], [`ucd_mask_name`], [`format_toned`]
//! and the small lookup tables are the single implementation.

/// The five emoji skin-tone code points, in tone order.
pub const TONES: [u32; 5] = [0x1F3FB, 0x1F3FC, 0x1F3FD, 0x1F3FE, 0x1F3FF];

/// The CLDR names of the five skin tones, in the same order as [`TONES`].
pub const TONE_NAMES: [&str; 5] = [
    "light skin tone",
    "medium-light skin tone",
    "medium skin tone",
    "medium-dark skin tone",
    "dark skin tone",
];

const _: () = assert!(TONES.len() == TONE_NAMES.len());

/// Longest canonical emoji key, in bytes.
///
/// The runtime keeps a fixed scratch buffer of this size; the generator rejects a stored key
/// that would not fit.
pub const MAX_KEY: usize = 64;

/// The most code points one stored emoji key may have.
pub const MAX_CPS: usize = 16;

/// The `U+200D` zero-width joiner.
pub const ZWJ: u32 = 0x200D;

/// The number of bits the rule id occupies in a packed emoji entry.
pub const RULE_BITS: u32 = 4;

/// The largest rule argument the packed `(argument << RULE_BITS) | rule` entry encodes.
pub const MAX_RULE_ARGUMENT: u16 = u16::MAX >> RULE_BITS;

/// The emoji name-grammar rule packed in the low four bits of an entry.
///
/// The discriminants are the wire ids: the generator writes them into the rule stream and the
/// runtime reads them back, so they must never change. [`Rule::from_id`] is the only way to turn
/// a decoded id back into a rule.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// `literal`: the name is stored in the literal-name arena.
    Literal = 0,
    /// `ucd_mask`: select words from the lowercase UCD name.
    UcdMask = 1,
    /// `ucd_mask_hyphen`: select words, joining the first two with a hyphen.
    UcdMaskHyphen = 2,
    /// `keycap`: `KEYCAP ` plus the keycap word.
    Keycap = 3,
    /// `gender_w`: swap `person` for `woman` in a base name.
    GenderWoman = 4,
    /// `gender_m`: swap `person` for `man` in a base name.
    GenderMan = 5,
    /// `facing`: base name plus ` facing right`.
    Facing = 6,
    /// `family`: `family: ` plus comma-joined family words.
    Family = 7,
    /// `couple`: `couple with heart: ` plus comma-joined family words.
    Couple = 8,
    /// `kiss`: `kiss: ` plus comma-joined family words.
    Kiss = 9,
    /// `pair`: plural word plus the pair phrase.
    Pair = 10,
    /// `tone_component`: one of the five skin-tone names.
    ToneComponent = 11,
    /// `clock`: the hour word plus ` o’clock`.
    Clock = 12,
    /// `keycap10`: the literal name `keycap: 10`.
    Keycap10 = 13,
}

impl Rule {
    /// The number of grammar rules, literal included.
    pub const COUNT: usize = 14;

    /// Every rule, in wire-id order.
    pub const ALL: [Self; Self::COUNT] = [
        Self::Literal,
        Self::UcdMask,
        Self::UcdMaskHyphen,
        Self::Keycap,
        Self::GenderWoman,
        Self::GenderMan,
        Self::Facing,
        Self::Family,
        Self::Couple,
        Self::Kiss,
        Self::Pair,
        Self::ToneComponent,
        Self::Clock,
        Self::Keycap10,
    ];

    /// The wire id of the rule, as packed into an entry.
    #[must_use]
    pub const fn id(self) -> u8 {
        self as u8
    }

    /// The rule with wire id `id`, or `None` when `id` is not a rule.
    #[must_use]
    pub const fn from_id(id: u8) -> Option<Self> {
        if (id as usize) < Self::COUNT {
            Some(Self::ALL[id as usize])
        } else {
            None
        }
    }

    /// Packs this rule and `argument` into an entry, or `None` when the argument is too wide.
    #[must_use]
    pub const fn pack(self, argument: u16) -> Option<u16> {
        if argument > MAX_RULE_ARGUMENT {
            return None;
        }
        Some((argument << RULE_BITS) | self.id() as u16)
    }

    /// Splits a packed entry back into its rule and argument.
    #[must_use]
    pub const fn unpack(packed: u16) -> Option<(Self, u16)> {
        match Self::from_id((packed & ((1 << RULE_BITS) - 1)) as u8) {
            Some(rule) => Some((rule, packed >> RULE_BITS)),
            None => None,
        }
    }
}

const _: () = assert!(Rule::COUNT <= 1 << RULE_BITS);

/// The index into [`TONES`] of `cp`, or `None` when `cp` is not a skin-tone component.
#[must_use]
pub const fn tone_index(cp: u32) -> Option<usize> {
    let mut index = 0;
    while index < TONES.len() {
        if TONES[index] == cp {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// Is `cp` one of the five emoji skin-tone components?
#[must_use]
pub const fn is_tone(cp: u32) -> bool {
    tone_index(cp).is_some()
}

/// The indices into [`TONES`] of the skin-tone code points in `cps`, in order.
///
/// Returns the indices and their count in a fixed-capacity array, so a lookup with at most two
/// tones does not allocate. Returns `None` as soon as a third tone is seen; callers that need
/// every tone, however many, use [`tone_positions`].
#[must_use]
pub fn tone_indices(cps: impl IntoIterator<Item = u32>) -> Option<([usize; 2], usize)> {
    let mut indices = [0usize; 2];
    let mut count = 0usize;
    for cp in cps {
        let Some(index) = tone_index(cp) else {
            continue;
        };
        *indices.get_mut(count)? = index;
        count += 1;
    }
    Some((indices, count))
}

/// The indices into [`TONES`] of the skin-tone code points in `cps`, in order.
///
/// Unlike [`tone_indices`] this accepts any number of tones, at the cost of allocating.
#[must_use]
pub fn tone_positions(cps: impl IntoIterator<Item = u32>) -> Vec<usize> {
    cps.into_iter().filter_map(tone_index).collect()
}

/// `swap_person` rewrites a `person` base name to the given gender word.
#[must_use]
pub fn swap_person(name: &str, gender: &str) -> String {
    if name == "person" {
        return gender.to_owned();
    }
    if let Some(rest) = name.strip_prefix("person: ") {
        return format!("{gender}: {rest}");
    }
    if let Some(rest) = name.strip_prefix("person ") {
        return format!("{gender} {rest}");
    }
    if let Some(head) = name.strip_suffix(" person") {
        return format!("{head} {gender}");
    }
    format!("{gender} {name}")
}

/// Comma-joins the family words of `cps`, skipping `ignored`.
#[must_use]
pub fn join_family(cps: &[u32], ignored: &[u32]) -> String {
    let mut out = String::new();
    for &cp in cps {
        if ignored.contains(&cp) {
            continue;
        }
        if let Some(word) = family_word(cp) {
            if !out.is_empty() {
                out.push_str(", ");
            }
            out.push_str(word);
        }
    }
    out
}

/// The family-member word of a code point.
#[must_use]
pub const fn family_word(cp: u32) -> Option<&'static str> {
    Some(match cp {
        0x1F9D1 => "adult",
        0x1F9D2 => "child",
        0x1F468 => "man",
        0x1F469 => "woman",
        0x1F466 => "boy",
        0x1F467 => "girl",
        _ => return None,
    })
}

/// The plural word of a pair.
#[must_use]
pub const fn plural(left: u32, right: u32) -> Option<&'static str> {
    Some(match (left, right) {
        (0x1F468, 0x1F468) => "men",
        (0x1F469, 0x1F469) => "women",
        (0x1F9D1, 0x1F9D1) => "people",
        (0x1F469, 0x1F468) => "woman and man",
        _ => return None,
    })
}

/// The phrase a pair's middle code point contributes.
#[must_use]
pub const fn pair_phrase(cp: u32) -> Option<&'static str> {
    Some(match cp {
        0x1F430 => "with bunny ears",
        0x1FAEF => "wrestling",
        0x1F91D => "holding hands",
        _ => return None,
    })
}

/// The keycap word of a base code point.
#[must_use]
pub const fn keycap_word(cp: u32) -> Option<&'static str> {
    Some(match cp {
        0x23 => "NUMBER SIGN",
        0x2A => "ASTERISK",
        0x30 => "DIGIT ZERO",
        0x31 => "DIGIT ONE",
        0x32 => "DIGIT TWO",
        0x33 => "DIGIT THREE",
        0x34 => "DIGIT FOUR",
        0x35 => "DIGIT FIVE",
        0x36 => "DIGIT SIX",
        0x37 => "DIGIT SEVEN",
        0x38 => "DIGIT EIGHT",
        0x39 => "DIGIT NINE",
        _ => return None,
    })
}

/// The `ucd_mask` rule: selects UCD-name words, optionally hyphenating the first two.
#[must_use]
pub fn ucd_mask_name(name: &str, mask: u8, hyphen: bool) -> String {
    let mut out = String::with_capacity(name.len());
    let mut selected = 0usize;
    for (word_index, word) in name.split(' ').enumerate() {
        if word_index >= 8 || mask >> word_index & 1 == 0 {
            continue;
        }
        if selected > 0 {
            out.push(if hyphen && selected == 1 { '-' } else { ' ' });
        }
        out.extend(word.chars().map(|c| c.to_ascii_lowercase()));
        selected += 1;
    }
    out
}

/// The `clock` rule: `CLOCK FACE <HOUR> OCLOCK` becomes `<hour> o’clock`.
#[must_use]
pub fn clock_name(name: &str) -> Option<String> {
    let mut words = name.split(' ');
    let clock = words.next()?;
    let face = words.next()?;
    let hour = words.next()?;
    let oclock = words.next()?;
    if words.next().is_some() || clock != "CLOCK" || face != "FACE" || oclock != "OCLOCK" {
        return None;
    }
    let mut out = String::with_capacity(hour.len() + " o’clock".len());
    out.extend(hour.chars().map(|c| c.to_ascii_lowercase()));
    out.push_str(" o’clock");
    Some(out)
}

/// Renders `base` with `tones` applied, following the 3-bit rule.
///
/// Rule `1` inserts the tone phrase after the first `": "`, rule `2` appends it, and rules
/// `3`/`4` do the same after collapsing consecutive duplicate tones.
///
/// # Panics
///
/// Panics when a tone index is not a position in [`TONES`]; [`tone_indices`] and
/// [`tone_positions`] only produce valid indices.
#[must_use]
pub fn format_toned(code: u8, base: &str, tones: &[usize]) -> String {
    let phrase = tone_phrase(code, tones);

    match (base.find(": "), code) {
        (Some(colon), 1 | 3) => {
            format!("{}: {}, {}", &base[..colon], phrase, &base[colon + 2..])
        }
        (Some(_), _) => format!("{base}, {phrase}"),
        (None, _) => format!("{base}: {phrase}"),
    }
}

/// The comma-joined tone names, collapsing consecutive duplicates for rules 3 and 4.
///
/// # Panics
///
/// Panics when a tone index is not a position in [`TONES`]; [`tone_indices`] and
/// [`tone_positions`] only produce valid indices.
#[must_use]
pub fn tone_phrase(code: u8, tones: &[usize]) -> String {
    let dedup = matches!(code, 3 | 4);
    let mut phrase = String::new();
    let mut previous = None;
    for &index in tones {
        if dedup && previous == Some(index) {
            continue;
        }
        if !phrase.is_empty() {
            phrase.push_str(", ");
        }
        phrase.push_str(TONE_NAMES[index]);
        previous = Some(index);
    }
    phrase
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_positions_finds_every_tone_in_order() {
        assert_eq!(tone_positions([0x1F3FF, 0x41, 0x1F3FB]), [4, 0]);
        assert_eq!(tone_positions(std::iter::empty()), []);
    }

    #[test]
    fn tone_indices_handles_zero_one_and_two_tones() {
        let (indices, count) = tone_indices(std::iter::empty()).expect("no tones is valid");
        assert_eq!((indices, count), ([0, 0], 0));
        assert_eq!(tone_indices([0x41, 0x42]), Some(([0, 0], 0)));
        assert_eq!(tone_indices([0x1F3FF]), Some(([4, 0], 1)));
        assert_eq!(
            tone_indices([0x41, 0x1F3FB, 0x42, 0x1F3FD]),
            Some(([0, 2], 2))
        );
    }

    #[test]
    fn tone_indices_rejects_a_third_tone() {
        assert_eq!(
            tone_indices([0x1F3FB, 0x1F3FC, 0x1F3FD]),
            None,
            "the fixed-capacity array only holds two tones"
        );
    }

    #[test]
    fn is_tone_accepts_only_the_five_components() {
        for tone in TONES {
            assert!(is_tone(tone));
        }
        assert!(!is_tone(0x1F3FA));
        assert!(!is_tone(0x1F400));
    }

    #[test]
    fn formats_a_toned_name_with_every_rule() {
        // Rule 1 inserts after the first `": "`; rule 3 does the same and deduplicates.
        assert_eq!(
            format_toned(1, "handshake: base", &[0]),
            "handshake: light skin tone, base"
        );
        assert_eq!(
            format_toned(3, "women holding hands: base", &[0, 0]),
            "women holding hands: light skin tone, base"
        );
        // Rules 2 and 4 append the phrase.
        assert_eq!(
            format_toned(2, "thumbs up", &[0, 1]),
            "thumbs up: light skin tone, medium-light skin tone"
        );
        assert_eq!(
            format_toned(4, "thumbs up: base", &[2, 2]),
            "thumbs up: base, medium skin tone"
        );
        // A base without a `": "` always appends after a colon.
        assert_eq!(
            format_toned(1, "thumbs up", &[4]),
            "thumbs up: dark skin tone"
        );
    }

    #[test]
    fn a_tone_phrase_keeps_non_consecutive_duplicates() {
        assert_eq!(
            tone_phrase(3, &[0, 1, 0]),
            "light skin tone, medium-light skin tone, light skin tone"
        );
        assert_eq!(tone_phrase(2, &[0, 0]), "light skin tone, light skin tone");
    }

    #[test]
    fn grammar_helpers_reproduce_the_published_names() {
        assert_eq!(swap_person("person", "woman"), "woman");
        assert_eq!(swap_person("person: red hair", "man"), "man: red hair");
        assert_eq!(swap_person("person running", "woman"), "woman running");
        assert_eq!(swap_person("running person", "man"), "running man");
        assert_eq!(swap_person("runner", "woman"), "woman runner");

        assert_eq!(family_word(0x1F469), Some("woman"));
        assert_eq!(family_word(0x1F994), None);
        assert_eq!(plural(0x1F469, 0x1F468), Some("woman and man"));
        assert_eq!(plural(0x1F994, 0x1F994), None);
        assert_eq!(pair_phrase(0x1F91D), Some("holding hands"));
        assert_eq!(keycap_word(0x23), Some("NUMBER SIGN"));
        assert_eq!(keycap_word(0x41), None);

        assert_eq!(
            join_family(&[0x1F469, ZWJ, 0x1F466, ZWJ, 0x1F467], &[ZWJ]),
            "woman, boy, girl"
        );
        assert_eq!(
            join_family(&[0x1F469, ZWJ, 0x2764, ZWJ, 0x1F468], &[ZWJ, 0x2764]),
            "woman, man"
        );
    }

    #[test]
    fn ucd_mask_selects_words_and_hyphenates_the_first_pair() {
        assert_eq!(
            ucd_mask_name("COPYRIGHT SIGN", 0b11, false),
            "copyright sign"
        );
        assert_eq!(
            ucd_mask_name("COPYRIGHT SIGN", 0b11, true),
            "copyright-sign"
        );
        assert_eq!(
            ucd_mask_name("ARABIC LIGATURE SALAM", 0b101, true),
            "arabic-salam"
        );
        // Bits past the eighth word are ignored.
        assert_eq!(ucd_mask_name("A B", 0xFF, false), "a b");
        assert_eq!(ucd_mask_name("A B", 0, false), "");
    }

    #[test]
    fn clock_names_require_the_exact_ucd_spelling() {
        assert_eq!(
            clock_name("CLOCK FACE TWO OCLOCK").as_deref(),
            Some("two o’clock")
        );
        assert_eq!(
            clock_name("CLOCK FACE TWELVE OCLOCK").as_deref(),
            Some("twelve o’clock")
        );
        assert_eq!(clock_name("CLOCK FACE TWO"), None);
        assert_eq!(clock_name("CLOCK HAND TWO OCLOCK"), None);
        assert_eq!(clock_name("CLOCK FACE TWO OCLOCK EXTRA"), None);
    }

    #[test]
    fn rule_ids_round_trip_and_reject_unknown_ids() {
        let rules = Rule::ALL;
        assert_eq!(rules.len(), Rule::COUNT);
        for (index, rule) in rules.into_iter().enumerate() {
            assert_eq!(usize::from(rule.id()), index, "{rule:?}");
            assert_eq!(Rule::from_id(rule.id()), Some(rule), "{rule:?}");
        }
        let first_unknown = u8::try_from(Rule::COUNT).expect("the rule count fits in a byte");
        assert_eq!(Rule::from_id(first_unknown), None);
        assert_eq!(Rule::from_id(u8::MAX), None);
    }

    #[test]
    fn packed_entries_round_trip_and_reject_wide_arguments() {
        for rule in Rule::ALL {
            for argument in [0u16, 1, 255, MAX_RULE_ARGUMENT] {
                let packed = rule.pack(argument).expect("the argument fits");
                assert_eq!(Rule::unpack(packed), Some((rule, argument)));
            }
            assert_eq!(rule.pack(MAX_RULE_ARGUMENT + 1), None);
        }
        assert_eq!(
            Rule::unpack(u16::from(
                u8::try_from(Rule::COUNT).expect("the rule count fits in a byte")
            )),
            None,
            "an unknown rule id is rejected"
        );
    }
}
