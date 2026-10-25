use std::error::Error as StdError;

use thiserror::Error;

/// A boxed, trait-object error for plugin initialization.
///
/// Used internally in plugin `new()` methods to allow heterogeneous errors
/// before wrapping into [`Error::Plugin`].
pub type BoxError = Box<dyn StdError + Send + Sync>;

/// An error that occurred during plugin activity.
#[derive(Error, Debug)]
pub enum Error {
    /// An IRC protocol or communication error.
    #[error("IRC error: {0}")]
    Irc(#[from] irc::error::Error),
    /// A plugin-specific error, boxed from the plugin's own error type.
    #[error("Plugin error: {0}")]
    Plugin(#[from] BoxError),
}

/// Wraps any error into [`Error::Plugin`].
///
/// Accepts errors that implement [`StdError + Send + Sync`] and boxes them
/// into a [`BoxError`], then wraps into [`Error::Plugin`].
pub fn plugin_err<E: StdError + Send + Sync + 'static>(e: E) -> Error {
    Error::Plugin(Box::new(e))
}

/// Resolves a secret from an optional configured value, falling back to an environment variable.
///
/// The configured value takes precedence when it is set and non-empty; otherwise the value of the
/// `name` environment variable is used. Empty values count as unset on both sides, so a blank
/// configured value or environment variable never becomes the secret. This lets users keep
/// secrets out of `config.toml` (for example through a secret manager) while still allowing them
/// to be configured directly.
///
/// # Errors
///
/// Returns [`Error::Plugin`] if neither the configured value nor the environment variable is set.
/// The error message includes the variable name.
pub fn resolve_secret(value: Option<&str>, name: &str) -> Result<String, Error> {
    zeta_utils::settings::resolve_optional_setting(value, name).ok_or_else(|| {
        Error::Plugin(Box::new(std::io::Error::other(format!(
            "neither the plugin setting nor environment variable `{name}` is set"
        ))))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment variable name that tests can rely on not being set.
    const UNSET: &str = "ZETA_TEST_RESOLVE_SECRET_UNSET";

    #[test]
    fn configured_secret_takes_precedence() {
        let secret = resolve_secret(Some("configured"), UNSET).expect("configured secret");

        assert_eq!(secret, "configured");
    }

    #[test]
    fn missing_secret_is_an_error() {
        let error = resolve_secret(None, UNSET).expect_err("missing secret");

        assert!(error.to_string().contains(UNSET), "{error}");
    }

    #[test]
    fn empty_secret_without_environment_is_an_error() {
        let error = resolve_secret(Some(""), UNSET).expect_err("missing secret");

        assert!(error.to_string().contains(UNSET), "{error}");
    }
}
