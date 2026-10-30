//! Client building for anti-bot-protected sites.
//!
//! These are fetched with [`wreq`] browser emulation: plain `reqwest` gets TLS-fingerprinted by
//! them regardless of headers. Built behind the `emulated` feature, which the anti-bot-protected
//! plugins enable.

use tracing::{Instrument, Span, info_span};
use wreq::header::{ACCEPT_ENCODING, HeaderMap, HeaderValue, InvalidHeaderValue, USER_AGENT};
use zeta_url::RedactedUrl;

use crate::WreqError;

/// The client span of a response, carried in its extensions so a body read stays inside the
/// span that covers the request. The last clone is dropped when the response is consumed.
#[derive(Clone, Debug)]
pub struct ClientSpan(Span);

/// Sends `request` inside an `http.client` span and returns the response only when the server
/// answered with a success status.
///
/// The span carries the OpenTelemetry client attributes — `otel.kind`, `http.request.method`,
/// `url.full` (redacted), `server.address` and `server.port` — plus
/// `http.response.status_code` once a response exists and `error.type` when the send fails. Its
/// exported name is the request method, the HTTP semconv fallback for a URL whose route is not
/// static; `http.client` remains the tracing name because span names must be static.
///
/// # Errors
///
/// Returns [`ResponseError::Request`] if the request fails, or [`ResponseError::Status`] when
/// the response status is not a success.
pub async fn send(request: wreq::RequestBuilder) -> Result<wreq::Response, ResponseError> {
    let (client, request) = request.build_split();
    let request = request?;
    let method = request.method().clone();
    let uri = request.uri().clone();
    let port = uri.port_u16().or_else(|| match uri.scheme_str() {
        Some("https") => Some(443),
        Some("http") => Some(80),
        _ => None,
    });

    let span = info_span!(
        "http.client",
        otel.kind = "client",
        otel.name = %method,
        http.request.method = %method,
        url.full = %RedactedUrl::from_unparsed(&uri.to_string()),
        server.address = %uri.host().unwrap_or_default(),
        server.port = port,
        http.response.status_code = tracing::field::Empty,
        error.type = tracing::field::Empty,
    );

    match client.execute(request).instrument(span.clone()).await {
        Ok(mut response) => {
            let status = response.status();

            span.record("http.response.status_code", status.as_u16());

            if status.is_success() {
                response.extensions_mut().insert(ClientSpan(span));

                Ok(response)
            } else {
                span.record("error.type", status.as_u16().to_string().as_str());

                Err(ResponseError::Status(status))
            }
        }
        Err(error) => {
            let error = WreqError::from(error);

            span.record("error.type", error.error_type());

            Err(error.into())
        }
    }
}

/// Sends `request` and reads the body of the successful response as text.
///
/// # Errors
///
/// Returns a [`ResponseError`] if the request fails, the response status is not a success, or
/// the body cannot be read.
pub async fn text(request: wreq::RequestBuilder) -> Result<String, ResponseError> {
    let response = send(request).await?;
    let span = response
        .extensions()
        .get::<ClientSpan>()
        .map_or_else(Span::none, |client| client.0.clone());

    async move {
        response.text().await.map_err(|error| {
            let error = ResponseError::from(error);

            if let ResponseError::Request(error) = &error {
                Span::current().record("error.type", error.error_type());
            }

            error
        })
    }
    .instrument(span)
    .await
}

/// A failed request through the browser-emulated client.
///
/// The transport variant wraps [`WreqError`], which redacts the request URI: these pages can
/// be fetched through a metadata proxy whose URL carries a credential in its query.
#[derive(Debug, thiserror::Error)]
pub enum ResponseError {
    /// The request failed, or its body could not be read.
    #[error("request error: {0}")]
    Request(#[from] WreqError),
    /// The server responded with an unsuccessful status code.
    #[error("request failed with status {0}")]
    Status(wreq::StatusCode),
}

impl From<wreq::Error> for ResponseError {
    fn from(error: wreq::Error) -> Self {
        Self::Request(error.into())
    }
}

/// Returns a `wreq` client builder emulating Firefox 142, layered with the headers the
/// emulation must not omit.
///
/// `Accept-Encoding` must be set explicitly: like reqwest, wreq does not advertise the
/// header itself even though it decompresses responses — and its absence is enough to get
/// flagged.
///
/// The profile's user agent is overridden with `user_agent` when given. This skews with the
/// emulated Firefox 142 fingerprint, and that is deliberate: the profile defaults to macOS —
/// which anti-bot systems score far more aggressively when requests originate from
/// datacenter networks — and the matching Linux Firefox 142 user agent is rejected by
/// DataDome outright, while Firefox 151 passes.
///
/// # Errors
///
/// Returns an error if `user_agent` cannot be used as a header value.
pub fn builder(user_agent: Option<&str>) -> Result<wreq::ClientBuilder, InvalidHeaderValue> {
    let mut headers = HeaderMap::new();

    headers.insert(
        ACCEPT_ENCODING,
        HeaderValue::from_static("gzip, deflate, br, zstd"),
    );

    if let Some(user_agent) = user_agent {
        let value = HeaderValue::from_str(user_agent)?;

        headers.insert(USER_AGENT, value);
    }

    Ok(wreq::Client::builder()
        .emulation(wreq_util::Emulation::Firefox142)
        .default_headers(headers))
}

/// Tests for the `wreq` wrapper: `emulated`-only builds (the titles plugin) reach them without
/// the `http` feature that gates the `send` and `parse_response` tests.
#[cfg(test)]
mod tests {
    use zeta_test_support::wiremock::{Mock, MockServer, ResponseTemplate};
    use zeta_test_support::{assert_redactions, refused_address};

    use super::{ResponseError, send, text};

    /// Builds the browser-emulated client the helpers are exercised with.
    fn client() -> wreq::Client {
        super::builder(None)
            .expect("the default emulation builds")
            .build()
            .expect("the emulated client builds")
    }

    #[tokio::test]
    async fn text_returns_the_body_of_a_successful_response() {
        let server = MockServer::start().await;

        Mock::given(zeta_test_support::wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>hello</html>"))
            .mount(&server)
            .await;

        let body = text(client().get(server.uri())).await.unwrap();

        assert_eq!(body, "<html>hello</html>");
    }

    #[tokio::test]
    async fn send_reports_error_statuses() {
        let server = MockServer::start().await;

        Mock::given(zeta_test_support::wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let error = send(client().get(server.uri()))
            .await
            .expect_err("a 404 must not be returned as a response");

        assert!(
            matches!(error, ResponseError::Status(status) if status == wreq::StatusCode::NOT_FOUND),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn emulated_request_errors_redact_the_uri_but_keep_the_cause() {
        let address = refused_address();
        let error = wreq::Client::builder()
            .build()
            .expect("wreq client")
            .get(format!("http://{address}/?key=secret"))
            .send()
            .await
            .expect_err("nothing listens on that address");

        // Sanity: wreq attaches the request URI to the error — redacting it is on us.
        assert!(error.uri().is_some(), "wreq attaches the request uri");

        let error = crate::WreqError::from(error);
        let full = error.full();

        assert_redactions(&error.to_string(), full, &format!("{error:?}"), &address);

        // `wreq` renders its first source itself; walking the chain again would print that
        // link twice.
        assert_eq!(full.matches("client error").count(), 1, "{full}");

        assert!(std::error::Error::source(&error).is_none(), "{full}");
    }
}
