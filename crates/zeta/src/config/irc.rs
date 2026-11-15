//! IRC client configuration.
//!
//! [`IrcConfig`] mirrors the `[irc]` section: the connection details, the channels to join, and
//! the TLS toggle. [`IrcConfig::resolved_port`] applies the plaintext or TLS default port when
//! none is configured, and the `From<&IrcConfig>` conversion builds the `irc` crate's client
//! configuration from it.

use std::collections::HashMap;

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::consts;

/// Configuration for an individual IRC channel.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct IrcChannelConfig {
    /// Name of the channel.
    pub name: String,
    /// The shared key to access the channel.
    pub key: Option<SecretString>,
}

/// TLS configuration for IRC connection.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize)]
pub struct IrcTlsConfig {
    /// Toggle TLS.
    pub enabled: bool,
}

/// IRC client configuration.
#[derive(Clone, Debug, Deserialize)]
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
    pub nick_password: Option<SecretString>,
    /// The client's nickname.
    pub nickname: String,
    /// The password to connect to the server.
    pub password: Option<SecretString>,
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
    /// Returns the port number to use for this IRC connection: the configured port, or the TLS
    /// or plaintext port depending on whether TLS is enabled.
    #[must_use]
    pub fn resolved_port(&self) -> u16 {
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
        let port = config.resolved_port();
        // `irc` treats `None` as TLS enabled, which would pair TLS with the plaintext port the
        // omitted `[irc.tls]` section implies; the default must be spelled out explicitly.
        let use_tls = Some(config.tls.as_ref().is_some_and(|tls| tls.enabled));
        let mut channels = Vec::with_capacity(config.channels.len());
        let mut channel_keys = HashMap::new();

        for channel in &config.channels {
            if let Some(key) = &channel.key {
                channel_keys.insert(channel.name.clone(), key.expose_secret().to_owned());
            }

            channels.push(channel.name.clone());
        }

        Self {
            nickname: Some(config.nickname.clone()),
            nick_password: config
                .nick_password
                .as_ref()
                .map(|password| password.expose_secret().to_owned()),
            username: config.username.clone(),
            realname: config.realname.clone(),
            server: Some(config.hostname.clone()),
            port: Some(port),
            password: config
                .password
                .as_ref()
                .map(|password| password.expose_secret().to_owned()),
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

/// Returns the default message sent in the `QUIT` when the bot shuts down.
fn default_shutdown_quit_message() -> String {
    consts::DEFAULT_SHUTDOWN_QUIT_MESSAGE.to_string()
}

#[cfg(test)]
mod tests {
    use figment::{
        Figment,
        providers::{Format, Toml},
    };

    use super::*;

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
                    key: Some("channel-secret".into()),
                },
                IrcChannelConfig {
                    name: "#other".to_string(),
                    key: None,
                },
            ],
            encoding: Some("utf-8".to_string()),
            hostname: "irc.example.com".to_string(),
            nick_password: Some("nickserv-secret".into()),
            nickname: "zeta".to_string(),
            password: Some("server-secret".into()),
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
        assert_eq!(config.resolved_port(), consts::DEFAULT_IRC_PORT);

        let converted = irc::client::data::Config::from(&config);

        assert_eq!(converted.use_tls, Some(false));
        assert_eq!(converted.port, Some(consts::DEFAULT_IRC_PORT));

        let config: IrcConfig = Figment::new()
            .merge(Toml::string(
                "nickname = \"zeta\"\nhostname = \"localhost\"\nalt_nicks = []\nchannels = []\n[ tls ]\nenabled = true\n",
            ))
            .extract()
            .expect("could not parse irc configuration");

        assert_eq!(config.resolved_port(), consts::DEFAULT_IRC_TLS_PORT);

        let converted = irc::client::data::Config::from(&config);

        assert_eq!(converted.use_tls, Some(true));
        assert_eq!(converted.port, Some(consts::DEFAULT_IRC_TLS_PORT));
    }
}
