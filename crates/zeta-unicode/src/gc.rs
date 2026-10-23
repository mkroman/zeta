//! The Unicode general category property (UAX #44).

/// A Unicode general category value.
///
/// The long names are what replies print (`Lowercase Letter`), the short names (`Ll`) are what
/// the UCD files carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GeneralCategory {
    /// `Lu` — Uppercase Letter.
    UppercaseLetter,
    /// `Ll` — Lowercase Letter.
    LowercaseLetter,
    /// `Lt` — Titlecase Letter.
    TitlecaseLetter,
    /// `Lm` — Modifier Letter.
    ModifierLetter,
    /// `Lo` — Other Letter.
    OtherLetter,
    /// `Mn` — Nonspacing Mark.
    NonspacingMark,
    /// `Mc` — Spacing Mark.
    SpacingMark,
    /// `Me` — Enclosing Mark.
    EnclosingMark,
    /// `Nd` — Decimal Number.
    DecimalNumber,
    /// `Nl` — Letter Number.
    LetterNumber,
    /// `No` — Other Number.
    OtherNumber,
    /// `Pc` — Connector Punctuation.
    ConnectorPunctuation,
    /// `Pd` — Dash Punctuation.
    DashPunctuation,
    /// `Ps` — Open Punctuation.
    OpenPunctuation,
    /// `Pe` — Close Punctuation.
    ClosePunctuation,
    /// `Pi` — Initial Punctuation.
    InitialPunctuation,
    /// `Pf` — Final Punctuation.
    FinalPunctuation,
    /// `Po` — Other Punctuation.
    OtherPunctuation,
    /// `Sm` — Math Symbol.
    MathSymbol,
    /// `Sc` — Currency Symbol.
    CurrencySymbol,
    /// `Sk` — Modifier Symbol.
    ModifierSymbol,
    /// `So` — Other Symbol.
    OtherSymbol,
    /// `Zs` — Space Separator.
    SpaceSeparator,
    /// `Zl` — Line Separator.
    LineSeparator,
    /// `Zp` — Paragraph Separator.
    ParagraphSeparator,
    /// `Cc` — Control.
    Control,
    /// `Cf` — Format.
    Format,
    /// `Cs` — Surrogate (never reachable through `char`).
    Surrogate,
    /// `Co` — Private Use.
    PrivateUse,
    /// `Cn` — Unassigned.
    Unassigned,
}

/// One row of [`CATEGORIES`]: a variant and both of its UCD spellings.
struct CategoryNames {
    category: GeneralCategory,
    short: &'static str,
    long: &'static str,
}

/// Every general category, in generated table-id order (id is the index plus one).
///
/// This is the single source of truth for the category names and their order. It mirrors
/// `zeta_unicode_format::GC_SHORT_NAMES`, which the build crate uses to write the ids; the
/// `table_order_matches_the_generated_short_names` test pins the two together.
const CATEGORIES: [CategoryNames; 30] = [
    CategoryNames {
        category: GeneralCategory::Control,
        short: "Cc",
        long: "Control",
    },
    CategoryNames {
        category: GeneralCategory::Format,
        short: "Cf",
        long: "Format",
    },
    CategoryNames {
        category: GeneralCategory::Unassigned,
        short: "Cn",
        long: "Unassigned",
    },
    CategoryNames {
        category: GeneralCategory::PrivateUse,
        short: "Co",
        long: "Private Use",
    },
    CategoryNames {
        category: GeneralCategory::Surrogate,
        short: "Cs",
        long: "Surrogate",
    },
    CategoryNames {
        category: GeneralCategory::LowercaseLetter,
        short: "Ll",
        long: "Lowercase Letter",
    },
    CategoryNames {
        category: GeneralCategory::ModifierLetter,
        short: "Lm",
        long: "Modifier Letter",
    },
    CategoryNames {
        category: GeneralCategory::OtherLetter,
        short: "Lo",
        long: "Other Letter",
    },
    CategoryNames {
        category: GeneralCategory::TitlecaseLetter,
        short: "Lt",
        long: "Titlecase Letter",
    },
    CategoryNames {
        category: GeneralCategory::UppercaseLetter,
        short: "Lu",
        long: "Uppercase Letter",
    },
    CategoryNames {
        category: GeneralCategory::SpacingMark,
        short: "Mc",
        long: "Spacing Mark",
    },
    CategoryNames {
        category: GeneralCategory::EnclosingMark,
        short: "Me",
        long: "Enclosing Mark",
    },
    CategoryNames {
        category: GeneralCategory::NonspacingMark,
        short: "Mn",
        long: "Nonspacing Mark",
    },
    CategoryNames {
        category: GeneralCategory::DecimalNumber,
        short: "Nd",
        long: "Decimal Number",
    },
    CategoryNames {
        category: GeneralCategory::LetterNumber,
        short: "Nl",
        long: "Letter Number",
    },
    CategoryNames {
        category: GeneralCategory::OtherNumber,
        short: "No",
        long: "Other Number",
    },
    CategoryNames {
        category: GeneralCategory::ConnectorPunctuation,
        short: "Pc",
        long: "Connector Punctuation",
    },
    CategoryNames {
        category: GeneralCategory::DashPunctuation,
        short: "Pd",
        long: "Dash Punctuation",
    },
    CategoryNames {
        category: GeneralCategory::ClosePunctuation,
        short: "Pe",
        long: "Close Punctuation",
    },
    CategoryNames {
        category: GeneralCategory::FinalPunctuation,
        short: "Pf",
        long: "Final Punctuation",
    },
    CategoryNames {
        category: GeneralCategory::InitialPunctuation,
        short: "Pi",
        long: "Initial Punctuation",
    },
    CategoryNames {
        category: GeneralCategory::OtherPunctuation,
        short: "Po",
        long: "Other Punctuation",
    },
    CategoryNames {
        category: GeneralCategory::OpenPunctuation,
        short: "Ps",
        long: "Open Punctuation",
    },
    CategoryNames {
        category: GeneralCategory::CurrencySymbol,
        short: "Sc",
        long: "Currency Symbol",
    },
    CategoryNames {
        category: GeneralCategory::ModifierSymbol,
        short: "Sk",
        long: "Modifier Symbol",
    },
    CategoryNames {
        category: GeneralCategory::MathSymbol,
        short: "Sm",
        long: "Math Symbol",
    },
    CategoryNames {
        category: GeneralCategory::OtherSymbol,
        short: "So",
        long: "Other Symbol",
    },
    CategoryNames {
        category: GeneralCategory::LineSeparator,
        short: "Zl",
        long: "Line Separator",
    },
    CategoryNames {
        category: GeneralCategory::ParagraphSeparator,
        short: "Zp",
        long: "Paragraph Separator",
    },
    CategoryNames {
        category: GeneralCategory::SpaceSeparator,
        short: "Zs",
        long: "Space Separator",
    },
];

