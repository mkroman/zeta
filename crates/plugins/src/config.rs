//! Per-plugin configuration plumbing.
//!
//! Every bundled plugin registers itself through [`zeta_plugin::PluginRegistration`], so its
//! `[plugins.<name>]` section is resolved by name instead of through a typed field per plugin:
//! [`PluginsConfig`] keeps each section's raw settings next to the host-managed `enabled`
//! toggle; the plugin's settings type is validated when the configuration is loaded (even when
//! the plugin is disabled) and deserialized again when the plugin is constructed.
//! [`PluginConfig`] is the typed view of one section.

use std::collections::HashMap;

use figment::value::{Dict, Value};
use serde::de::{DeserializeOwned, Error as _};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeta_plugin::inventory;

/// An individual plugin's typed configuration: its `[plugins.<name>]` section.
///
/// Returned by [`PluginsConfig::config`] for the plugin's settings type. The `enabled` key is
/// managed by the host; a section that omits it inherits the global `enabled_by_default` under
/// `[plugins]` (`true` when that is omitted too). Every other key belongs to the plugin's
/// settings type. Unknown settings keys are ignored, collected in
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

impl<'de, S> PluginConfig<S>
where
    S: Deserialize<'de>,
{
    /// Deserializes a plugin section, resolving an omitted `enabled` key to `default_enabled`.
    ///
    /// The [`Deserialize`] implementation calls this with `true`, the standalone default.
    pub(crate) fn deserialize_with_default<D>(
        deserializer: D,
        default_enabled: bool,
    ) -> Result<Self, D::Error>
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
            None => default_enabled,
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

impl<'de, S> Deserialize<'de> for PluginConfig<S>
where
    S: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::deserialize_with_default(deserializer, true)
    }
}

/// The standalone default: the plugin is enabled, as a section omitted under the global
/// default (`enabled_by_default = true`) would be.
impl<S: Default> Default for PluginConfig<S> {
    fn default() -> Self {
        Self {
            enabled: true,
            settings: S::default(),
            unknown_keys: Vec::new(),
        }
    }
}

/// One raw `[plugins.<name>]` section.
///
/// The settings stay a [`figment::value::Value`] until the plugin's registration deserializes
/// them into its own settings type; `enabled` is already resolved against the global
/// [`PluginsConfig::enabled_by_default`] toggle.
#[derive(Clone, Debug)]
pub struct PluginSection {
    /// Whether the plugin should be registered, with an omitted key resolved against
    /// [`PluginsConfig::enabled_by_default`].
    pub enabled: bool,
    /// The section's settings, without the host-managed `enabled` key.
    pub settings: Value,
}

impl Serialize for PluginSection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("enabled", &self.enabled)?;
        match &self.settings {
            Value::Dict(_, settings) => {
                for (key, value) in settings {
                    map.serialize_entry(key, value)?;
                }
            }
            // `PluginSection::parse` always produces a dictionary; keep a non-dictionary value
            // addressable if a caller constructs one directly.
            settings => map.serialize_entry("settings", settings)?,
        }
        map.end()
    }
}

impl PluginSection {
    /// Splits one raw section value into its resolved `enabled` toggle and settings.
    ///
    /// Returns the settings as a [`Dict`] alongside the parsed section, so
    /// [`PluginsConfig`] can record unrecognized sections without parsing them twice.
    fn parse(value: &Value, enabled_by_default: bool) -> Result<(Self, Dict), Box<figment::Error>> {
        let mut section = Dict::deserialize(value).map_err(Box::new)?;
        let enabled = match section.remove("enabled") {
            Some(value) => bool::deserialize(&value).map_err(|error| {
                Box::new(figment::Error::custom(format!(
                    "invalid `enabled` key: {error}"
                )))
            })?,
            None => enabled_by_default,
        };

        let settings = Value::from(section.clone());

        Ok((Self { enabled, settings }, section))
    }
}

