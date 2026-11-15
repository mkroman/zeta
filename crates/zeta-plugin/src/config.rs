//! Plugin-facing configuration.
//!
//! The host keeps the full configuration; plugins only see the [`HostConfig`] subset carried on
//! their [`Context`](crate::Context). The HTTP configuration lives in `zeta-http`, which owns the
//! shared client builders that consume it.

use figment::value::Value;
use serde::de::DeserializeOwned;
#[cfg(feature = "http")]
use zeta_http::HttpConfig;

use crate::hostmask::Hostmasks;

/// The subset of the host configuration plugins can see through their context.
///
/// This is deliberately narrower than the host's configuration: per-plugin settings are handed
/// to each plugin through its constructor, and host-only sections (database, tracing, the IRC
/// connection, the media mirror) never reach plugin code.
///
/// The host builds this once at startup, so the admin hostmasks of `[irc].admin_hostmasks` are
/// compiled into a single [`Hostmasks`] matcher. Plugins clone that matcher instead of compiling
/// the configured list themselves, which also logs the "no admin hostmasks configured" warning
/// exactly once.
#[derive(Clone, Debug, Default)]
pub struct HostConfig {
    /// HTTP configuration for the shared client
    /// ([`Context::http`](crate::Context::http)) and for plugins that build their own clients.
    #[cfg(feature = "http")]
    pub http: HttpConfig,
    /// The admin hostmasks, compiled once by the host at startup.
    admins: Hostmasks,
}

impl HostConfig {
    /// Builds the plugin-visible configuration from the host's sections.
    ///
    /// `admins` is the matcher the host compiled from `[irc].admin_hostmasks` at startup, so
    /// every plugin matches senders against the same patterns.
    #[must_use]
    pub const fn new(#[cfg(feature = "http")] http: HttpConfig, admins: Hostmasks) -> Self {
        Self {
            #[cfg(feature = "http")]
            http,
            admins,
        }
    }

    /// Returns the compiled admin hostmasks of `[irc].admin_hostmasks`.
    ///
    /// A sender is an admin when its `nick!user@host` matches one of the configured patterns,
    /// where any of the three components may contain wildcards (`*` or `?`), e.g. `mk!mk@*` or
    /// `*!*@example.com`. With no hostmasks configured, nobody is an admin.
    #[must_use]
    pub const fn admins(&self) -> &Hostmasks {
        &self.admins
    }
}

/// Deserializes `value` into `T`, collecting the keys that `T` did not recognize.
///
/// Unknown keys are a warning, never an error: a configuration that still names a setting
/// removed by a newer release must keep working. The caller reports the keys, so a typo or a
/// stale setting is still visible in the logs.
///
/// # Errors
///
/// Returns the deserialization error, including the path to the offending value, if `value` does
/// not deserialize into `T`.
pub(crate) fn deserialize_with_unknown_keys<T: DeserializeOwned>(
    value: &Value,
) -> Result<(T, Vec<String>), Box<figment::Error>> {
    let mut unknown_keys = Vec::new();
    let settings = serde_ignored::deserialize(value, |path| {
        unknown_keys.push(path.to_string());
    })
    .map_err(Box::new)?;

    Ok((settings, unknown_keys))
}
