//! The runtime's decoded emoji section.
//!
//! The wire format and the decode core live in [`zeta_unicode_format::emoji::decode`]; this
//! module re-exports the shared decoder as [`Emoji`], and the name layer fills the
//! [`NameLookup`](zeta_unicode_format::NameLookup) seam. A lookup canonicalizes `U+FE0F`,
//! binary-searches the key arena, and materializes the name through the shared rule renderer;
//! skin-tone variants strip the tones, find the base, and replay the base's 3-bit formatting
//! rule when the 2-bit published-tone mask allows it.
//!
//! The `ucd_mask` and `clock` rules need the character name of one code point.
//! [`NameLookup`](zeta_unicode_format::NameLookup) is that seam: the name layer supplies it, so
//! this module never depends on the name decoder.

pub(crate) use zeta_unicode_format::EmojiDecoder as Emoji;

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use zeta_unicode_format::{
        EMOJI_TONE_CODE_BITS, EMOJI_TONE_MASK_BITS, NameLookup, Rule, ZWJ, huffman, pack_bits,
    };

    use super::*;

    /// A name lookup backed by a small static map.
    struct Names;

    impl NameLookup for Names {
        fn write_name(&mut self, cp: u32, out: &mut String) -> bool {
            let name = match cp {
                0xA9 => "COPYRIGHT SIGN",
                0x1F551 => "CLOCK FACE TWO OCLOCK",
                0x1F44D => "THUMBS UP SIGN",
                _ => return false,
            };
            out.push_str(name);
            true
        }
    }

    /// Builds a section from explicit streams and tone arrays.
    fn section(
        entries: &[(Vec<u32>, Rule, u16)],
        literals: &[&str],
        tones: &[(u8, u8)],
    ) -> Vec<u8> {
        let mut key_raw = Vec::new();
        let mut previous = 0u32;
        let mut rule_raw = Vec::new();
        let mut masks = Vec::new();
        for (cps, rule, argument) in entries {
            let first = cps[0];
            zeta_unicode_format::write_varint(first - previous, &mut key_raw);
            previous = first;
            zeta_unicode_format::write_varint(
                u32::try_from(cps.len()).expect("a key length"),
                &mut key_raw,
            );
            for &cp in cps.iter().skip(1) {
                zeta_unicode_format::write_varint(cp, &mut key_raw);
            }
            rule_raw.push(rule.id());
            if matches!(*rule, Rule::UcdMask | Rule::UcdMaskHyphen) {
                masks.push(u8::try_from(*argument).unwrap());
            }
        }
        let mut name_raw = Vec::new();
        for name in literals {
            name_raw.extend_from_slice(name.as_bytes());
            name_raw.push(0);
        }
        // Drop the final NUL: the decoder splits between names.
        name_raw.pop();
        let streams = [
            (huffman::encode(&key_raw, 0), key_raw.len(), 0u8),
            (huffman::encode(&name_raw, 0), name_raw.len(), 0),
            (huffman::encode(&rule_raw, 0), rule_raw.len(), 0),
            (huffman::encode(&masks, 0), masks.len(), 0),
        ];
        let mut out = Vec::new();
        out.extend_from_slice(&u16::try_from(entries.len()).unwrap().to_le_bytes());
        out.extend_from_slice(&u16::try_from(literals.len()).unwrap().to_le_bytes());
        for (stream, raw_len, order) in &streams {
            out.extend_from_slice(&u16::try_from(*raw_len).unwrap().to_le_bytes());
            out.extend_from_slice(&u16::try_from(stream.len()).unwrap().to_le_bytes());
            out.push(*order);
        }
        for (stream, _, _) in &streams {
            out.extend_from_slice(stream);
        }
        let codes: Vec<u8> = tones.iter().map(|&(code, _)| code).collect();
        let masks: Vec<u8> = tones.iter().map(|&(_, mask)| mask).collect();
        out.extend_from_slice(&pack_bits(codes, EMOJI_TONE_CODE_BITS));
        out.extend_from_slice(&pack_bits(masks, EMOJI_TONE_MASK_BITS));
        out
    }

    #[test]
    fn rules_materialize_without_stored_names() {
        let entries = vec![
            (vec![0x23, 0x20E3], Rule::Keycap, 0),
            (vec![0xA9], Rule::UcdMask, 3),
            (vec![0x1F44D], Rule::Literal, 0),
            (vec![0x1F551], Rule::Clock, 0),
        ];
        let blob = section(&entries, &["thumbs up"], &[(0, 0), (0, 0), (0, 0), (0, 0)]);
        let emoji = Emoji::parse(&blob).expect("the section parses");
        assert_eq!(emoji.entry_count(), 4);
        assert_eq!(
            emoji.emoji_name(&mut Names, "©").as_deref(),
            Some("copyright sign")
        );
        assert_eq!(
            emoji.emoji_name(&mut Names, "🕑").as_deref(),
            Some("two o’clock")
        );
        assert_eq!(
            emoji.emoji_name(&mut Names, "#️⃣").as_deref(),
            Some("KEYCAP NUMBER SIGN")
        );
        assert_eq!(
            emoji.emoji_name(&mut Names, "👍").as_deref(),
            Some("thumbs up")
        );
    }

    #[test]
    fn every_grammar_rule_materializes() {
        // The literal bases the rules reference, plus one entry per rule.
        let mut rows: Vec<(Vec<u32>, Rule, u16, Option<&str>)> = vec![
            (vec![0x1F3C3], Rule::Literal, 0, Some("person running")),
            (vec![0x1F3C3, ZWJ, 0x27A1], Rule::Facing, 0, None),
            (vec![0x1F3FB], Rule::ToneComponent, 0, None),
            (
                vec![0x1F468, ZWJ, 0x1F469, ZWJ, 0x1F466],
                Rule::Family,
                0,
                None,
            ),
            (
                vec![0x1F469, ZWJ, 0x2764, ZWJ, 0x1F48B, ZWJ, 0x1F468],
                Rule::Kiss,
                0,
                None,
            ),
            (
                vec![0x1F469, ZWJ, 0x2764, ZWJ, 0x1F468],
                Rule::Couple,
                0,
                None,
            ),
            (
                vec![0x1F469, ZWJ, 0x1F91D, ZWJ, 0x1F468],
                Rule::Pair,
                0,
                None,
            ),
            (
                vec![0x1F469, ZWJ, 0x1F9B0, ZWJ, 0x2640],
                Rule::GenderWoman,
                1,
                None,
            ),
            (vec![0x1F51F], Rule::Keycap10, 0, None),
            (vec![0x1F9D1], Rule::Literal, 0, Some("person")),
            (
                vec![0x1F9D1, ZWJ, 0x1F9B0],
                Rule::Literal,
                0,
                Some("person: red hair"),
            ),
        ];
        rows.sort_by_key(|(cps, _, _, _)| {
            cps.iter()
                .flat_map(|&cp| {
                    let mut buffer = [0u8; 4];
                    char::from_u32(cp)
                        .expect("scalar value")
                        .encode_utf8(&mut buffer)
                        .as_bytes()
                        .to_vec()
                })
                .collect::<Vec<u8>>()
        });
        let entries: Vec<(Vec<u32>, Rule, u16)> = rows
            .iter()
            .map(|(cps, rule, argument, _)| (cps.clone(), *rule, *argument))
            .collect();
        let literals: Vec<&str> = rows
            .iter()
            .filter_map(|(_, rule, _, name)| (*rule == Rule::Literal).then_some(*name).flatten())
            .collect();
        let tones = vec![(0u8, 0u8); entries.len()];
        let blob = section(&entries, &literals, &tones);
        let emoji = Emoji::parse(&blob).expect("the section parses");
        for (key, want) in [
            ("🏃‍➡", "person running facing right"),
            ("🏻", "light skin tone"),
            ("👨‍👩‍👦", "family: man, woman, boy"),
            ("👩‍❤‍💋‍👨", "kiss: woman, man"),
            ("👩‍❤‍👨", "couple with heart: woman, man"),
            ("👩‍🤝‍👨", "woman and man holding hands"),
            ("👩‍🦰‍♀", "woman: red hair"),
            ("🔟", "keycap: 10"),
        ] {
            assert_eq!(
                emoji.emoji_name(&mut Names, key).as_deref(),
                Some(want),
                "{key:?}"
            );
        }
    }

    #[test]
    fn an_adult_headed_gender_rule_does_not_rewrite_its_base() {
        // The trainer never assigns a gender rule to a `person` head, so the runtime's head
        // guard answers `None` for this corrupt entry instead of rewriting it to itself.
        let entries = vec![(vec![0x1F9D1, ZWJ, 0x1F9B0], Rule::GenderWoman, 0)];
        let blob = section(&entries, &[], &[(0, 0)]);
        let emoji = Emoji::parse(&blob).expect("the section parses");
        assert_eq!(emoji.emoji_name(&mut Names, "🧑‍🦰"), None);
    }

    #[test]
    fn a_gender_rule_rewrites_a_man_or_woman_head_to_the_person_base() {
        // Both roles resolve through the person base one level down; an adult head carrying
        // the rule instead is rejected by the head guard
        // (`an_adult_headed_gender_rule_does_not_rewrite_its_base`).
        let entries = vec![
            (vec![0x1F468, ZWJ, 0x1F9B0], Rule::GenderMan, 0),
            (vec![0x1F469, ZWJ, 0x1F9B0], Rule::GenderWoman, 0),
            (vec![0x1F9D1, ZWJ, 0x1F9B0], Rule::Literal, 0),
        ];
        let blob = section(&entries, &["person: red hair"], &[(0, 0), (0, 0), (0, 0)]);
        let emoji = Emoji::parse(&blob).expect("the section parses");
        assert_eq!(
            emoji.emoji_name(&mut Names, "👨‍🦰").as_deref(),
            Some("man: red hair")
        );
        assert_eq!(
            emoji.emoji_name(&mut Names, "👩‍🦰").as_deref(),
            Some("woman: red hair")
        );
    }

    #[test]
    fn tone_variants_use_the_published_rule_and_mask() {
        let entries = vec![(vec![0x1F44D], Rule::Literal, 0)];
        let blob = section(&entries, &["thumbs up"], &[(2, 1)]);
        let emoji = Emoji::parse(&blob).expect("the section parses");
        assert_eq!(
            emoji.emoji_name(&mut Names, "👍🏻").as_deref(),
            Some("thumbs up: light skin tone")
        );
        assert_eq!(emoji.emoji_name(&mut Names, "👍🏻🏿"), None);
        assert_eq!(emoji.emoji_name(&mut Names, "👍"), Some("thumbs up".into()));
        assert!(matches!(
            emoji.emoji_name(&mut Names, "👍🏻"),
            Some(Cow::Owned(_))
        ));
    }

    #[test]
    fn canonicalization_and_negative_cases_match_the_incumbent() {
        let entries = vec![(vec![0x1F44D], Rule::Literal, 0)];
        let blob = section(&entries, &["thumbs up"], &[(2, 1)]);
        let emoji = Emoji::parse(&blob).expect("the section parses");
        assert_eq!(emoji.emoji_name(&mut Names, ""), None);
        assert_eq!(emoji.emoji_name(&mut Names, "\u{FE0F}"), None);
        assert_eq!(emoji.emoji_name(&mut Names, "🏻👋"), None);
        assert_eq!(emoji.emoji_name(&mut Names, "\u{FE0F}👍"), None);
        assert_eq!(emoji.emoji_name(&mut Names, "d'être"), None);
        assert_eq!(emoji.emoji_name(&mut Names, "🐱‍🏍"), None);
        assert_eq!(
            emoji.emoji_name(&mut Names, "👍\u{FE0F}").as_deref(),
            Some("thumbs up")
        );
        assert!(!emoji.may_start(b'd'));
        assert!(emoji.may_start(b'\xF0'));
    }

    #[test]
    fn truncated_sections_are_rejected() {
        let entries = vec![(vec![0x1F44D], Rule::Literal, 0)];
        let blob = section(&entries, &["thumbs up"], &[(0, 0)]);
        assert!(Emoji::parse(&blob).is_some());
        for length in 0..blob.len() {
            assert!(Emoji::parse(&blob[..length]).is_none(), "length {length}");
        }
        assert!(Emoji::parse(&[]).is_none());
    }
}
