//! Configuration loading and validation.

use std::{borrow::Cow, collections::HashMap, fmt, time::Duration};

use figment::{
    Figment,
    providers::{Env, Format, Toml},
};
use miette::{IntoDiagnostic, WrapErr};
use serde::{Deserialize, Serialize};

use crate::consts;

pub use zeta_http::HttpConfig;
#[cfg(feature = "mirror")]
pub use zeta_mirror::MirrorConfig;

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
    /// Loads the configuration from `path`, with `ZETA_`-prefixed environment overrides.
    ///
    /// Environment variables override the file, with `__` separating sections (e.g.
    /// `ZETA_IRC__NICK_PASSWORD`).
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or the merged configuration is invalid.
    pub fn load(path: impl AsRef<std::path::Path>) -> miette::Result<Self> {
        Self::from_figment(
            &Figment::new()
                .merge(Toml::file(path))
                .merge(Env::prefixed("ZETA_").lowercase(false).split("__")),
        )
    }

    /// Extracts the configuration from an already-built provider chain.
    fn from_figment(figment: &Figment) -> miette::Result<Self> {
        let config: Self = figment.extract().into_diagnostic()?;

        config
            .http
            .validate()
            .into_diagnostic()
            .wrap_err("invalid `[http].user_agent`")?;

        Ok(config)
    }

    /// Removes and returns the per-plugin configuration sections.
    ///
    /// The host calls this once while starting up and hands each plugin its own section through
    /// its constructor. Afterwards the config holds defaults for all plugin sections.
    #[must_use]
    pub(crate) fn take_plugins(&mut self) -> plugins::PluginsConfig {
        std::mem::take(&mut self.plugins)
    }
}

/// Renders a secret as `<redacted>` when it is set, and as `None` when it is not.
///
/// Configuration dumps still show whether a secret is configured, which is what operators need,
/// without printing its value.
struct Secret<'a>(Option<&'a str>);

impl fmt::Debug for Secret<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(_) => f.write_str("<redacted>"),
            None => f.write_str("None"),
        }
    }
}

/// Database connection configuration.
#[derive(Clone, Deserialize, Serialize)]
pub struct DbConfig {
    /// Connection URL
    pub url: String,
    /// Minimum number of connections to keep active in the connection pool
    #[serde(default = "default_min_db_connections")]
    pub min_connections: u32,
    /// Maximum number of connections to keep active in the connection pool
    #[serde(default = "default_max_db_connections")]
    pub max_connections: u32,
    /// Maximum idle duration for individual connections.
    #[serde(default = "default_db_idle_timeout", with = "humantime_serde")]
    pub idle_timeout: Duration,
    /// Set the maximum lifetime of individual connections.
    ///
    /// Any connection with a lifetime greater than this will be closed.
    #[serde(default = "default_db_max_lifetime", with = "humantime_serde")]
    pub max_lifetime: Duration,
}

// Manual implementation so the password embedded in the connection URL is never printed.
impl fmt::Debug for DbConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbConfig")
            .field("url", &RedactedUrlPassword(&self.url))
            .field("min_connections", &self.min_connections)
            .field("max_connections", &self.max_connections)
            .field("idle_timeout", &self.idle_timeout)
            .field("max_lifetime", &self.max_lifetime)
            .finish()
    }
}

/// Renders a connection URL with its password redacted.
struct RedactedUrlPassword<'a>(&'a str);

impl fmt::Debug for RedactedUrlPassword<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&redact_url_password(self.0), f)
    }
}

/// Replaces the password in a connection URL with `<redacted>`.
///
/// For example, `postgresql://zeta:secret@localhost/zeta` becomes
/// `postgresql://zeta:<redacted>@localhost/zeta`. A value without the `scheme://userinfo@host`
/// structure is redacted entirely, since there is no way to tell which part of it is the secret.
fn redact_url_password(url: &str) -> Cow<'_, str> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Cow::Borrowed("<redacted>");
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);

    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return Cow::Borrowed(url);
    };
    let Some((user, _password)) = userinfo.split_once(':') else {
        return Cow::Borrowed(url);
    };

    Cow::Owned(format!("{scheme}://{user}:<redacted>@{host}{tail}"))
}

/// Tracing and logging configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TracingConfig {
    /// Enable tracing
    pub enabled: bool,
    /// The deployment environment reported as `deployment.environment.name`.
    ///
    /// Falls back to the `ZETA_ENVIRONMENT` environment variable when unset.
    #[serde(default)]
    pub environment: Option<String>,
}

