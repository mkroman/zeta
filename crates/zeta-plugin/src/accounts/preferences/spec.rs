//! The descriptors of a plugin's per-user preferences.
//!
//! A descriptor is plain data: a plugin declares its preferences through a [`PreferenceSet`]
//! implementation, names it in its `#[plugin]` attribute, and returns the descriptors from
//! [`Registered::preferences`](crate::Registered::preferences). The runtime folds the loaded
//! plugins' descriptors into their [`CatalogEntry`](crate::CatalogEntry), so the `.set`
//! command offers exactly the preferences of the plugins that are actually loaded. Descriptors
//! carry no values and no secrets — only what the `.set` command needs to list, describe, and
//! validate a preference, and what [`Preferences`](super::Preferences) needs to key and
//! canonicalize the values.

/// The kind of values a preference accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreferenceKind {
    /// One of a fixed set of values.
    Enum(&'static [&'static str]),
}

impl PreferenceKind {
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

/// One per-user preference a plugin offers.
///
/// The `plugin` field must spell the name of the plugin that declares the preference; the
/// runtime warns when the two disagree, since the mismatch would put the preference under a
/// name no plugin resolves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreferenceSpec {
    /// The plugin offering the preference, as its bundled name.
    pub plugin: &'static str,
    /// The preference's key within the plugin, e.g. `language`.
    pub key: &'static str,
    /// The kind of values the preference accepts.
    pub kind: PreferenceKind,
    /// One-sentence description of what the preference does.
    pub description: &'static str,
    /// The value used when the user has not set one, or [`None`] when the plugin's behavior
    /// with the preference unset is not a stored value (e.g. an omitted API parameter).
    pub default: Option<&'static str>,
}

impl PreferenceSpec {
    /// The `plugin.key` identifier users address the preference by.
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}.{}", self.plugin, self.key)
    }

    /// Whether `value` is accepted for the preference.
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

/// A type that declares the preferences one plugin offers.
///
/// Implemented by the derive macro a plugin uses for its preference values, and named in the
/// `#[plugin]` attribute; the generated [`Registered::preferences`](crate::Registered::preferences)
/// forwards the result to the runtime, which folds the descriptors into the plugin's
/// [`CatalogEntry`](crate::CatalogEntry).
pub trait PreferenceSet {
    /// The descriptors of the plugin's preferences, in the order the `.set` command lists them.
    ///
    /// `plugin` is the name of the plugin declaring them; every returned descriptor spells it in
    /// its `plugin` field, so the runtime can tell a mismatch from a descriptor that belongs to
    /// another plugin.
    #[must_use]
    fn preferences(plugin: &'static str) -> Vec<PreferenceSpec>;
}

/// One preference value with a fixed, canonical set of accepted spellings.
///
/// Implemented by the derive macro a plugin puts on its preference enum: [`VALUES`](Self::VALUES)
/// lists every accepted spelling, [`canonical`](Self::canonical) parses one case-insensitively,
/// and [`as_str`](Self::as_str) renders the canonical spelling back. Kept as a trait rather than
/// plain strings so a consumer names its preference values instead of matching string literals.
pub trait PreferenceValue: Copy + 'static {
    /// Every canonical value, in the order the descriptors list them.
    const VALUES: &'static [&'static str];

    /// The value `value` spells, matched case-insensitively.
    ///
    /// Returns [`None`] when `value` is not one of [`VALUES`](Self::VALUES).
    fn canonical(value: &str) -> Option<Self>;

    /// The canonical spelling of the value.
    fn as_str(self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    static ACCENT: PreferenceSpec = PreferenceSpec {
        plugin: "spec-test",
        key: "accent",
        kind: PreferenceKind::Enum(&["DA", "DE"]),
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
    fn id_addresses_the_preference_by_plugin_and_key() {
        assert_eq!(ACCENT.id(), "spec-test.accent");
    }
}
