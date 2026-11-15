use std::error::Error as StdError;

use thiserror::Error;

/// A boxed, trait-object error for plugin initialization.
///
/// Used internally in plugin `new()` methods to allow heterogeneous errors
/// before wrapping into [`Error::Plugin`].
pub type BoxError = Box<dyn StdError + Send + Sync>;

/// An error that occurred during plugin activity.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    /// An IRC protocol or communication error.
    #[error("IRC error: {0}")]
    Irc(#[from] irc::error::Error),
    /// A plugin-specific error, boxed from the plugin's own error type.
    #[error("Plugin error: {0}")]
    Plugin(#[from] BoxError),
    /// A plugin's `[plugins.<name>]` section did not deserialize into its settings type.
    ///
    /// [`Display`](std::fmt::Display) is reply-grade and names the plugin; the deserialization
    /// detail, including the path to the offending value, stays in [`source`](StdError::source).
    #[error("invalid settings for plugin `{plugin}`")]
    InvalidSettings {
        /// The name of the plugin whose settings are invalid.
        plugin: &'static str,
        /// The deserialization error.
        #[source]
        source: Box<figment::Error>,
    },
    /// A required secret was neither configured nor set in its environment variable.
    ///
    /// [`Display`](std::fmt::Display) is reply-grade and names the setting to configure; it
    /// never carries a secret value.
    #[error("the `{name}` setting is not configured")]
    MissingSecret {
        /// The name of the environment variable (or the plugin setting) the secret is read from.
        name: String,
    },
}

/// Wraps any error into [`Error::Plugin`].
///
/// Accepts errors that implement [`StdError + Send + Sync`] and boxes them
/// into a [`BoxError`], then wraps into [`Error::Plugin`].
#[must_use]
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
/// Returns [`Error::MissingSecret`] if neither the configured value nor the environment variable
/// is set. The error names the setting to configure, never the secret.
pub fn resolve_secret(value: Option<&str>, name: &str) -> Result<String, Error> {
    zeta_utils::settings::resolve_optional_setting(value, name).ok_or_else(|| {
        Error::MissingSecret {
            name: name.to_owned(),
        }
    })
}

#[cfg(test)]
mod tests {
    use serde::de::Error as _;

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

        assert_eq!(
            error.to_string(),
            format!("the `{UNSET}` setting is not configured")
        );
    }

    #[test]
    fn empty_secret_without_environment_is_an_error() {
        let error = resolve_secret(Some(""), UNSET).expect_err("missing secret");

        assert_eq!(
            error.to_string(),
            format!("the `{UNSET}` setting is not configured")
        );
    }

    /// The reply text a channel may see per variant, the internal text that must stay out of it,
    /// and the operator detail the log recovers through [`Chain`](crate::Chain).
    #[test]
    fn error_display_is_reply_grade_and_the_chain_keeps_the_detail() {
        const DETAIL: &str = "invalid type: string \"not a number\", expected u32";

        let cases = [
            (
                Error::InvalidSettings {
                    plugin: "test_plugin",
                    source: Box::new(figment::Error::custom(DETAIL)),
                },
                "invalid settings for plugin `test_plugin`",
                Some(DETAIL),
            ),
            (
                Error::MissingSecret {
                    name: UNSET.to_owned(),
                },
                "the `ZETA_TEST_RESOLVE_SECRET_UNSET` setting is not configured",
                None,
            ),
        ];

        for (error, reply, detail) in cases {
            // A channel may see `Display`: it names the failure without the source text.
            assert_eq!(error.to_string(), reply);

            let Some(detail) = detail else {
                continue;
            };

            assert!(
                !reply.contains(detail),
                "the reply must not carry the source text: {reply}"
            );

            // The operator log recovers the detail through the chain.
            let chain = crate::Chain(&error).to_string();
            assert!(chain.contains(detail), "{chain}");
        }
    }
}
