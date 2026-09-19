use std::time::Duration;

use figment::value::{Dict, Value};
use serde::{Deserialize, Deserializer, Serialize};

use crate::consts::{
    DEFAULT_DB_IDLE_TIMEOUT, DEFAULT_IRC_PORT, DEFAULT_IRC_TLS_PORT, DEFAULT_MAX_DB_CONNECTIONS,
    DEFAULT_SHUTDOWN_QUIT_MESSAGE, HTTP_TIMEOUT, HTTP_USER_AGENT,
};
use crate::plugin::PluginsConfig;

/// Main application configuration structure.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    /// Database configuration
    pub database: DbConfig,
    /// Tracing configuration
    pub tracing: TracingConfig,
    /// IRC client configuration
    pub irc: IrcConfig,
    /// HTTP client configuration
    #[serde(default)]
    pub http: HttpConfig,
    /// Shared media mirroring configuration
    #[cfg(feature = "mirror")]
    #[serde(default)]
    pub mirror: crate::mirror::MirrorConfig,
    /// Per-plugin configuration sections.
    ///
    /// Consumed by [`Config::take_plugins`] during startup, before any plugin is constructed; the
    /// config kept in the context holds defaults for every plugin section afterwards.
    #[serde(default)]
    plugins: PluginsConfig,
}

impl Config {
    /// Removes and returns the per-plugin configuration sections.
    ///
    /// The host calls this once while starting up and hands each plugin its own section through
    /// its constructor. Afterwards the config holds defaults for all plugin sections.
    #[must_use]
    pub(crate) fn take_plugins(&mut self) -> PluginsConfig {
        std::mem::take(&mut self.plugins)
    }
}

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

/// Database connection configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DbConfig {
    /// Connection URL
    pub url: String,
    /// Maximum number of connections to keep in the connection pool
    #[serde(default = "default_max_db_connections")]
    pub max_connections: u32,
    /// Maximum idle duration for individual connections, in seconds
    #[serde(default = "default_db_idle_timeout", with = "humantime_serde")]
    pub idle_timeout: Duration,
}

/// HTTP client configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HttpConfig {
    /// Duration before an HTTP request times out.
    #[serde(default = "default_http_timeout", with = "humantime_serde")]
    pub timeout: Duration,
    /// The `User-Agent` header sent with HTTP requests.
    #[serde(default = "default_http_user_agent")]
    pub user_agent: String,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout: default_http_timeout(),
            user_agent: default_http_user_agent(),
        }
    }
}

/// Tracing and logging configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TracingConfig {
    /// Enable tracing
    pub enabled: bool,
}

/// Configuration for an individual IRC channel.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct IrcChannelConfig {
    /// Name of the channel.
    pub name: String,
    /// The shared key to access the channel.
    pub key: Option<String>,
}

/// TLS configuration for IRC connection.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct IrcTlsConfig {
    /// Toggle TLS.
    pub enabled: bool,
}

/// IRC client configuration.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct IrcConfig {
    /// Hostmasks of admin users, in `nick!user@hostname` form, where any of the three
    /// components may contain wildcards (`*` or `?`), e.g. `mk!mk@*` or `*!*@example.com`.
    /// Admins are authorized to manage URL and sender filters with the `.filter` command;
    /// with no hostmasks configured, nobody is an admin.
    #[serde(default)]
    pub admin_hostmasks: Vec<String>,
    /// Alternative nicknames for the client, if the default is taken.
    pub alt_nicks: Vec<String>,
    /// List of channels to automatically manage.
    pub channels: Vec<IrcChannelConfig>,
    /// The encoding type used for this connection. This is typically UTF-8, but could be something
    /// else.
    pub encoding: Option<String>,
    /// The hostname of the server to connect to.
    pub hostname: String,
    /// The client’s NICKSERV password.
    pub nick_password: Option<String>,
    /// The client's nickname.
    pub nickname: String,
    /// The password to connect to the server.
    pub password: Option<String>,
    /// The port number of the server to connect to.
    pub port: Option<u16>,
    /// The client's real name.
    pub realname: Option<String>,
    /// Whether the client should use `NickServ` GHOST to reclaim its primary nickname if it is in
    /// use.
    #[serde(default)]
    pub should_ghost: bool,
    /// The message sent in the `QUIT` when the bot shuts down gracefully (on `SIGINT` or
    /// `SIGTERM`), e.g. when a container orchestrator stops the pod.
    #[serde(default = "default_shutdown_quit_message")]
    pub quit_message: String,
    /// TLS configuration.
    pub tls: Option<IrcTlsConfig>,
    /// The client's username.
    pub username: Option<String>,
}

