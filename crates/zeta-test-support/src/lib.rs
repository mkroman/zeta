//! Test utilities shared by the zeta workspace crates.
//!
//! A leaf crate on purpose: the standalone client crates cannot depend on `zeta` — the
//! dependency direction runs the other way — so the helpers here stick to the workspace's
//! external dependencies. Everything that constructs a `zeta` type stays in the consuming
//! crate's own test module instead.
//!
//! The connection-refused helpers use a raw socket rather than wiremock on purpose: a mock
//! server cannot refuse a connection while it is running, and dropping one before connecting
//! would add a lifecycle to the redaction tests without buying any determinism. Wiremock is
//! re-exported for everything else.

use std::net::SocketAddr;
use std::time::Duration;

use url::Url;

pub use wiremock;

/// How long the wiremock server behind [`timeout_error`] waits before answering: long enough
/// that a client with [`TIMEOUT_CLIENT`] cannot possibly get a response.
const LATE_RESPONSE: Duration = Duration::from_secs(5);

/// The client timeout [`timeout_error`] requests with: the response cannot arrive within it.
const TIMEOUT_CLIENT: Duration = Duration::from_millis(50);

/// Returns the address of a just-dropped local listener.
///
/// Connections to the address are refused deterministically, while URLs built against it can
/// still carry credential-looking queries.
///
/// # Panics
///
/// Panics if the listener cannot be bound or its address cannot be read.
#[must_use]
pub fn refused_address() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    drop(listener);
    address
}

/// Sends a GET with the given query string to a just-dropped local listener.
///
/// Returns the transport error and the address: a real connection failure whose URL still
/// carries a credential-looking query, for testing the wrappers' redaction guarantees.
///
/// # Panics
///
/// Panics if the request does not fail, or if reqwest does not attach the request URL to the
/// error.
pub async fn refused_get(query: &str) -> (reqwest::Error, SocketAddr) {
    let address = refused_address();

    let error = reqwest::Client::new()
        .get(format!("http://{address}/?{query}"))
        .send()
        .await
        .expect_err("nothing listens on that address");

    // Sanity: reqwest attaches the request URL to the error — redacting it is on us.
    assert!(error.url().is_some(), "reqwest attaches the request url");

    (error, address)
}

/// Sends a GET with a credential-looking query against a wiremock server that answers only
/// after the request's client timeout has expired, returning the timeout error and the address.
///
/// # Panics
///
/// Panics if the request fails with anything but a timeout.
pub async fn timeout_error() -> (reqwest::Error, SocketAddr) {
    let server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(200).set_delay(LATE_RESPONSE))
        .mount(&server)
        .await;

    let client = reqwest::Client::builder()
        .timeout(TIMEOUT_CLIENT)
        .build()
        .expect("build the http client");

    let error = client
        .get(format!("http://{}?key=secret", server.address()))
        .send()
        .await
        .expect_err("the request should hit the client timeout");

    assert!(error.is_timeout(), "expected a timeout, got: {error}");

    (error, *server.address())
}

/// Asserts the redaction guarantees a connection-refused wrapper error makes.
///
/// No rendering leaks the query string; the short form (what a channel reply gets) is
/// classified, host-only and bounded; the full rendering (what a log line gets) carries the
/// redacted URL.
///
/// # Panics
///
/// Panics on the first broken guarantee.
pub fn assert_redactions(message: &str, full: &str, debug: &str, address: &SocketAddr) {
    assert!(!message.contains("secret"), "{message}");
    assert!(message.starts_with("could not connect"), "{message}");
    assert!(message.contains(&address.ip().to_string()), "{message}");
    assert!(message.len() < 64, "{message}");

    assert!(!full.contains("secret"), "{full}");
    assert!(full.contains(&format!("http://{address}/")), "{full}");
    assert!(full.contains("refused"), "{full}");

    assert!(!debug.contains("secret"), "{debug}");
}

