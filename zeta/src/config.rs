use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::consts::{DEFAULT_DB_IDLE_TIMEOUT, DEFAULT_MAX_DB_CONNECTIONS};

/// Main application configuration structure.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    /// Database configuration
    pub database: DbConfig,
    /// Tracing configuration
    pub tracing: TracingConfig,
    /// IRC client configuration
    pub irc: IrcConfig,
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

/// DNS resolution configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DnsConfig {
    /// Number of records the cache can hold
    pub cache_size: Option<usize>,
    /// Number of retries after lookup failure before giving up
    pub attempts: Option<usize>,
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
    /// The shared key to access the channel.
    pub key: Option<String>,
}

/// TLS configuration for IRC connection.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct IrcTlsConfig {
    /// Enable TLS.
    pub enabled: bool,
}

/// IRC client configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct IrcConfig {
    /// The client's nickname.
    pub nickname: String,
    /// Alternative nicknames for the client, if the default is taken.
    pub alt_nicks: Vec<String>,
    /// The client's username.
    pub username: Option<String>,
    /// The client's real name.
    pub realname: Option<String>,
    /// The hostname of the server to connect to.
    pub hostname: String,
    /// The password to connect to the server.
    pub password: Option<String>,
    /// The port number of the server to connect to.
    pub port: Option<u16>,
    /// TLS configuration.
    pub tls: Option<IrcTlsConfig>,
    /// List of channels to automatically manage.
    pub channels: HashMap<String, Option<IrcChannelConfig>>,
}

impl IrcConfig {
    /// Returns the port number to use for this IRC connection.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port.map_or_else(|| self.fallback_port(), |port| port)
    }

    /// Returns whether TLS is enabled for this IRC connection.
    fn is_tls_enabled(&self) -> bool {
        self.tls.as_ref().map(|x| x.enabled) == Some(true)
    }

    /// Return the port number to use based on whether the connection requires TLS or not.
    fn fallback_port(&self) -> u16 {
        if self.is_tls_enabled() { 6697 } else { 6667 }
    }
}

impl From<IrcConfig> for irc::client::data::Config {
    fn from(config: IrcConfig) -> Self {
        let port = config.port();
        let channels = config.channels.into_keys().collect::<Vec<_>>();
        let use_tls = config.tls.map(|x| x.enabled);

        Self {
            nickname: Some(config.nickname),
            server: Some(config.hostname),
            port: Some(port),
            use_tls,
            channels,
            alt_nicks: config.alt_nicks,
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