/// Maps a variant's declaration-order discriminant to its row in [`CATEGORIES`].
///
/// The mapping is built at compile time from [`CATEGORIES`], so a row can never point at the
/// wrong variant; `every_variant_round_trips_through_names_and_ids` also checks the reverse
/// direction.
const ROW_OF: [usize; 30] = build_row_of();

/// Builds [`ROW_OF`] from the single category table.
#[allow(
    clippy::cast_possible_truncation,
    reason = "both loop bounds are the 30-entry table size"
)]
const fn build_row_of() -> [usize; 30] {
    let mut rows = [0usize; CATEGORIES.len()];
    let mut variant = 0usize;
    while variant < CATEGORIES.len() {
        let mut row = 0usize;
        while row < CATEGORIES.len() {
            if CATEGORIES[row].category as u8 == variant as u8 {
                rows[variant] = row;
                break;
            }
            row += 1;
        }
        variant += 1;
    }
    rows
}

impl GeneralCategory {
    /// The position of this category in [`CATEGORIES`].
    const fn row_index(self) -> usize {
        ROW_OF[self as usize]
    }

    /// Maps a generated table id to its general category.
    ///
    /// The id is the position in `zeta_unicode_format::GC_SHORT_NAMES` plus one, which is what
    /// the general-category section stores. Returns `None` for `0` and for anything past the
    /// end of the table.
    #[must_use]
    pub const fn from_id(id: usize) -> Option<Self> {
        let Some(index) = id.checked_sub(1) else {
            return None;
        };
        if index < CATEGORIES.len() {
            Some(CATEGORIES[index].category)
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
        while index < CATEGORIES.len() {
            let bytes = CATEGORIES[index].short.as_bytes();
            if bytes[0] == short[0] && bytes[1] == short[1] {
                return Some(CATEGORIES[index].category);
            }
            index += 1;
        }
        None
    }

    /// The two-letter UCD abbreviation of this general category, e.g. `Lu`.
    #[must_use]
    pub const fn short_name(self) -> &'static str {
        CATEGORIES[self.row_index()].short
    }

    /// The long UCD name of this general category, e.g. `Uppercase Letter`.
    #[must_use]
    pub const fn long_name(self) -> &'static str {
        CATEGORIES[self.row_index()].long
    }
}

impl std::fmt::Display for GeneralCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.long_name())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use zeta_unicode_format::GC_SHORT_NAMES;

    use super::{CATEGORIES, GeneralCategory};

    #[test]
    fn every_variant_round_trips_through_names_and_ids() {
        let mut seen = HashSet::new();

        for (index, row) in CATEGORIES.iter().enumerate() {
            let id = index + 1;
            let short: [u8; 2] = row
                .short
                .as_bytes()
                .try_into()
                .expect("a two-letter short name");

            assert_eq!(GeneralCategory::from_id(id), Some(row.category));
            assert_eq!(GeneralCategory::from_short_name(&short), Some(row.category));
            assert_eq!(row.category.short_name(), row.short);
            assert_eq!(row.category.long_name(), row.long);
            assert_eq!(row.category.row_index(), index);
            assert_eq!(row.category.to_string(), row.long);
            assert!(seen.insert(row.category), "duplicate variant in the table");
        }

        assert_eq!(seen.len(), CATEGORIES.len());
    }

    #[test]
    fn table_order_matches_the_generated_short_names() {
        assert_eq!(CATEGORIES.len(), GC_SHORT_NAMES.len());

        for (index, short) in GC_SHORT_NAMES.iter().enumerate() {
            let id = index + 1;
            let category = GeneralCategory::from_id(id).expect("every table id resolves");
            let bytes: [u8; 2] = short
                .as_bytes()
                .try_into()
                .expect("a two-letter short name");

            assert_eq!(category.short_name(), *short, "short name at table id {id}");
            assert_eq!(
                GeneralCategory::from_short_name(&bytes),
                Some(category),
                "table id {id} does not resolve from its short name"
            );
        }
    }

    #[test]
    fn unknown_ids_and_short_names_are_rejected() {
        assert_eq!(GeneralCategory::from_id(0), None);
        assert_eq!(GeneralCategory::from_id(CATEGORIES.len() + 1), None);
        assert_eq!(GeneralCategory::from_id(usize::MAX), None);
        assert_eq!(GeneralCategory::from_short_name(b"Xx"), None);
        assert_eq!(GeneralCategory::from_short_name(b"lu"), None);
        assert_eq!(GeneralCategory::from_short_name(b"L "), None);
    }
}
