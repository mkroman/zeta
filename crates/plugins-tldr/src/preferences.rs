//! The plugin's per-user preferences: the `.tldr` options a user can store once.
//!
//! The command's flags override them for a single invocation; a user who sets a preference
//! gets it on every later `.tldr` without the flag. The values are the same enums the command
//! parses, so the flags and `.set` can never disagree about what a value is. Each field of the
//! set is one preference: its doc comment is the description `.set` lists, and its type
//! supplies the accepted values.

use clap::ValueEnum;

use zeta_plugin::{PreferenceSet, PreferenceValue};

/// The `.tldr` preferences a user can store, changed with the `.set` command.
///
/// Each field is one preference. A non-`Option` field's default applies when the user has no
/// stored value, or resets it; an `Option` field has no stored default, and leaving it unset
/// keeps the endpoint's own behavior.
#[derive(Debug, Default, PreferenceSet)]
pub struct Preferences {
    /// the style of the .tldr summary: prose or key moments
    summary_type: SummaryType,
    /// the output language of the .tldr summary
    language: Option<Language>,
    /// whether .tldr sends one truncated line or the full summary
    delivery: Delivery,
}

/// The summary styles the extension offers.
///
/// The lowercase canonical spelling is the `summary_type` parameter value sent to the endpoint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum, PreferenceValue)]
#[preference(rename_all = "lower")]
pub enum SummaryType {
    /// the page summarized as flowing prose
    #[default]
    Summary,
    /// the page distilled into key moments
    Takeaway,
}

/// How much of the summary the reply carries.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum, PreferenceValue)]
#[preference(rename_all = "lower")]
pub enum Delivery {
    /// one budgeted line, cut with an ellipsis
    #[default]
    Truncated,
    /// the whole summary, wrapped over several lines
    Full,
}

/// The output languages the extension offers, by their codes.
///
/// The uppercase canonical spelling is the `target_language` parameter value sent to the
/// endpoint. The type has no default: an unset language leaves the endpoint's own choice, so
/// the preference is an `Option<Language>` and an unset stays `None`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum, PreferenceValue)]
#[preference(rename_all = "UPPER")]
pub enum Language {
    /// document language
    Doc,
    /// Bulgarian
    Bg,
    /// Czech
    Cs,
    /// Danish
    Da,
    /// German
    De,
    /// Greek
    El,
    /// English
    En,
    /// Spanish
    Es,
    /// Estonian
    Et,
    /// Finnish
    Fi,
    /// French
    Fr,
    /// Hungarian
    Hu,
    /// Indonesian
    Id,
    /// Italian
    It,
    /// Japanese
    Ja,
    /// Korean
    Ko,
    /// Lithuanian
    Lt,
    /// Latvian
    Lv,
    /// Norwegian (bokmål)
    Nb,
    /// Dutch
    Nl,
    /// Polish
    Pl,
    /// Portuguese
    Pt,
    /// Romanian
    Ro,
    /// Russian
    Ru,
    /// Slovak
    Sk,
    /// Slovenian
    Sl,
    /// Swedish
    Sv,
    /// Turkish
    Tr,
    /// Ukrainian
    Uk,
    /// Chinese (simplified)
    Zh,
    /// Chinese (traditional)
    #[preference(name = "ZH-HANT")]
    #[value(name = "ZH-HANT")]
    ZhHant,
}
