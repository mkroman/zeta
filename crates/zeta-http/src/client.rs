//! The plain `reqwest` client shared by the API plugins.
//!
//! [`send`] opens one `http.client` span around the request — so name resolution, connect, TLS,
//! the time to first byte and the body read are on the span — logs a failed request once before
//! the error comes back as a [`RequestError`], and hands the response to [`crate::response`]
//! with the span attached. [`builder`]/[`build_client`] build the client from [`HttpConfig`].

#[cfg(feature = "http")]
use tracing::Span;
use tracing::{Instrument, error, info_span};
use zeta_url::{RedactedUrl, RedactedUrlRef};

#[cfg(feature = "http")]
use crate::HttpConfig;
use crate::{BoxFuture, RequestError};

/// The client span of a response, carried in its extensions so the body read and the parse run
/// inside the span the request opened.
///
/// [`send`] cannot close the span when the response headers arrive: the body is read by the
/// caller, and a span that ends there would hide the download. The last clone is dropped when
/// the response is consumed, which closes the span after the body read.
///
/// Only the `http` feature reads the span back (from `response`); with the `request` feature
/// alone it would be dead weight, so it is not compiled there.
#[cfg(feature = "http")]
#[derive(Clone, Debug)]
pub struct ClientSpan(pub(crate) Span);

/// Sends `request` inside an `http.client` span and returns the response.
///
/// The span starts before the request is executed, so it covers name resolution, connect, TLS,
/// the time to first byte and — through the `ClientSpan` stored in the response — the body
/// read and parse. It carries the OpenTelemetry client attributes — `otel.kind`,
/// `http.request.method`, `url.full` (redacted), `server.address` and `server.port` — plus
/// `http.response.status_code` once a response exists and `error.type` when the send fails. It is
/// exported as `http.client`: the method is an attribute, and a method-only name would hide the
/// operation the call belongs to; the caller's span in the trace says what is being fetched.
///
/// A failed send is logged once inside the span — with `url.full` (the request URL without its
/// query, fragment, or userinfo), `error.type`, and the error's full rendering — before the
/// error comes back as a [`RequestError`]: these APIs carry their key in the query string, and
/// the error reaches `Display` (and `Debug`) in plugin and dispatcher logs, while a log line
/// still needs to say which request failed and why.
///
/// # Errors
///
/// Returns the [`RequestError`] of the failed send, with its URL redacted.
///
/// Outlined on purpose: every API plugin reaches this through its own response type, and with
/// LTO the compiler otherwise clones the request-build and span state machine into each caller.
/// The box allocates once per request, which is negligible next to the round trip it awaits.
#[inline(never)]
pub fn send(
    request: reqwest::RequestBuilder,
) -> BoxFuture<'static, Result<reqwest::Response, RequestError>> {
    Box::pin(async move {
        let (client, request) = request.build_split();
        let request = match request {
            Ok(request) => request,
            // A request that cannot be built never reaches the network, so there is nothing for a
            // client span to measure; the log still names the URL when the error carries one.
            Err(error) => return Err(log_request_failure(RequestError::from(error))),
        };
        let method = request.method().clone();

        let span = info_span!(
            "http.client",
            otel.kind = "client",
            http.request.method = %method,
            url.full = %RedactedUrlRef::from_url(request.url()),
            server.address = %request.url().host_str().unwrap_or_default(),
            server.port = request.url().port_or_known_default(),
            http.response.status_code = tracing::field::Empty,
            error.type = tracing::field::Empty,
        );

        match client.execute(request).instrument(span.clone()).await {
            Ok(response) => {
                span.record("http.response.status_code", response.status().as_u16());

                // The body helpers read the span back; without the `http` feature there is no body
                // helper, so the span ends with the response headers.
                #[cfg(feature = "http")]
                let response = attach_span(response, span);

                Ok(response)
            }
            Err(error) => {
                let error = RequestError::from(error);

                span.record("error.type", error.error_type());

                // Logged inside the span: the event marks the span as failed and is exported with
                // its attributes, also when the caller has no enabled span of its own.
                Err(span.in_scope(|| log_request_failure(error)))
            }
        }
    })
}

/// Attaches the client span to `response`, so the body read stays in the span the request
/// opened.
#[cfg(feature = "http")]
fn attach_span(mut response: reqwest::Response, span: Span) -> reqwest::Response {
    response.extensions_mut().insert(ClientSpan(span));

    response
}

/// Logs a failed request once, when the error names its URL, and returns the error.
fn log_request_failure(error: RequestError) -> RequestError {
    if let Some(url) = error.url() {
        log_request_error(&error, url);
    }

    error
}

/// Logs a failed request once, with its redacted URL and error type.
fn log_request_error(error: &RequestError, url: &RedactedUrl) {
    error!(
        url.full = %url,
        error.type = error.error_type(),
        exception.message = %error.full(),
        "request failed"
    );
}

/// Returns a default HTTP client configured by [`HttpConfig`].
///
/// # Panics
///
/// Panics if the default HTTP client fails to build.
#[must_use]
#[cfg(feature = "http")]
pub fn build_client(config: &HttpConfig) -> reqwest::Client {
    builder(config)
        .build()
        .expect("could not build http client")
}

/// Returns a default HTTP client builder configured by [`HttpConfig`].
#[cfg(feature = "http")]
pub fn builder(config: &HttpConfig) -> reqwest::ClientBuilder {
    reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(config.timeout)
        .user_agent(config.user_agent.clone())
}
