//! The Unicode general category property (UAX #44).

use zeta_unicode_format::{GC_ID_BITS, GC_ID_MASK, GC_PAGES, GC_SHIFT, GC_SHORT_NAMES};

use crate::{sections, tables};

/// A Unicode general category value.
///
/// The long names are what replies print (`Lowercase Letter`), the short names (`Ll`) are what
/// the UCD files carry. The variants are declared in generated table-id order, so a variant's
/// discriminant is also its index in `zeta_unicode_format::GC_SHORT_NAMES` and in the crate's
/// long-name table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GeneralCategory {
    /// `Cc` — Control.
    Control,
    /// `Cf` — Format.
    Format,
    /// `Cn` — Unassigned.
    Unassigned,
    /// `Co` — Private Use.
    PrivateUse,
    /// `Cs` — Surrogate (never reachable through `char`).
    Surrogate,
    /// `Ll` — Lowercase Letter.
    LowercaseLetter,
    /// `Lm` — Modifier Letter.
    ModifierLetter,
    /// `Lo` — Other Letter.
    OtherLetter,
    /// `Lt` — Titlecase Letter.
    TitlecaseLetter,
    /// `Lu` — Uppercase Letter.
    UppercaseLetter,
    /// `Mc` — Spacing Mark.
    SpacingMark,
    /// `Me` — Enclosing Mark.
    EnclosingMark,
    /// `Mn` — Nonspacing Mark.
    NonspacingMark,
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
    /// `Pe` — Close Punctuation.
    ClosePunctuation,
    /// `Pf` — Final Punctuation.
    FinalPunctuation,
    /// `Pi` — Initial Punctuation.
    InitialPunctuation,
    /// `Po` — Other Punctuation.
    OtherPunctuation,
    /// `Ps` — Open Punctuation.
    OpenPunctuation,
    /// `Sc` — Currency Symbol.
    CurrencySymbol,
    /// `Sk` — Modifier Symbol.
    ModifierSymbol,
    /// `Sm` — Math Symbol.
    MathSymbol,
    /// `So` — Other Symbol.
    OtherSymbol,
    /// `Zl` — Line Separator.
    LineSeparator,
    /// `Zp` — Paragraph Separator.
    ParagraphSeparator,
    /// `Zs` — Space Separator.
    SpaceSeparator,
}

/// Every general category, in generated table-id order (id is the index plus one).
///
/// The declaration order above, this array and the two name arrays all match
/// `zeta_unicode_format::GC_SHORT_NAMES`, which the build crate uses to write the ids; the
/// `table_order_matches_the_generated_short_names` test pins the four together.
const ALL: [GeneralCategory; 30] = [
    GeneralCategory::Control,
    GeneralCategory::Format,
    GeneralCategory::Unassigned,
    GeneralCategory::PrivateUse,
    GeneralCategory::Surrogate,
    GeneralCategory::LowercaseLetter,
    GeneralCategory::ModifierLetter,
    GeneralCategory::OtherLetter,
    GeneralCategory::TitlecaseLetter,
    GeneralCategory::UppercaseLetter,
    GeneralCategory::SpacingMark,
    GeneralCategory::EnclosingMark,
    GeneralCategory::NonspacingMark,
    GeneralCategory::DecimalNumber,
    GeneralCategory::LetterNumber,
    GeneralCategory::OtherNumber,
    GeneralCategory::ConnectorPunctuation,
    GeneralCategory::DashPunctuation,
    GeneralCategory::ClosePunctuation,
    GeneralCategory::FinalPunctuation,
    GeneralCategory::InitialPunctuation,
    GeneralCategory::OtherPunctuation,
    GeneralCategory::OpenPunctuation,
    GeneralCategory::CurrencySymbol,
    GeneralCategory::ModifierSymbol,
    GeneralCategory::MathSymbol,
    GeneralCategory::OtherSymbol,
    GeneralCategory::LineSeparator,
    GeneralCategory::ParagraphSeparator,
    GeneralCategory::SpaceSeparator,
];