/// Per-plugin configuration extracted from the `[plugins]` section.
///
/// Sections are keyed by plugin name, e.g. `[plugins.dig]`, and resolved against the plugins
/// that registered themselves through [`zeta_plugin::PluginRegistration`]. Sections whose name
/// matches no bundled plugin are collected in [`PluginsConfig::unknown`] so the host can warn
/// about likely typos; a section for a bundled plugin compiled out by its cargo feature is not
/// a typo and stays out of that map.
///
/// A section that is omitted entirely follows the global [`PluginsConfig::enabled_by_default`]
/// and gives the plugin its default settings. The `enabled` key is managed by the host; every
/// other key belongs to the plugin's settings type, deserialized through
/// [`PluginsConfig::config`] or, at startup, by the plugin's registration.
#[derive(Clone, Debug)]
pub struct PluginsConfig {
    /// Whether a plugin that does not set `enabled` explicitly — or has no section at all — is
    /// enabled.
    ///
    /// `true` (the default) makes plugins opt-out: every compiled-in plugin is enabled unless
    /// its section says `enabled = false`. `false` makes them opt-in: a plugin only runs when
    /// its section says `enabled = true`.
    pub enabled_by_default: bool,
    /// The configuration sections, keyed by plugin name.
    ///
    /// Holds every section in the table, whether or not a plugin registration matches it.
    /// Sections whose name is not a bundled plugin are also mirrored in
    /// [`PluginsConfig::unknown`].
    pub sections: HashMap<String, PluginSection>,
    /// Sections that match no bundled plugin name.
    ///
    /// Collected during deserialization so the host can warn about likely typos. Compiled-out
    /// bundled plugins are not included: their sections are known, even though no registration
    /// consumed them.
    pub unknown: HashMap<String, Dict>,
}

impl Serialize for PluginsConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("enabled_by_default", &self.enabled_by_default)?;
        for (name, section) in &self.sections {
            map.serialize_entry(name, section)?;
        }
        map.end()
    }
}

impl PluginsConfig {
    /// Returns the typed configuration of the plugin named `name`.
    ///
    /// `name` is the plugin's [`Plugin::NAME`](zeta_plugin::Plugin::NAME), which is also its
    /// module name and its section key. A section that is absent from the configuration follows
    /// [`PluginsConfig::enabled_by_default`] and gives the plugin its default settings.
    ///
    /// # Errors
    ///
    /// Returns an error if the section's settings do not deserialize into `S` — a malformed
    /// value, or a settings type that does not match the configuration.
    pub fn config<S>(&self, name: &str) -> Result<PluginConfig<S>, Box<figment::Error>>
    where
        S: Default + DeserializeOwned,
    {
        let Some(section) = self.sections.get(name) else {
            return Ok(PluginConfig {
                enabled: self.enabled_by_default,
                settings: S::default(),
                unknown_keys: Vec::new(),
            });
        };

        let mut unknown_keys = Vec::new();
        let settings = serde_ignored::deserialize(&section.settings, |path| {
            unknown_keys.push(path.to_string());
        })
        .map_err(Box::new)?;

        Ok(PluginConfig {
            enabled: section.enabled,
            settings,
            unknown_keys,
        })
    }
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            enabled_by_default: true,
            sections: HashMap::new(),
            unknown: HashMap::new(),
        }
    }
}

impl<'de> Deserialize<'de> for PluginsConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error as _;

        let mut values = HashMap::<String, Value>::deserialize(deserializer)?;

        // The global opt-in/opt-out toggle is pulled out first so it is not reported as an
        // unknown section.
        let enabled_by_default = match values.remove("enabled_by_default") {
            Some(value) => bool::deserialize(&value).map_err(|error| {
                D::Error::custom(format!("invalid `enabled_by_default` key: {error}"))
            })?,
            None => true,
        };

        // Validate the settings of every plugin that is compiled in, whether it is enabled or
        // not: a malformed section is a configuration error, not a reason to skip the plugin.
        let registrations: HashMap<&'static str, &zeta_plugin::PluginRegistration> =
            inventory::iter::<zeta_plugin::PluginRegistration>
                .into_iter()
                .map(|registration| (registration.name, registration))
                .collect();

        let mut sections = HashMap::with_capacity(values.len());
        let mut unknown = HashMap::new();

        for (name, value) in values {
            let (section, dict) =
                PluginSection::parse(&value, enabled_by_default).map_err(|error| {
                    D::Error::custom(format!("invalid `{name}` plugin section: {error}"))
                })?;

            if let Some(registration) = registrations.get(name.as_str()) {
                registration
                    .validate_settings(Some(&section.settings))
                    .map_err(|error| {
                        D::Error::custom(format!("invalid `{name}` plugin section: {error}"))
                    })?;
            } else if !crate::BUNDLED_PLUGIN_NAMES.contains(&name.as_str()) {
                // A bundled plugin that was compiled out is not a typo; its section is simply
                // not consumed in this build.
                unknown.insert(name.clone(), dict);
            }

            sections.insert(name, section);
        }

        Ok(Self {
            enabled_by_default,
            sections,
            unknown,
        })
    }
}

