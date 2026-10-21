use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};

/// The settings type for plugins that have no configuration.
///
/// Used as the settings type of plugins that do not read any values from their
/// `[plugins.<name>]` configuration section. Only the host-managed `enabled` key is read; any
/// other key is ignored, but reported as a warning by the host — likely a typo, or a setting
/// that no longer exists.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct NoSettings;

// The host deserializes each plugin's `[plugins.<name>]` section — a map, empty when the section
// is — into its settings type. A derived `Deserialize` for a unit struct only accepts unit
// input, so the section is read through an empty, every-key-ignored struct instead; unknown keys
// keep being reported through `serde_ignored`, exactly as for a settings struct.
impl<'de> Deserialize<'de> for NoSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// An empty settings section: every key in it is unknown and ignored.
        #[derive(Deserialize)]
        struct Ignored {}

        Ignored::deserialize(deserializer)?;

        Ok(Self)
    }
}

/// The name of a plugin.
///
/// The name identifies the plugin in the help system and the plugin catalog, and is the plugin's
/// module name — the same value as [`NamedPlugin::NAME`](crate::NamedPlugin::NAME).
///
/// Cheap to clone: the name is shared behind an [`Arc`].
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PluginName(Arc<str>);

impl PluginName {
    /// Creates a plugin name.
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Self(name.into())
    }

    /// Returns the name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for PluginName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for PluginName {
    fn from(name: &str) -> Self {
        Self(name.into())
    }
}

impl From<String> for PluginName {
    fn from(name: String) -> Self {
        Self(name.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_round_trips_through_its_conversions() {
        let name = PluginName::new("dig");

        assert_eq!(name.as_str(), "dig");
        assert_eq!(name.as_ref(), "dig");
        assert_eq!(name.to_string(), "dig");
        assert_eq!(name, PluginName::from("dig"));
        assert_eq!(name, PluginName::from("dig".to_string()));
    }

    #[test]
    fn names_are_ordered_and_hashable() {
        let first = PluginName::from("alert");
        let second = PluginName::from("dig");

        assert!(first < second);
        assert_eq!(PluginName::from("dig"), second);
    }
}
