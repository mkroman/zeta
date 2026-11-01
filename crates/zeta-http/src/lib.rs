//! HTTP client infrastructure for the zeta IRC bot and its plugins.
//!
//! Two independent clients live here: the plain `reqwest` client shared by the API plugins
//! (behind the `http` feature) and the browser-emulated `wreq` client for anti-bot-protected
//! sites (behind the `emulated` feature), together with the [`HttpConfig`] both are built from,
//! the OAuth2 client-credentials token cache and the transport-error wrappers that keep
//! credentials out of formatted output.
//!
//! The plain client is split over two private modules: `client` sends requests and reads raw
//! bodies, `response` classifies statuses and parses JSON or text. Their items are re-exported
//! at the crate root.

#[cfg(any(feature = "request", feature = "http"))]
mod client;
mod config;
mod error;
#[cfg(feature = "http")]
mod response;
#[cfg(feature = "emulated")]
mod uri;

/// A boxed, `Send` future: the return type of the outlined HTTP state machines.
///
/// `send` and `read_success` are type-erased so that their coroutine polls are compiled once
/// instead of once per caller; passing the future through a `Box` keeps it out of the caller's
/// generic instantiation.
#[cfg(any(feature = "request", feature = "http"))]
pub(crate) type BoxFuture<'a, T> = std::pin::Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[cfg(feature = "emulated")]
pub mod emulated;
#[cfg(feature = "http")]
pub mod json;
#[cfg(feature = "oauth")]
pub mod oauth;

pub use config::HttpConfig;
#[cfg(any(feature = "request", feature = "http"))]
pub use error::RequestError;
#[cfg(feature = "emulated")]
pub use error::WreqError;
#[cfg(feature = "emulated")]
pub(crate) use uri::RedactedUriRef;

#[cfg(any(feature = "request", feature = "http"))]
pub use client::send;
#[cfg(feature = "http")]
pub use client::{build_client, builder};
#[cfg(feature = "http")]
pub use response::{
    ApiError, get_json, get_json_or_404, get_text, parse_response, parse_response_or_404,
    parse_text,
};

/// Unit tests for the transport-error wrappers.
///
/// The `http.client` span and log tests live in `tests/client_spans.rs`: `tracing` caches callsite
/// interest for the whole process, so a callsite first used on a thread without a subscriber is
/// disabled for every later test. Keeping those tests in their own binary, where every test
/// installs its subscriber before touching the client, keeps the callsite enabled without a global
/// subscriber.
#[cfg(all(test, feature = "http"))]
mod tests {
    use super::*;
    use zeta_test_support::{assert_redactions, assert_timeout_redactions};

    #[tokio::test]
    async fn request_errors_redact_the_url_but_keep_the_cause() {
        let (error, address) = zeta_test_support::refused_get("key=secret").await;

        let error = RequestError::from(error);

        assert_redactions(
            &error.to_string(),
            error.full(),
            &format!("{error:?}"),
            &address,
            "key=secret",
        );

        // The chain ends at the wrapper, so nothing that walks `source()` can reach a raw error.
        assert!(std::error::Error::source(&error).is_none(), "{error:?}");

        let error = ApiError::from(error);

        assert!(!error.to_string().contains("secret"), "{error}");
        assert!(!format!("{error:?}").contains("secret"), "{error:?}");
    }

    #[tokio::test]
    async fn request_timeout_errors_classify_and_redact() {
        let (error, address) = zeta_test_support::timeout_error().await;

        let error = RequestError::from(error);

        assert_timeout_redactions(
            &error.to_string(),
            error.full(),
            &format!("{error:?}"),
            &address,
            "key=secret",
        );

        assert_eq!(error.error_type(), "timeout");
    }
}
