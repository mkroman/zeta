//! HTTP transport helpers: connection-refused and timeout errors, their redaction assertions,
//! the API-error fixture, and the wiremock servers client tests run against.
//!
//! [`refused_address`] and the redaction assertions are always available: client crates assert
//! on their own error wrappers without needing a server. The request helpers, the API-error
//! fixture, the servers, and the re-exported [`wiremock`] crate need the `http` feature.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
#[cfg(feature = "http")]
use std::time::Duration;

#[cfg(feature = "http")]
pub use wiremock;

/// How long the wiremock server behind [`timeout_error`] waits before answering: long enough
/// that a client with [`TIMEOUT_CLIENT`] cannot possibly get a response.
#[cfg(feature = "http")]
const LATE_RESPONSE: Duration = Duration::from_secs(5);

/// The client timeout [`timeout_error`] requests with: the response cannot arrive within it.
#[cfg(feature = "http")]
const TIMEOUT_CLIENT: Duration = Duration::from_millis(50);

/// Returns the local address whose connections are refused.
///
/// The address names TCP port 0: IANA reserves it ([RFC 6335]), and a zero port in `bind` means
/// "pick an ephemeral port", so no listener can ever occupy it. A connection to it therefore
/// always fails in the connect phase, without this helper holding a socket — while URLs built
/// against the address can still carry credential-looking queries. A fixed nonzero port would
/// only be free by convention and could belong to another test.
///
/// [RFC 6335]: https://www.rfc-editor.org/rfc/rfc6335#section-6
#[must_use]
pub const fn refused_address() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
}

/// Sends a GET with the given query string to [`refused_address`].
///
/// Returns the transport error and the address: a real connection failure whose URL still
/// carries a credential-looking query, for testing the wrappers' redaction guarantees.
///
/// # Panics
///
/// Panics if the request does not fail, or if reqwest does not attach the request URL to the
/// error.
#[cfg(feature = "http")]
pub async fn refused_get(query: &str) -> (reqwest::Error, SocketAddr) {
    let address = refused_address();

    let error = reqwest::Client::new()
        .get(format!("http://{address}/?{query}"))
        .send()
        .await
        .expect_err("port zero has no listener");

    // Sanity: reqwest attaches the request URL to the error — redacting it is on us.
    assert!(error.url().is_some(), "reqwest attaches the request url");

    (error, address)
}

/// Builds a deterministic API failure from a request that never leaves the machine: reqwest
/// cannot resolve the relative URL, so building the request fails before it is sent.
///
/// Reply-mapper tests use it to construct any error `E: From<ApiError>` — usually the plugin's
/// own error — without a server. The rendering is reply-grade: it names the failure, not the
/// URL, and carries no response body.
///
/// # Panics
///
/// Panics if reqwest accepts the relative URL, which would mean the fixture no longer fails
/// deterministically.
#[cfg(feature = "http")]
#[must_use]
pub fn api_error<E>() -> E
where
    E: From<zeta_http::ApiError>,
{
    let error = reqwest::Client::new()
        .get("not a url")
        .build()
        .expect_err("a relative URL cannot be built into a request");

    E::from(zeta_http::ApiError::from(zeta_http::RequestError::from(
        error,
    )))
}

/// Sends a GET with a credential-looking query against a wiremock server that answers only
/// after the request's client timeout has expired, returning the timeout error and the address.
///
/// # Panics
///
/// Panics if the request fails with anything but a timeout.
#[cfg(feature = "http")]
pub async fn timeout_error() -> (reqwest::Error, SocketAddr) {
    // The server is exclusive: the delayed response keeps the request in flight after the
    // client times out and the test returns. A pooled server would be recycled with that late
    // request still on the wire, and the next test to borrow it would record it as its own.
    let server = wiremock::MockServer::builder().start().await;

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
/// No rendering leaks `query`; the short form (what a channel reply gets) is classified,
/// host-only and bounded; the full rendering (what a log line gets) carries the redacted URL.
///
/// # Panics
///
/// Panics on the first broken guarantee.
pub fn assert_redactions(
    message: &str,
    full: &str,
    debug: &str,
    address: &SocketAddr,
    query: &str,
) {
    assert_common(message, full, debug, address, query, "could not connect");

    // The source chain carries hyper's `client error (Connect)` link on every platform. The
    // OS wording below it differs (Linux says "Connection refused", macOS says "Address not
    // available" for the reserved port), so only Linux can assert on it.
    assert!(full.contains("client error (Connect)"), "{full}");

    #[cfg(target_os = "linux")]
    assert!(full.contains("refused"), "{full}");
}

/// The timeout counterpart of [`assert_redactions`].
///
/// The timeout short form instead of the connection one, and no cause-chain word to lean on.
///
/// # Panics
///
/// Panics on the first broken guarantee.
pub fn assert_timeout_redactions(
    message: &str,
    full: &str,
    debug: &str,
    address: &SocketAddr,
    query: &str,
) {
    assert_common(message, full, debug, address, query, "request timed out");
}

/// Asserts the guarantees shared by [`assert_redactions`] and [`assert_timeout_redactions`].
fn assert_common(
    message: &str,
    full: &str,
    debug: &str,
    address: &SocketAddr,
    query: &str,
    short_prefix: &str,
) {
    assert!(!message.contains(query), "{message}");
    assert!(message.starts_with(short_prefix), "{message}");
    assert!(message.contains(&address.ip().to_string()), "{message}");
    assert!(message.len() < 64, "{message}");

    assert!(!full.contains(query), "{full}");
    assert!(full.contains(&format!("http://{address}/")), "{full}");

    assert!(!debug.contains(query), "{debug}");
}

/// Starts a wiremock server answering `404 Not Found` to every request.
///
/// An S3-compatible endpoint where the object check reports the object as absent and the upload
/// then fails with the non-retryable status.
#[cfg(feature = "http")]
pub async fn not_found_server() -> wiremock::MockServer {
    let server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(404))
        .mount(&server)
        .await;

    server
}

#[cfg(all(test, feature = "http"))]
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
        // The URL keeps the reserved port instead of normalizing it away: that is the whole
        // refusal strategy.
        assert!(
            error
                .url()
                .unwrap()
                .as_str()
                .starts_with("http://127.0.0.1:0/"),
            "{}",
            error.url().unwrap()
        );
        assert_eq!(address.ip().to_string(), "127.0.0.1");
        assert_eq!(address.port(), 0);
    }

    #[test]
    fn api_error_is_a_request_failure_without_the_url() {
        let error: zeta_http::ApiError = api_error();
        let message = error.to_string();

        assert!(message.starts_with("request error"), "{message}");
        assert!(!message.contains("not a url"), "{message}");
        assert!(!message.contains("secret"), "{message}");
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
