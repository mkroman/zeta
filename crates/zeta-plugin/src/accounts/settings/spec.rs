//! The descriptors of a plugin's per-user settings.
//!
//! A descriptor is static data: a plugin owns a `&'static [UserSettingSpec]` slice, names it in
//! its `#[plugin(user_settings = …)]` attribute, and the runtime aggregates the loaded plugins'
//! slices into the [`UserSettingsCatalog`](super::UserSettingsCatalog) when registration
//! finishes. Descriptors carry no values and no secrets — only what the `.settings` command
//! needs to list, describe, and validate a setting, and what [`UserSettings`](super::UserSettings)
//! needs to key and canonicalize the values.

/// The kind of values a setting accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserSettingKind {
    /// One of a fixed set of values.
    Enum(&'static [&'static str]),
}

impl UserSettingKind {
    /// The canonical stored form of `value`, matched case-insensitively against the accepted
    /// values.
    ///
    /// Returns [`None`] when `value` is not one of the kind's values; the canonical form is the
    /// spelling from the descriptor, so stored values and listed values never drift apart.
    #[must_use]
    pub fn canonical(&self, value: &str) -> Option<&'static str> {
        match self {
            Self::Enum(values) => values
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(value))
                .copied(),
        }
    }

    /// Renders the accepted values for a reply, e.g. `summary, takeaway`.
    #[must_use]
    pub fn values(&self) -> String {
        match self {
            Self::Enum(values) => values.join(", "),
        }
    }
}

/// One per-user setting a plugin offers.
///
/// The `plugin` field must spell the name of the plugin that carries the slice; the runtime
/// warns when the two disagree, since the mismatch would put the setting under a name no
/// plugin resolves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserSettingSpec {
    /// The plugin offering the setting, as its bundled name.
    pub plugin: &'static str,
    /// The setting's key within the plugin, e.g. `language`.
    pub key: &'static str,
    /// The kind of values the setting accepts.
    pub kind: UserSettingKind,
    /// One-sentence description of what the setting does.
    pub description: &'static str,
    /// The value used when the user has not set one, or [`None`] when the plugin's behavior
    /// with the setting unset is not a stored value (e.g. an omitted API parameter).
    pub default: Option<&'static str>,
}

impl UserSettingSpec {
    /// The `plugin.key` identifier users address the setting by.
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}.{}", self.plugin, self.key)
    }

    /// Whether `value` is accepted for the setting.
    #[must_use]
    pub fn accepts(&self, value: &str) -> bool {
        self.kind.canonical(value).is_some()
    }

    /// The canonical stored form of `value`, matched case-insensitively.
    #[must_use]
    pub fn canonical(&self, value: &str) -> Option<&'static str> {
        self.kind.canonical(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static ACCENT: UserSettingSpec = UserSettingSpec {
        plugin: "spec-test",
        key: "accent",
        kind: UserSettingKind::Enum(&["DA", "DE"]),
        description: "the accent",
        default: Some("DA"),
    };

    #[test]
    fn canonical_matches_case_insensitively_and_returns_the_descriptor_spelling() {
        assert_eq!(ACCENT.canonical("de"), Some("DE"));
        assert_eq!(ACCENT.canonical("EN"), None);
        assert!(ACCENT.accepts("da"));
        assert!(!ACCENT.accepts("EN"));
        assert_eq!(ACCENT.kind.values(), "DA, DE");
    }

    #[test]
    fn id_addresses_the_setting_by_plugin_and_key() {
        assert_eq!(ACCENT.id(), "spec-test.accent");
    }
}