/// Configuration for an individual IRC channel.
#[derive(Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct IrcChannelConfig {
    /// Name of the channel.
    pub name: String,
    /// The shared key to access the channel.
    pub key: Option<String>,
}

// Manual implementation so the channel key is never printed.
impl fmt::Debug for IrcChannelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IrcChannelConfig")
            .field("name", &self.name)
            .field("key", &Secret(self.key.as_deref()))
            .finish()
    }
}

/// TLS configuration for IRC connection.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct IrcTlsConfig {
    /// Toggle TLS.
    pub enabled: bool,
}

/// IRC client configuration.
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct IrcConfig {
    /// Hostmasks of admin users, in `nick!user@hostname` form, where any of the three
    /// components may contain wildcards (`*` or `?`), e.g. `mk!mk@*` or `*!*@example.com`.
    /// Admins are authorized to manage URL and sender filters with the `.filter` command and
    /// unwall's site list with `.unwall`; with no hostmasks configured, nobody is an admin.
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

// Manual implementation so the `NickServ` and server passwords are never printed.
impl fmt::Debug for IrcConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IrcConfig")
            .field("admin_hostmasks", &self.admin_hostmasks)
            .field("alt_nicks", &self.alt_nicks)
            .field("channels", &self.channels)
            .field("encoding", &self.encoding)
            .field("hostname", &self.hostname)
            .field("nick_password", &Secret(self.nick_password.as_deref()))
            .field("nickname", &self.nickname)
            .field("password", &Secret(self.password.as_deref()))
            .field("port", &self.port)
            .field("realname", &self.realname)
            .field("should_ghost", &self.should_ghost)
            .field("quit_message", &self.quit_message)
            .field("tls", &self.tls)
            .field("username", &self.username)
            .finish()
    }
}

impl IrcConfig {
    /// Returns the port number to use for this IRC connection: the configured port, or the TLS
    /// or plaintext port depending on whether TLS is enabled.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port.unwrap_or_else(|| {
            if self.tls.as_ref().is_some_and(|tls| tls.enabled) {
                consts::DEFAULT_IRC_TLS_PORT
            } else {
                consts::DEFAULT_IRC_PORT
            }
        })
    }
}

