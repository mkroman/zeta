use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    /// Database configuration
    pub database: DbConfig,
    /// Tracing configuration
    pub tracing: TracingConfig,
    /// IRC client configuration
    pub irc: IrcConfig,
}

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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TracingConfig {
    /// Enable tracing
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct IrcChannelConfig {
    /// The shared key to access the channel.
    pub key: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct IrcTlsConfig {
    /// Enable TLS.
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
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
    #[must_use]
    pub fn port(&self) -> u16 {
        match self.port {
            Some(port) => port,
            None => self.fallback_port(),
        }
    }

    /// Return the port number to use based on whether the connection requires TLS or not.
    fn fallback_port(&self) -> u16 {
        if self.tls.as_ref().map(|tls| tls.enabled) == Some(true) {
            6697
        } else {
            6667
        }
    }
}

impl From<IrcConfig> for irc::client::data::Config {
    fn from(config: IrcConfig) -> Self {
        let port = config.port();
        let channels = config.channels.into_keys().collect::<Vec<_>>().clone();
        let use_tls = config.tls.map(|x| x.enabled);

        irc::client::data::Config {
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

#[must_use]
pub const fn default_max_db_connections() -> u32 {
    crate::database::DEFAULT_MAX_CONNECTIONS
}

#[must_use]
pub const fn default_db_idle_timeout() -> Duration {
    crate::database::DEFAULT_IDLE_TIMEOUT
}