/// The long UCD names, in the same order as [`ALL`].
const LONG_NAMES: [&str; 30] = [
    "Control",
    "Format",
    "Unassigned",
    "Private Use",
    "Surrogate",
    "Lowercase Letter",
    "Modifier Letter",
    "Other Letter",
    "Titlecase Letter",
    "Uppercase Letter",
    "Spacing Mark",
    "Enclosing Mark",
    "Nonspacing Mark",
    "Decimal Number",
    "Letter Number",
    "Other Number",
    "Connector Punctuation",
    "Dash Punctuation",
    "Close Punctuation",
    "Final Punctuation",
    "Initial Punctuation",
    "Other Punctuation",
    "Open Punctuation",
    "Currency Symbol",
    "Modifier Symbol",
    "Math Symbol",
    "Other Symbol",
    "Line Separator",
    "Paragraph Separator",
    "Space Separator",
];

impl GeneralCategory {
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
        LONG_NAMES[self as usize]
    }
}

impl std::fmt::Display for GeneralCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.long_name())
    }
}

/// Looks up the general category of `cp` in the two-level page table.
///
/// The section is a page directory of `u16` record indices over packed `u16` records: the
/// page-relative run start in the high bits and the category id in the low
/// [`GC_ID_BITS`](zeta_unicode_format::GC_ID_BITS) bits.
#[allow(
    clippy::redundant_pub_crate,
    reason = "the module is private, but `pub(crate)` states the intended visibility"
)]
pub(crate) fn general_category_of(cp: u32) -> Option<GeneralCategory> {
    let table = tables::section(sections::GC)?;
    let directory = tables::bytes_at(table, 0, GC_PAGES * 2)?;
    let records = table.get(GC_PAGES * 2..).unwrap_or_default();

    let page = usize::try_from(cp >> GC_SHIFT).ok()?;
    let relative = cp & ((1 << GC_SHIFT) - 1);
    let (first, end) = tables::u16_pair_at(directory, page)?;

    // Find the last run in the page whose start is at or before `cp`. When no run starts in
    // this page (`first == end`) or `cp` falls before the page's first run, `low` stays at
    // `first` and the carried-over run ending just before it answers instead.
    let (mut low, mut high) = (usize::from(first), usize::from(end));
    while low < high {
        let mid = low + (high - low) / 2;
        let start = u32::from(tables::u16_at(records, mid.checked_mul(2)?)? >> GC_ID_BITS);
        if start <= relative {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    let record = low.checked_sub(1)?;

    let id = usize::from(tables::u16_at(records, record.checked_mul(2)?)? & GC_ID_MASK);
    GeneralCategory::from_id(id)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use zeta_unicode_format::GC_SHORT_NAMES;

    use super::{ALL, GeneralCategory, LONG_NAMES};

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
            assert_eq!(category.long_name(), LONG_NAMES[index]);
            assert_eq!(category.to_string(), LONG_NAMES[index]);
            assert!(seen.insert(category), "duplicate variant in the table");
        }

        assert_eq!(seen.len(), ALL.len());
    }

    #[test]
    fn table_order_matches_the_generated_short_names() {
        assert_eq!(ALL.len(), 30);
        assert_eq!(ALL.len(), GC_SHORT_NAMES.len());
        assert_eq!(LONG_NAMES.len(), ALL.len());

        for (index, short) in GC_SHORT_NAMES.iter().enumerate() {
            let id = index + 1;
            let category = GeneralCategory::from_id(id).expect("every table id resolves");
            let bytes: [u8; 2] = short
                .as_bytes()
                .try_into()
                .expect("a two-letter short name");

            assert_eq!(category as usize, index, "variant order at table id {id}");
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
        assert_eq!(GeneralCategory::from_id(ALL.len() + 1), None);
        assert_eq!(GeneralCategory::from_id(usize::MAX), None);
        assert_eq!(GeneralCategory::from_short_name(b"Xx"), None);
        assert_eq!(GeneralCategory::from_short_name(b"lu"), None);
        assert_eq!(GeneralCategory::from_short_name(b"L "), None);
    }
}