impl From<&IrcConfig> for irc::client::data::Config {
    fn from(config: &IrcConfig) -> Self {
        let port = config.port();
        // `irc` treats `None` as TLS enabled, which would pair TLS with the plaintext port the
        // omitted `[irc.tls]` section implies; the default must be spelled out explicitly.
        let use_tls = Some(config.tls.as_ref().is_some_and(|tls| tls.enabled));
        let mut channels = Vec::with_capacity(config.channels.len());
        let mut channel_keys = HashMap::new();

        for channel in &config.channels {
            if let Some(key) = &channel.key {
                channel_keys.insert(channel.name.clone(), key.clone());
            }

            channels.push(channel.name.clone());
        }

        Self {
            nickname: Some(config.nickname.clone()),
            nick_password: config.nick_password.clone(),
            username: config.username.clone(),
            realname: config.realname.clone(),
            server: Some(config.hostname.clone()),
            port: Some(port),
            password: config.password.clone(),
            encoding: config.encoding.clone(),
            use_tls,
            channels,
            channel_keys,
            alt_nicks: config.alt_nicks.clone(),
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

/// Returns the default duration a connection can be idle before it is dropped.
const fn default_db_idle_timeout() -> Duration {
    consts::DEFAULT_DB_IDLE_TIMEOUT
}

/// Returns the default maximum lifetime of database pool connections.
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
    fn invalid_http_user_agent_is_rejected() {
        let toml = format!("{FULL_BASE}\n[http]\nuser_agent = \"bad\\nagent\"\n");
        let figment = Figment::new().merge(Toml::string(&toml));

        let error =
            Config::from_figment(&figment).expect_err("an invalid user agent should be rejected");

        assert!(error.to_string().contains("user_agent"), "{error}");
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

    #[test]
    fn irc_config_conversion_carries_the_connection_settings() {
        let config = IrcConfig {
            alt_nicks: vec!["zeta_".to_string()],
            channels: vec![
                IrcChannelConfig {
                    name: "#zeta".to_string(),
                    key: Some("channel-secret".to_string()),
                },
                IrcChannelConfig {
                    name: "#other".to_string(),
                    key: None,
                },
            ],
            encoding: Some("utf-8".to_string()),
            hostname: "irc.example.com".to_string(),
            nick_password: Some("nickserv-secret".to_string()),
            nickname: "zeta".to_string(),
            password: Some("server-secret".to_string()),
            port: Some(7000),
            realname: Some("Zeta".to_string()),
            should_ghost: true,
            tls: Some(IrcTlsConfig { enabled: true }),
            username: Some("zeta".to_string()),
            ..IrcConfig::default()
        };

        let config = irc::client::data::Config::from(&config);

        assert_eq!(config.nickname.as_deref(), Some("zeta"));
        assert_eq!(config.nick_password.as_deref(), Some("nickserv-secret"));
        assert_eq!(config.username.as_deref(), Some("zeta"));
        assert_eq!(config.realname.as_deref(), Some("Zeta"));
        assert_eq!(config.server.as_deref(), Some("irc.example.com"));
        assert_eq!(config.port, Some(7000));
        assert_eq!(config.password.as_deref(), Some("server-secret"));
        assert_eq!(config.encoding.as_deref(), Some("utf-8"));
        assert_eq!(config.use_tls, Some(true));
        assert_eq!(config.channels, ["#zeta", "#other"]);
        assert_eq!(
            config.channel_keys.get("#zeta").map(String::as_str),
            Some("channel-secret")
        );
        assert_eq!(config.alt_nicks, ["zeta_"]);
        assert!(config.should_ghost);
    }

    #[test]
    fn omitted_tls_section_is_plaintext() {
        let config: IrcConfig = Figment::new()
            .merge(Toml::string(
                "nickname = \"zeta\"\nhostname = \"localhost\"\nalt_nicks = []\nchannels = []\n",
            ))
            .extract()
            .expect("could not parse irc configuration");

        // An omitted section means plaintext, and the conversion must say so explicitly: `irc`
        // would otherwise treat `None` as TLS enabled and pair TLS with the plaintext port.
        assert_eq!(config.port(), consts::DEFAULT_IRC_PORT);

        let converted = irc::client::data::Config::from(&config);

        assert_eq!(converted.use_tls, Some(false));
        assert_eq!(converted.port, Some(consts::DEFAULT_IRC_PORT));

        let config: IrcConfig = Figment::new()
            .merge(Toml::string(
                "nickname = \"zeta\"\nhostname = \"localhost\"\nalt_nicks = []\nchannels = []\n[ tls ]\nenabled = true\n",
            ))
            .extract()
            .expect("could not parse irc configuration");

        assert_eq!(config.port(), consts::DEFAULT_IRC_TLS_PORT);

        let converted = irc::client::data::Config::from(&config);

        assert_eq!(converted.use_tls, Some(true));
        assert_eq!(converted.port, Some(consts::DEFAULT_IRC_TLS_PORT));
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let config: Config = Figment::new()
            .merge(Toml::string(
                r##"
[database]
url = "postgresql://zeta:db-secret@localhost/zeta_test"

[tracing]
enabled = false

[irc]
nickname = "zeta"
hostname = "localhost"
alt_nicks = []
channels = [{ name = "#zeta", key = "channel-secret" }]
nick_password = "nickserv-secret"
password = "server-secret"
"##,
            ))
            .extract()
            .expect("configuration should parse");

        let debug = format!("{config:?}");

        for secret in [
            "nickserv-secret",
            "server-secret",
            "channel-secret",
            "db-secret",
        ] {
            assert!(!debug.contains(secret), "`{secret}` leaked: {debug}");
        }

        // Non-secret fields stay visible, and the database URL keeps its host.
        assert!(debug.contains("localhost"), "{debug}");
        assert!(debug.contains("#zeta"), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
    }

    #[test]
    fn database_url_password_is_redacted() {
        assert_eq!(
            redact_url_password("postgresql://zeta:secret@localhost/zeta_test").as_ref(),
            "postgresql://zeta:<redacted>@localhost/zeta_test"
        );
        // A URL without a password is unchanged, and a value that is not a URL is hidden.
        assert_eq!(
            redact_url_password("postgresql://zeta@localhost/zeta_test").as_ref(),
            "postgresql://zeta@localhost/zeta_test"
        );
        assert_eq!(redact_url_password("secret").as_ref(), "<redacted>");
    }

    /// The repository `config.toml` must parse into the full configuration: a broken edit has
    /// to fail here rather than at someone's deployment.
    #[test]
    fn repository_config_parses() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config.toml");

        let config = Config::from_figment(&Figment::new().merge(Toml::file(path)))
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
