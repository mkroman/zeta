//! The Unicode general category property (UAX #44).
//!
//! The rows below are the single source of the category variants, their generated table ids
//! and both name tables. The trainer writes those ids into the general-category section and
//! the runtime maps them back, so both sides use this one declaration.

/// Declares the general-category table.
///
/// Each row carries the variant name, its two-letter UCD abbreviation ("short name") and its
/// long UCD name, in generated table-id order. The enum, [`ALL`] and [`GC_LONG_NAMES`] all come
/// from those rows, so they cannot drift apart. An optional fourth element appends a note to
/// the variant's doc comment, before the final period.
macro_rules! general_categories {
    ($(($variant:ident, $short:literal, $long:literal $(, $note:literal)?)),+ $(,)?) => {
        /// A Unicode general category value.
        ///
        /// The long names are what replies print (`Lowercase Letter`), the short names (`Ll`) are what
        /// the UCD files carry. The declaration order is the generated table-id order, so a variant's
        /// discriminant is also its index in [`GC_SHORT_NAMES`] and in [`GC_LONG_NAMES`].
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum GeneralCategory {
            $(
                #[doc = concat!("`", $short, "` — ", $long, general_categories!(@note $($note)?), ".")]
                $variant,
            )+
        }

        /// Every general category, in generated table-id order (id is the index plus one).
        pub const ALL: [GeneralCategory; general_categories!(@count $($variant),+)] = [
            $(GeneralCategory::$variant,)+
        ];

        /// The two-letter UCD abbreviations, in the same order as [`ALL`].
        ///
        /// The generated table ids are positions in this array plus one.
        pub const GC_SHORT_NAMES: [&str; general_categories!(@count $($variant),+)] = [
            $($short,)+
        ];

        /// The long UCD names, in the same order as [`ALL`].
        pub const GC_LONG_NAMES: [&str; general_categories!(@count $($variant),+)] = [
            $($long,)+
        ];

        impl GeneralCategory {
            /// Maps a generated table id to its general category.
            ///
            /// The id is the position in [`GC_SHORT_NAMES`] plus one, which is what the
            /// general-category section stores. Returns `None` for `0` and for anything past
            /// the end of the table.
            #[must_use]
            pub const fn from_id(id: usize) -> Option<Self> {
                let Some(index) = id.checked_sub(1) else {
                    return None;
                };
                if index < ALL.len() {
                    Some(ALL[index])
                } else {
                    None
                }
            }

            /// Maps a two-letter UCD abbreviation such as `Lu` to its general category.
            ///
            /// Returns `None` for anything that is not one of the thirty defined values.
            #[must_use]
            pub const fn from_short_name(short: &[u8; 2]) -> Option<Self> {
                let mut index = 0;
                while index < GC_SHORT_NAMES.len() {
                    let bytes = GC_SHORT_NAMES[index].as_bytes();
                    if bytes[0] == short[0] && bytes[1] == short[1] {
                        return Some(ALL[index]);
                    }
                    index += 1;
                }
                None
            }

            /// The two-letter UCD abbreviation of this general category, e.g. `Lu`.
            #[must_use]
            pub const fn short_name(self) -> &'static str {
                GC_SHORT_NAMES[self as usize]
            }

            /// The long UCD name of this general category, e.g. `Uppercase Letter`.
            #[must_use]
            pub const fn long_name(self) -> &'static str {
                GC_LONG_NAMES[self as usize]
            }
        }

        impl core::fmt::Display for GeneralCategory {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str(self.long_name())
            }
        }
    };

    (@count $($variant:ident),+) => {
        <[()]>::len(&[$(general_categories!(@unit $variant)),+])
    };
    (@unit $variant:ident) => { () };
    (@note) => { "" };
    (@note $note:literal) => { $note };
}

general_categories! {
    (Control, "Cc", "Control"),
    (Format, "Cf", "Format"),
    (Unassigned, "Cn", "Unassigned"),
    (PrivateUse, "Co", "Private Use"),
    (Surrogate, "Cs", "Surrogate", " (never reachable through `char`)"),
    (LowercaseLetter, "Ll", "Lowercase Letter"),
    (ModifierLetter, "Lm", "Modifier Letter"),
    (OtherLetter, "Lo", "Other Letter"),
    (TitlecaseLetter, "Lt", "Titlecase Letter"),
    (UppercaseLetter, "Lu", "Uppercase Letter"),
    (SpacingMark, "Mc", "Spacing Mark"),
    (EnclosingMark, "Me", "Enclosing Mark"),
    (NonspacingMark, "Mn", "Nonspacing Mark"),
    (DecimalNumber, "Nd", "Decimal Number"),
    (LetterNumber, "Nl", "Letter Number"),
    (OtherNumber, "No", "Other Number"),
    (ConnectorPunctuation, "Pc", "Connector Punctuation"),
    (DashPunctuation, "Pd", "Dash Punctuation"),
    (ClosePunctuation, "Pe", "Close Punctuation"),
    (FinalPunctuation, "Pf", "Final Punctuation"),
    (InitialPunctuation, "Pi", "Initial Punctuation"),
    (OtherPunctuation, "Po", "Other Punctuation"),
    (OpenPunctuation, "Ps", "Open Punctuation"),
    (CurrencySymbol, "Sc", "Currency Symbol"),
    (ModifierSymbol, "Sk", "Modifier Symbol"),
    (MathSymbol, "Sm", "Math Symbol"),
    (OtherSymbol, "So", "Other Symbol"),
    (LineSeparator, "Zl", "Line Separator"),
    (ParagraphSeparator, "Zp", "Paragraph Separator"),
    (SpaceSeparator, "Zs", "Space Separator"),
}

// A category id is packed into `GC_ID_BITS` low bits of a general-category record. The table
// itself supplies the ids, so this is the only bound the record encoding needs.
const _: () = assert!(GC_SHORT_NAMES.len() < 1usize << crate::GC_ID_BITS);

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{ALL, GC_LONG_NAMES, GC_SHORT_NAMES, GeneralCategory};

    #[test]
    fn every_variant_round_trips_through_names_and_ids() {
        let mut seen = HashSet::new();

        for (index, &category) in ALL.iter().enumerate() {
            let id = index + 1;
            let short = GC_SHORT_NAMES[index];
            let bytes: [u8; 2] = short
                .as_bytes()
                .try_into()
                .expect("a two-letter short name");

            assert_eq!(GeneralCategory::from_id(id), Some(category));
            assert_eq!(GeneralCategory::from_short_name(&bytes), Some(category));
            assert_eq!(category.short_name(), short);
            assert_eq!(category.long_name(), GC_LONG_NAMES[index]);
            assert_eq!(category.to_string(), GC_LONG_NAMES[index]);
            assert!(seen.insert(category), "duplicate variant in the table");
        }

        assert_eq!(seen.len(), ALL.len());
    }

    #[test]
    fn unknown_ids_and_short_names_are_rejected() {
        assert_eq!(GeneralCategory::from_id(0), None);
        assert_eq!(GeneralCategory::from_id(ALL.len() + 1), None);
        assert_eq!(GeneralCategory::from_id(usize::MAX), None);
        assert_eq!(GeneralCategory::from_short_name(b"Xx"), None);
        assert_eq!(GeneralCategory::from_short_name(b"lu"), None);
        assert_eq!(GeneralCategory::from_short_name(b"L "), None);
    }
}
