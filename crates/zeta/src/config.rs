//! Configuration loading and validation.
//!
//! [`Config`] is the host's complete configuration: the database, tracing, IRC, HTTP and mirror
//! sections, plus the per-plugin sections that the host hands to the plugins at startup. The
//! module is split by section: `database` holds the connection pool settings behind
//! [`DbConfig`](crate::config::DbConfig), `tracing` the telemetry toggle behind
//! [`TracingConfig`](crate::config::TracingConfig), and `irc` the client settings behind
//! [`IrcConfig`](crate::config::IrcConfig) and their conversion to the `irc` crate's
//! configuration. The HTTP and mirror sections are owned by the `zeta-http` and `zeta-mirror`
//! crates and re-exported here.
//!
//! Loading merges the configuration file with `ZETA_`-prefixed environment overrides and
//! validates the HTTP user agent before returning.

mod database;
mod irc;
#[cfg(test)]
mod tests;
mod tracing;

use figment::{
    Figment,
    providers::{Env, Format, Toml},
};
use miette::{IntoDiagnostic, WrapErr};
use serde::Deserialize;
use zeta_plugin::Hostmasks;

pub use self::database::DbConfig;
pub use self::irc::{IrcChannelConfig, IrcConfig, IrcTlsConfig};
pub use self::tracing::TracingConfig;
pub use zeta_http::HttpConfig;
#[cfg(feature = "mirror")]
pub use zeta_mirror::MirrorConfig;

/// The plugin-visible subset of the configuration, as carried on the plugins' context.
impl From<&Config> for zeta_plugin::config::HostConfig {
    fn from(config: &Config) -> Self {
        // Compiling the admin hostmasks here, once at startup, keeps every plugin that
        // authorizes administrative commands from compiling the same list again — and turns the
        // no-hostmasks warning into a single startup line instead of one per plugin.
        let admins = Hostmasks::compile(&config.irc.admin_hostmasks, "run admin commands");

        Self::new(admins).with_http(config.http.clone())
    }
}

/// Main application configuration structure.
#[derive(Clone, Debug, Deserialize)]
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
