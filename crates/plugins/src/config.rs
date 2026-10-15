//! Per-plugin configuration plumbing.
//!
//! One [`PluginConfig`] value exists per bundled plugin, holding the host-managed `enabled`
//! toggle next to the plugin's own typed settings.

use figment::value::{Dict, Value};
use serde::{Deserialize, Deserializer, Serialize};

/// An individual plugin's configuration: its `[plugins.<name>]` section.
///
/// One value exists per compiled plugin. Sections are type-checked at startup; malformed values
/// abort configuration loading with a diagnostic.
///
/// The `enabled` key is managed by the host and defaults to `true`; every other key belongs to
/// the plugin's settings type (e.g. `dig::Settings`), deserialized through
/// [`PluginConfig::settings`]. Unknown settings keys are ignored, collected in
/// [`PluginConfig::unknown_keys`] and warned about by the host: a config that names a setting
/// removed by a newer release must keep working, so rejection is too harsh.
#[derive(Clone, Debug, Serialize)]
pub struct PluginConfig<S> {
    /// Enable the plugin.
    pub enabled: bool,
    /// The plugin's typed settings.
    ///
    /// Flattened so serialization round-trips with the custom [`Deserialize`] implementation
    /// below, which reads settings keys at the section level.
    #[serde(flatten)]
    pub settings: S,
    /// Keys in the plugin's configuration section that the settings type did not recognize.
    ///
    /// Populated during deserialization — a typo'd key, or a setting that no longer exists.
    /// The host logs each one as a warning. Skipped by serialization, so round-trips only
    /// carry `enabled` and the settings themselves.
    #[serde(skip)]
    pub unknown_keys: Vec<String>,
}

impl<'de, S> Deserialize<'de> for PluginConfig<S>
where
    S: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error as _;

        // A custom implementation instead of `#[serde(flatten)]`, which would silently swallow
        // unknown keys: `serde_ignored` reports every key the settings type ignores so the
        // host can warn about a typo, or a setting removed by this version.
        let mut section = Dict::deserialize(deserializer)?;
        let enabled = match section.remove("enabled") {
            Some(value) => bool::deserialize(&value)
                .map_err(|error| D::Error::custom(format!("invalid `enabled` key: {error}")))?,
            None => true,
        };

        let mut unknown_keys = Vec::new();
        let value = Value::from(section);
        let settings = serde_ignored::deserialize(&value, |path| {
            unknown_keys.push(path.to_string());
        })
        .map_err(|error| D::Error::custom(format!("invalid settings: {error}")))?;

        Ok(Self {
            enabled,
            settings,
            unknown_keys,
        })
    }
}

impl<S: Default> Default for PluginConfig<S> {
    fn default() -> Self {
        Self {
            enabled: true,
            settings: S::default(),
            unknown_keys: Vec::new(),
        }
    }
}

#[cfg(all(test, feature = "dig", feature = "health"))]
mod tests {
    use std::net::IpAddr;

    use figment::{
        Error, Figment,
        providers::{Format, Toml},
    };
    use zeta_test_support::assert_empty;

    use super::*;
    use crate::PluginsConfig;

    /// The base configuration the full-config tests start from.
    const FULL_BASE: &str = r#"
[database]
url = "postgresql://localhost/zeta_test"

[tracing]
enabled = true

[irc]
nickname = "zeta"
hostname = "localhost"
alt_nicks = []
channels = []
"#;

    /// Extracts the `[plugins]` subtree from an inline TOML document.
    fn extract(toml: &str) -> Result<PluginsConfig, Box<Error>> {
        Figment::new()
            .merge(Toml::string(toml))
            .focus("plugins")
            .extract()
            .map_err(Box::new)
    }

    #[test]
    fn parses_plugin_sections() {
        let plugins = extract(
            r#"
[plugins.dig]
enabled = true
nameservers = ["1.1.1.1", "2606:4700:4700::1111"]

[plugins.health]
enabled = false
"#,
        )
        .expect("could not parse configuration");

        assert!(plugins.dig.enabled);
        assert_eq!(
            plugins.dig.settings.nameservers,
            vec![
                IpAddr::from([1, 1, 1, 1]),
                IpAddr::from([0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111]),
            ]
        );
        assert!(!plugins.health.enabled);
        assert_empty!(plugins.unknown);
    }