impl Default for IrcConfig {
    fn default() -> Self {
        Self {
            admin_hostmasks: Vec::new(),
            alt_nicks: Vec::new(),
            channels: Vec::new(),
            encoding: None,
            hostname: String::new(),
            nick_password: None,
            nickname: String::new(),
            password: None,
            port: None,
            realname: None,
            should_ghost: false,
            quit_message: default_shutdown_quit_message(),
            tls: None,
            username: None,
        }
    }
}

impl IrcConfig {
    /// Returns the port number to use for this IRC connection.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port.unwrap_or_else(|| self.fallback_port())
    }

    /// Returns whether TLS is enabled for this IRC connection.
    fn is_tls_enabled(&self) -> bool {
        self.tls.as_ref().is_some_and(|x| x.enabled)
    }

    /// Return the port number to use based on whether the connection requires TLS or not.
    fn fallback_port(&self) -> u16 {
        if self.is_tls_enabled() {
            DEFAULT_IRC_TLS_PORT
        } else {
            DEFAULT_IRC_PORT
        }
    }
}

impl From<IrcConfig> for irc::client::data::Config {
    fn from(config: IrcConfig) -> Self {
        let port = config.port();
        let channels: Vec<String> = config
            .channels
            .iter()
            .map(|channel| channel.name.clone())
            .collect();
        // TODO: channel keys
        let use_tls = config.tls.map(|x| x.enabled);

        Self {
            nickname: Some(config.nickname),
            nick_password: config.nick_password,
            server: Some(config.hostname),
            port: Some(port),
            use_tls,
            channels,
            alt_nicks: config.alt_nicks,
            should_ghost: config.should_ghost,
            ..Default::default()
        }
    }
}

/// Returns the default value for number of maximum database connections.
const fn default_max_db_connections() -> u32 {
    DEFAULT_MAX_DB_CONNECTIONS
}

/// Returns the default duration a connection can be idle before it is dropped.
const fn default_db_idle_timeout() -> Duration {
    DEFAULT_DB_IDLE_TIMEOUT
}

/// Returns the default duration before an HTTP request times out.
const fn default_http_timeout() -> Duration {
    HTTP_TIMEOUT
}

/// Returns the default `User-Agent` header sent with HTTP requests.
fn default_http_user_agent() -> String {
    HTTP_USER_AGENT.to_string()
}

/// Returns the default message sent in the `QUIT` when the bot shuts down.
fn default_shutdown_quit_message() -> String {
    DEFAULT_SHUTDOWN_QUIT_MESSAGE.to_string()
}

#[cfg(all(test, feature = "plugin-dig", feature = "plugin-health"))]
mod tests {
    use std::net::IpAddr;

    use super::*;
    use figment::{
        Error, Figment,
        providers::{Format, Toml},
    };

    /// Extracts the `[plugins]` subtree from an inline TOML document.
    fn extract(toml: &str) -> Result<PluginsConfig, Box<Error>> {
        Figment::new()
            .merge(Toml::string(toml))
            .focus("plugins")
            .extract()
            .map_err(Box::new)
    }

