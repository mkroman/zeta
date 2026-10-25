//! Configuration loading and validation.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::consts;

#[cfg(feature = "mirror")]
pub use zeta_mirror::MirrorConfig;
pub use zeta_plugin::config::HttpConfig;

/// The plugin-visible subset of the configuration, as carried on the plugins' context.
impl From<&Config> for zeta_plugin::config::HostConfig {
    fn from(config: &Config) -> Self {
        Self {
            http: config.http.clone(),
            admin_hostmasks: config.irc.admin_hostmasks.clone(),
        }
    }
}

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
    pub mirror: MirrorConfig,
    /// Per-plugin configuration sections.
    ///
    /// Consumed by [`Config::take_plugins`] during startup, before any plugin is constructed; the
    /// config kept in the context holds defaults for every plugin section afterwards.
    #[serde(default)]
    plugins: plugins::PluginsConfig,
}

impl Config {
    /// Removes and returns the per-plugin configuration sections.
    ///
    /// The host calls this once while starting up and hands each plugin its own section through
    /// its constructor. Afterwards the config holds defaults for all plugin sections.
    #[must_use]
    pub(crate) fn take_plugins(&mut self) -> plugins::PluginsConfig {
        std::mem::take(&mut self.plugins)
    }
}

/// Database connection configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DbConfig {
    /// Connection URL
    pub url: String,
    /// Minimum number of connections to keep active in the connection pool
    #[serde(default = "default_min_db_connections")]
    pub min_connections: u32,
    /// Maximum number of connections to keep active in the connection pool
    #[serde(default = "default_max_db_connections")]
    pub max_connections: u32,
    /// Maximum idle duration for individual connections, in seconds
    #[serde(default = "default_db_idle_timeout", with = "humantime_serde")]
    pub idle_timeout: Duration,
    /// Set the maximum lifetime of individual connections.
    ///
    /// Any connection with a lifetime greater than this will be closed.
    #[serde(default = "default_db_max_lifetime", with = "humantime_serde")]
    pub max_lifetime: Duration,
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
            consts::DEFAULT_IRC_TLS_PORT
        } else {
            consts::DEFAULT_IRC_PORT
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

/// Returns the default value for number of minimum database connections.
const fn default_min_db_connections() -> u32 {
    consts::DEFAULT_MIN_DB_CONNECTIONS
}

/// Returns the default value for number of maximum database connections.
const fn default_max_db_connections() -> u32 {
    consts::DEFAULT_MAX_DB_CONNECTIONS
}

/// Returns the default maximum lifetime of database pool connections.
const fn default_db_idle_timeout() -> Duration {
    consts::DEFAULT_DB_IDLE_TIMEOUT
}

/// Returns the default duration a connection can be idle before it is dropped.
const fn default_db_max_lifetime() -> Duration {
    consts::DEFAULT_DB_MAX_LIFETIME
}

/// Returns the default message sent in the `QUIT` when the bot shuts down.
fn default_shutdown_quit_message() -> String {
    consts::DEFAULT_SHUTDOWN_QUIT_MESSAGE.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    use figment::{
        Figment,
        providers::{Format, Toml},
    };
    use zeta_test_support::assert_empty;

    /// The base configuration the config tests start from.
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

    #[test]
    fn http_defaults_are_used_when_omitted() {
        let config: HttpConfig = Figment::new()
            .merge(Toml::string(""))
            .extract()
            .expect("could not parse http configuration");

        let defaults = HttpConfig::default();

        assert_eq!(config.timeout, defaults.timeout);
        assert_eq!(config.user_agent, defaults.user_agent);
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
    fn irc_quit_message_defaults_to_shutting_down() {
        let config: IrcConfig = Figment::new()
            .merge(Toml::string(
                "nickname = \"zeta\"\nhostname = \"localhost\"\nalt_nicks = []\nchannels = []\n",
            ))
            .extract()
            .expect("could not parse irc configuration");

        assert_eq!(config.quit_message, consts::DEFAULT_SHUTDOWN_QUIT_MESSAGE);
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

    /// The repository `config.toml` must parse into the full configuration: a broken edit has
    /// to fail here rather than at someone's deployment.
    #[test]
    fn repository_config_parses() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config.toml");

        let config = Figment::new()
            .merge(Toml::file(path))
            .extract::<Config>()
            .expect("the repository config.toml should parse");

        assert_empty!(config.plugins.unknown);
    }

    #[test]
    fn full_config_without_plugins_section_parses() {
        let config = Figment::new()
            .merge(Toml::string(FULL_BASE))
            .extract::<Config>()
            .expect("configuration without a [plugins] section should parse");

        assert_empty!(config.plugins.unknown);
    }

    #[test]
    fn take_plugins_removes_sections() {
        let mut config = Figment::new()
            .merge(Toml::string(&format!(
                "{FULL_BASE}\n[plugins.dig]\nnameservers = [\"1.1.1.1\"]\n"
            )))
            .extract::<Config>()
            .expect("configuration should parse");

        let plugins = config.take_plugins();

        // The sections were removed from the config and returned to the caller; the plugin
        // values themselves are asserted in the plugins crate's configuration tests.
        assert_empty!(plugins.unknown);
        assert!(plugins.sections.contains_key("dig"));
        assert!(config.plugins.sections.is_empty());
    }

    #[test]
    fn plugins_enabled_by_default_is_configurable() {
        let mut config = Figment::new()
            .merge(Toml::string(FULL_BASE))
            .merge(Toml::string("[plugins]\nenabled_by_default = false\n"))
            .extract::<Config>()
            .expect("configuration should parse");

        let plugins = config.take_plugins();

        // The toggle is host-managed; how it resolves each plugin's `enabled` is asserted in
        // the plugins crate's configuration tests.
        assert!(!plugins.enabled_by_default);
        assert_empty!(plugins.unknown);
    }

    #[test]
    fn host_config_mirrors_the_visible_subset() {
        let config = Figment::new()
            .merge(Toml::string(FULL_BASE))
            .merge(Toml::string("[irc]\nadmin_hostmasks = [\"mk!mk@*\"]\n"))
            .extract::<Config>()
            .expect("configuration should parse");

        let host = zeta_plugin::config::HostConfig::from(&config);

        assert_eq!(host.http, config.http);
        assert_eq!(host.admin_hostmasks, ["mk!mk@*"]);
    }
}