#[cfg(all(test, feature = "dig", feature = "health"))]
mod tests {
    use std::net::IpAddr;

    use figment::{
        Error, Figment,
        providers::{Format, Toml},
    };
    use zeta_plugin::NoSettings;
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

        let dig = plugins
            .config::<crate::dig::Settings>("dig")
            .expect("dig settings should deserialize");
        assert!(dig.enabled);
        assert_eq!(
            dig.settings.nameservers,
            vec![
                IpAddr::from([1, 1, 1, 1]),
                IpAddr::from([0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111]),
            ]
        );

        let health = plugins
            .config::<NoSettings>("health")
            .expect("health settings should deserialize");
        assert!(!health.enabled);
        assert_empty!(plugins.unknown);
    }

    #[test]
    fn missing_plugins_section_defaults_to_enabled() {
        let plugins = extract("").expect("could not parse configuration");

        assert!(plugins.enabled_by_default);

        let dig = plugins
            .config::<crate::dig::Settings>("dig")
            .expect("dig settings should deserialize");
        assert!(dig.enabled);
        assert_eq!(
            dig.settings.nameservers,
            crate::dig::Settings::default().nameservers
        );

        let health = plugins
            .config::<NoSettings>("health")
            .expect("health settings should deserialize");
        assert!(health.enabled);
    }

    #[test]
    fn omitted_enabled_defaults_to_true() {
        let plugins = extract("[plugins.health]\n").expect("could not parse configuration");

        assert!(plugins.enabled_by_default);

        let health = plugins
            .config::<NoSettings>("health")
            .expect("health settings should deserialize");
        assert!(health.enabled);
    }

    #[test]
    fn enabled_by_default_false_opts_plugins_in() {
        let plugins = extract(
            r"
[plugins]
enabled_by_default = false

[plugins.dig]
enabled = true
",
        )
        .expect("could not parse configuration");

        assert!(!plugins.enabled_by_default);
        // `dig` opted itself in with an explicit toggle; `health` has no section at all and
        // follows the global default.
        assert!(
            plugins
                .config::<crate::dig::Settings>("dig")
                .expect("dig settings should deserialize")
                .enabled
        );
        assert!(
            !plugins
                .config::<NoSettings>("health")
                .expect("health settings should deserialize")
                .enabled
        );
        // The global toggle itself is not reported as an unknown section.
        assert_empty!(plugins.unknown);
    }

    #[test]
    fn explicit_enabled_overrides_the_global_default() {
        let plugins = extract(
            r"
[plugins]
enabled_by_default = true

[plugins.health]
enabled = false
",
        )
        .expect("could not parse configuration");

        assert!(plugins.enabled_by_default);
        assert!(
            !plugins
                .config::<NoSettings>("health")
                .expect("health settings should deserialize")
                .enabled
        );
    }

    #[test]
    fn invalid_enabled_by_default_type_is_rejected() {
        let error = extract("[plugins]\nenabled_by_default = \"yes\"\n")
            .expect_err("invalid type should be rejected");

        assert!(error.to_string().contains("enabled_by_default"), "{error}");
    }

    #[test]
    fn unknown_sections_are_captured() {
        let plugins =
            extract("[plugins.digg]\nenabled = false\n").expect("could not parse configuration");

        assert!(plugins.unknown.contains_key("digg"));
        assert!(plugins.sections.contains_key("digg"));
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

        assert!(error.to_string().contains("dig"), "{error}");
    }

    #[test]
    fn invalid_ip_address_is_rejected() {
        let error = extract("[plugins.dig]\nnameservers = [\"not-an-ip\"]\n")
            .expect_err("invalid IP address should be rejected");

        assert!(error.to_string().contains("dig"), "{error}");
    }

    #[test]
    fn empty_nameservers_are_rejected() {
        let error = extract("[plugins.dig]\nnameservers = []\n")
            .expect_err("empty nameservers should be rejected");

        assert!(error.to_string().contains("dig"), "{error}");
    }

    #[test]
    fn invalid_settings_are_rejected_for_disabled_plugins() {
        let error = extract("[plugins.dig]\nenabled = false\nnameservers = \"1.1.1.1\"\n")
            .expect_err("invalid settings should be rejected even when the plugin is disabled");

        assert!(error.to_string().contains("dig"), "{error}");
    }

    #[test]
    fn plugin_config_round_trips() {
        let plugins = extract("[plugins.dig]\nnameservers = [\"1.1.1.1\"]\n")
            .expect("could not parse configuration");
        let dig = plugins
            .config::<crate::dig::Settings>("dig")
            .expect("dig settings should deserialize");

        let json = serde_json::to_value(&dig).expect("could not serialize");
        assert_eq!(json["enabled"], serde_json::json!(true));
        assert_eq!(json["nameservers"], serde_json::json!(["1.1.1.1"]));

        let round_tripped: PluginConfig<crate::dig::Settings> =
            serde_json::from_value(json).expect("could not deserialize");
        assert_eq!(round_tripped.settings.nameservers, dig.settings.nameservers);
    }

    #[test]
    fn plugins_config_round_trips() {
        let plugins = extract("[plugins.dig]\nnameservers = [\"1.1.1.1\"]\n")
            .expect("could not parse configuration");

        let json = serde_json::to_value(&plugins).expect("could not serialize");
        assert_eq!(json["enabled_by_default"], serde_json::json!(true));
        assert_eq!(json["dig"]["enabled"], serde_json::json!(true));
        assert_eq!(json["dig"]["nameservers"], serde_json::json!(["1.1.1.1"]));
        assert!(
            json.get("sections").is_none(),
            "the serialized shape must stay flat: {json}"
        );

        let round_tripped: PluginsConfig =
            serde_json::from_value(json).expect("could not deserialize");
        assert_eq!(
            round_tripped.sections["dig"].settings,
            plugins.sections["dig"].settings
        );
    }

    #[test]
    fn unknown_settings_keys_are_ignored() {
        let plugins = extract("[plugins.dig]\nnameserverss = [\"1.1.1.1\"]\n")
            .expect("unknown settings keys should be ignored");
        let dig = plugins
            .config::<crate::dig::Settings>("dig")
            .expect("dig settings should deserialize");

        assert_eq!(dig.unknown_keys, ["nameserverss"]);
        assert_eq!(
            dig.settings.nameservers,
            crate::dig::Settings::default().nameservers
        );
    }

    #[test]
    fn unknown_settings_keys_are_ignored_for_plugins_without_settings() {
        let plugins = extract("[plugins.health]\nenabled = true\nwhatever = 1\n")
            .expect("unknown settings keys should be ignored");
        let health = plugins
            .config::<NoSettings>("health")
            .expect("health settings should deserialize");

        assert_eq!(health.unknown_keys, ["whatever"]);
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

        let health = config
            .config::<NoSettings>("health")
            .expect("health settings should deserialize");
        assert!(health.enabled);
        assert!(config.enabled_by_default);

        let dig = config
            .config::<crate::dig::Settings>("dig")
            .expect("dig settings should deserialize");
        assert_eq!(
            dig.settings.nameservers,
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

        assert!(config.enabled_by_default);
        assert!(
            config
                .config::<crate::dig::Settings>("dig")
                .expect("dig settings should deserialize")
                .enabled
        );
        assert!(
            config
                .config::<NoSettings>("health")
                .expect("health settings should deserialize")
                .enabled
        );
        assert_empty!(config.unknown);
    }
}