/// The timeout counterpart of [`assert_redactions`].
///
/// The timeout short form instead of the connection one, and no cause-chain word to lean on.
///
/// # Panics
///
/// Panics on the first broken guarantee.
pub fn assert_timeout_redactions(message: &str, full: &str, debug: &str, address: &SocketAddr) {
    assert!(!message.contains("secret"), "{message}");
    assert!(message.starts_with("request timed out"), "{message}");
    assert!(message.contains(&address.ip().to_string()), "{message}");
    assert!(message.len() < 64, "{message}");

    assert!(!full.contains("secret"), "{full}");
    assert!(full.contains(&format!("http://{address}/")), "{full}");

    assert!(!debug.contains("secret"), "{debug}");
}

/// Starts a wiremock server answering `404 Not Found` to every request.
///
/// An S3-compatible endpoint where the object check reports the object as absent and the upload
/// then fails with the non-retryable status.
pub async fn not_found_server() -> wiremock::MockServer {
    let server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(404))
        .mount(&server)
        .await;

    server
}

/// Asserts that `parse` classifies every `input` of `cases` as its expected result.
///
/// Names the input when an assertion fails.
///
/// # Panics
///
/// Panics if a case's URL cannot be parsed or an expectation fails.
pub fn assert_parses<T: std::fmt::Debug + PartialEq>(
    parse: impl Fn(&Url) -> T,
    cases: &[(&str, T)],
) {
    for (input, expected) in cases {
        let url = Url::parse(input).unwrap();

        assert_eq!(&parse(&url), expected, "for {input}");
    }
}

/// Generates the three settings tests shared by every plugin with a configuration section.
///
/// Expands to a `default_settings` test asserting the values produced by [`Default`], a
/// `settings_deserialize` test deserializing the given JSON object before asserting on it, and
/// a `settings_ignore_unknown_keys` test pinning that unknown keys are ignored instead of
/// rejected — removing a setting must never crash deployments that still name it.
#[macro_export]
macro_rules! settings_tests {
    (
        $ty:ty, $settings:ident,
        default: { $($default:tt)* }
        deserialize: { $($json:tt)* } assert: { $($assert:tt)* }
    ) => {
        #[test]
        fn default_settings() {
            let $settings = <$ty>::default();

            $($default)*
        }

        #[test]
        fn settings_deserialize() {
            let $settings: $ty = serde_json::from_value(serde_json::json!({ $($json)* }))
                .expect("could not deserialize settings");

            $($assert)*
        }

        #[test]
        fn settings_ignore_unknown_keys() {
            let mut value = serde_json::json!({ $($json)* });
            value
                .as_object_mut()
                .expect("the settings fixture should be an object")
                .insert("setting_removed_in_this_version".to_owned(), serde_json::Value::Bool(true));

            serde_json::from_value::<$ty>(value)
                .expect("unknown settings keys must be ignored, never rejected");
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refused_get_fails_to_connect_with_the_query_attached() {
        let (error, address) = refused_get("login_token=secret").await;

        assert!(error.is_connect(), "{error}");
        assert!(
            error
                .url()
                .unwrap()
                .as_str()
                .ends_with("?login_token=secret"),
            "{}",
            error.url().unwrap()
        );
        assert_eq!(address.ip().to_string(), "127.0.0.1");
    }

    #[tokio::test]
    async fn timeout_error_times_out_with_the_query_attached() {
        let (error, address) = timeout_error().await;

        assert!(error.is_timeout(), "{error}");
        assert!(error.url().unwrap().as_str().ends_with("?key=secret"));
        assert_eq!(address.ip().to_string(), "127.0.0.1");
    }

    #[tokio::test]
    async fn not_found_server_answers_every_request_with_a_404() {
        let server = not_found_server().await;

        for method in [
            reqwest::Method::GET,
            reqwest::Method::HEAD,
            reqwest::Method::PUT,
        ] {
            let response = reqwest::Client::new()
                .request(
                    method.clone(),
                    format!("http://{}/bucket/key", server.address()),
                )
                .send()
                .await
                .expect("the server responds");

            assert_eq!(
                response.status(),
                reqwest::StatusCode::NOT_FOUND,
                "{method}"
            );
        }

        let requests = server
            .received_requests()
            .await
            .expect("requests are tracked");

        assert_eq!(requests.len(), 3);
    }
}
