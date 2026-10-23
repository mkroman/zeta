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

impl GeneralCategory {
    /// Maps a two-letter UCD abbreviation such as `Lu` to its general category.
    ///
    /// Returns `None` for anything that is not one of the thirty defined values.
    #[must_use]
    pub const fn from_short_name(short: &[u8; 2]) -> Option<Self> {
        let category = match *short {
            [b'L', b'u'] => Self::UppercaseLetter,
            [b'L', b'l'] => Self::LowercaseLetter,
            [b'L', b't'] => Self::TitlecaseLetter,
            [b'L', b'm'] => Self::ModifierLetter,
            [b'L', b'o'] => Self::OtherLetter,
            [b'M', b'n'] => Self::NonspacingMark,
            [b'M', b'c'] => Self::SpacingMark,
            [b'M', b'e'] => Self::EnclosingMark,
            [b'N', b'd'] => Self::DecimalNumber,
            [b'N', b'l'] => Self::LetterNumber,
            [b'N', b'o'] => Self::OtherNumber,
            [b'P', b'c'] => Self::ConnectorPunctuation,
            [b'P', b'd'] => Self::DashPunctuation,
            [b'P', b's'] => Self::OpenPunctuation,
            [b'P', b'e'] => Self::ClosePunctuation,
            [b'P', b'i'] => Self::InitialPunctuation,
            [b'P', b'f'] => Self::FinalPunctuation,
            [b'P', b'o'] => Self::OtherPunctuation,
            [b'S', b'm'] => Self::MathSymbol,
            [b'S', b'c'] => Self::CurrencySymbol,
            [b'S', b'k'] => Self::ModifierSymbol,
            [b'S', b'o'] => Self::OtherSymbol,
            [b'Z', b's'] => Self::SpaceSeparator,
            [b'Z', b'l'] => Self::LineSeparator,
            [b'Z', b'p'] => Self::ParagraphSeparator,
            [b'C', b'c'] => Self::Control,
            [b'C', b'f'] => Self::Format,
            [b'C', b's'] => Self::Surrogate,
            [b'C', b'o'] => Self::PrivateUse,
            [b'C', b'n'] => Self::Unassigned,
            _ => return None,
        };
        Some(category)
    }

    /// The two-letter UCD abbreviation of this general category, e.g. `Lu`.
    #[must_use]
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::UppercaseLetter => "Lu",
            Self::LowercaseLetter => "Ll",
            Self::TitlecaseLetter => "Lt",
            Self::ModifierLetter => "Lm",
            Self::OtherLetter => "Lo",
            Self::NonspacingMark => "Mn",
            Self::SpacingMark => "Mc",
            Self::EnclosingMark => "Me",
            Self::DecimalNumber => "Nd",
            Self::LetterNumber => "Nl",
            Self::OtherNumber => "No",
            Self::ConnectorPunctuation => "Pc",
            Self::DashPunctuation => "Pd",
            Self::OpenPunctuation => "Ps",
            Self::ClosePunctuation => "Pe",
            Self::InitialPunctuation => "Pi",
            Self::FinalPunctuation => "Pf",
            Self::OtherPunctuation => "Po",
            Self::MathSymbol => "Sm",
            Self::CurrencySymbol => "Sc",
            Self::ModifierSymbol => "Sk",
            Self::OtherSymbol => "So",
            Self::SpaceSeparator => "Zs",
            Self::LineSeparator => "Zl",
            Self::ParagraphSeparator => "Zp",
            Self::Control => "Cc",
            Self::Format => "Cf",
            Self::Surrogate => "Cs",
            Self::PrivateUse => "Co",
            Self::Unassigned => "Cn",
        }
    }

    /// The long UCD name of this general category, e.g. `Uppercase Letter`.
    #[must_use]
    pub const fn long_name(self) -> &'static str {
        match self {
            Self::UppercaseLetter => "Uppercase Letter",
            Self::LowercaseLetter => "Lowercase Letter",
            Self::TitlecaseLetter => "Titlecase Letter",
            Self::ModifierLetter => "Modifier Letter",
            Self::OtherLetter => "Other Letter",
            Self::NonspacingMark => "Nonspacing Mark",
            Self::SpacingMark => "Spacing Mark",
            Self::EnclosingMark => "Enclosing Mark",
            Self::DecimalNumber => "Decimal Number",
            Self::LetterNumber => "Letter Number",
            Self::OtherNumber => "Other Number",
            Self::ConnectorPunctuation => "Connector Punctuation",
            Self::DashPunctuation => "Dash Punctuation",
            Self::OpenPunctuation => "Open Punctuation",
            Self::ClosePunctuation => "Close Punctuation",
            Self::InitialPunctuation => "Initial Punctuation",
            Self::FinalPunctuation => "Final Punctuation",
            Self::OtherPunctuation => "Other Punctuation",
            Self::MathSymbol => "Math Symbol",
            Self::CurrencySymbol => "Currency Symbol",
            Self::ModifierSymbol => "Modifier Symbol",
            Self::OtherSymbol => "Other Symbol",
            Self::SpaceSeparator => "Space Separator",
            Self::LineSeparator => "Line Separator",
            Self::ParagraphSeparator => "Paragraph Separator",
            Self::Control => "Control",
            Self::Format => "Format",
            Self::Surrogate => "Surrogate",
            Self::PrivateUse => "Private Use",
            Self::Unassigned => "Unassigned",
        }
    }
}

impl std::fmt::Display for GeneralCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.long_name())
    }
}