    #[test]
    fn missing_plugins_section_defaults_to_enabled() {
        let plugins = extract("").expect("could not parse configuration");

        assert!(plugins.dig.enabled);
        assert_eq!(
            plugins.dig.settings.nameservers,
            crate::dig::Settings::default().nameservers
        );
        assert!(plugins.health.enabled);
    }

    #[test]
    fn omitted_enabled_defaults_to_true() {
        let plugins = extract("[plugins.health]\n").expect("could not parse configuration");

        assert!(plugins.health.enabled);
    }

    #[test]
    fn unknown_sections_are_captured() {
        let plugins =
            extract("[plugins.digg]\nenabled = false\n").expect("could not parse configuration");

        assert!(plugins.unknown.contains_key("digg"));
    }

    #[test]
    fn invalid_enabled_type_is_rejected() {
        let error = extract("[plugins.health]\nenabled = \"yes\"\n")
            .expect_err("invalid type should be rejected");

        assert!(error.to_string().contains("enabled"), "{error}");
    }

    #[test]
    fn invalid_settings_type_is_rejected() {
        let error = extract("[plugins.dig]\nnameservers = \"1.1.1.1\"\n")
            .expect_err("invalid nameservers type should be rejected");

        // `flatten` stops the reported key path at the section; the section is still identified.
        assert!(error.to_string().contains("dig"), "{error}");
    }

    #[test]
    fn invalid_ip_address_is_rejected() {
        assert!(
            extract("[plugins.dig]\nnameservers = [\"not-an-ip\"]\n").is_err(),
            "invalid IP address should be rejected"
        );
    }

    #[test]
    fn empty_nameservers_are_rejected() {
        assert!(
            extract("[plugins.dig]\nnameservers = []\n").is_err(),
            "empty nameservers should be rejected"
        );
    }

    #[test]
    fn plugin_config_round_trips() {
        let plugins = extract("[plugins.dig]\nnameservers = [\"1.1.1.1\"]\n")
            .expect("could not parse configuration");

        let json = serde_json::to_value(&plugins.dig).expect("could not serialize");
        assert_eq!(json["enabled"], serde_json::json!(true));
        assert_eq!(json["nameservers"], serde_json::json!(["1.1.1.1"]));

        let round_tripped: PluginConfig<crate::dig::Settings> =
            serde_json::from_value(json).expect("could not deserialize");
        assert_eq!(
            round_tripped.settings.nameservers,
            plugins.dig.settings.nameservers
        );
    }

    #[test]
    fn unknown_settings_keys_are_ignored() {
        let plugins = extract("[plugins.dig]\nnameserverss = [\"1.1.1.1\"]\n")
            .expect("unknown settings keys should be ignored");

        assert_eq!(plugins.dig.unknown_keys, ["nameserverss"]);
        assert_eq!(
            plugins.dig.settings.nameservers,
            crate::dig::Settings::default().nameservers
        );
    }

    #[test]
    fn unknown_settings_keys_are_ignored_for_plugins_without_settings() {
        let plugins = extract("[plugins.health]\nenabled = true\nwhatever = 1\n")
            .expect("unknown settings keys should be ignored");

        assert_eq!(plugins.health.unknown_keys, ["whatever"]);
    }

    #[test]
    fn non_table_section_is_rejected() {
        assert!(
            extract("[plugins]\ndig = \"x\"\n").is_err(),
            "scalar plugin section should be rejected"
        );
    }

    /// The plugin sections of the repository `config.toml` must keep parsing and hold the
    /// values the documentation quotes.
    #[test]
    fn repository_plugin_sections_parse() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config.toml");

        let config = Figment::new()
            .merge(Toml::file(path))
            .focus("plugins")
            .extract::<PluginsConfig>()
            .expect("the repository config.toml should parse");

        assert!(config.health.enabled);
        assert_eq!(
            config.dig.settings.nameservers,
            vec![
                IpAddr::from([1, 1, 1, 1]),
                IpAddr::from([1, 0, 0, 1]),
                IpAddr::from([0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111]),
                IpAddr::from([0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1001]),
            ]
        );
        assert_empty!(config.unknown);
    }

    #[test]
    fn full_config_without_plugins_section_parses() {
        let config = Figment::new()
            .merge(Toml::string(FULL_BASE))
            .focus("plugins")
            .extract::<PluginsConfig>()
            .expect("configuration without a [plugins] section should parse");

        assert!(config.dig.enabled);
        assert!(config.health.enabled);
        assert_empty!(config.unknown);
    }
}