    #[test]
    fn http_defaults_are_used_when_omitted() {
        let config: HttpConfig = Figment::new()
            .merge(Toml::string(""))
            .extract()
            .expect("could not parse http configuration");

        assert_eq!(config.timeout, HTTP_TIMEOUT);
        assert_eq!(config.user_agent, HTTP_USER_AGENT);
    }

    #[test]
    fn http_settings_parse() {
        let config: HttpConfig = Figment::new()
            .merge(Toml::string(
                "timeout = \"10s\"\nuser_agent = \"zeta/test\"\n",
            ))
            .extract()
            .expect("could not parse http configuration");

        assert_eq!(config.timeout, Duration::from_secs(10));
        assert_eq!(config.user_agent, "zeta/test");
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
        assert!(plugins.unknown.is_empty());
    }

    #[test]
    fn missing_plugins_section_defaults_to_enabled() {
        let plugins = extract("").expect("could not parse configuration");

        assert!(plugins.dig.enabled);
        assert_eq!(
            plugins.dig.settings.nameservers,
            crate::plugin::dig::Settings::default().nameservers
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

        let round_tripped: PluginConfig<crate::plugin::dig::Settings> =
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
            crate::plugin::dig::Settings::default().nameservers
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

    #[test]
    fn irc_quit_message_defaults_to_shutting_down() {
        let config: IrcConfig = Figment::new()
            .merge(Toml::string(
                "nickname = \"zeta\"\nhostname = \"localhost\"\nalt_nicks = []\nchannels = []\n",
            ))
            .extract()
            .expect("could not parse irc configuration");

        assert_eq!(config.quit_message, DEFAULT_SHUTDOWN_QUIT_MESSAGE);
    }

    #[test]
    fn irc_quit_message_is_configurable() {
        let config: IrcConfig = Figment::new()
            .merge(Toml::string(
                "nickname = \"zeta\"\nhostname = \"localhost\"\nalt_nicks = []\nchannels = []\nquit_message = \"goodbye\"\n",
            ))
            .extract()
            .expect("could not parse irc configuration");

        assert_eq!(config.quit_message, "goodbye");
    }

    #[test]
    fn repository_config_parses() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../config.toml");

        let config = Figment::new()
            .merge(Toml::file(path))
            .extract::<Config>()
            .expect("the repository config.toml should parse");

        assert!(config.plugins.health.enabled);
        assert_eq!(
            config.plugins.dig.settings.nameservers,
            vec![
                IpAddr::from([1, 1, 1, 1]),
                IpAddr::from([1, 0, 0, 1]),
                IpAddr::from([0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111]),
                IpAddr::from([0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1001]),
            ]
        );
        assert!(config.plugins.unknown.is_empty());
    }

    #[test]
    fn full_config_without_plugins_section_parses() {
        let config = Figment::new()
            .merge(Toml::string(
                r#"
[database]
url = "postgresql://localhost/zeta_test"

[tracing]
enabled = true

[irc]
nickname = "zeta"
hostname = "localhost"
alt_nicks = []
channels = []
"#,
            ))
            .extract::<Config>()
            .expect("configuration without a [plugins] section should parse");

        assert!(config.plugins.dig.enabled);
        assert!(config.plugins.health.enabled);
        assert!(config.plugins.unknown.is_empty());
    }

    #[test]
    fn take_plugins_removes_sections() {
        let mut config = Figment::new()
            .merge(Toml::string(
                r#"
[database]
url = "postgresql://localhost/zeta_test"

[tracing]
enabled = true

[irc]
nickname = "zeta"
hostname = "localhost"
alt_nicks = []
channels = []

[plugins.dig]
nameservers = ["1.1.1.1"]
"#,
            ))
            .extract::<Config>()
            .expect("configuration should parse");

        let plugins = config.take_plugins();

        assert_eq!(
            plugins.dig.settings.nameservers,
            vec![IpAddr::from([1, 1, 1, 1])]
        );
        assert_eq!(
            config.plugins.dig.settings.nameservers,
            crate::plugin::dig::Settings::default().nameservers
        );
    }
}
