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
#[cfg(feature = "uri")]
mod uri;

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
#[cfg(feature = "uri")]
pub use uri::RedactedUriRef;

#[cfg(any(feature = "request", feature = "http"))]
pub use client::send;
#[cfg(feature = "http")]
pub use client::{build_client, builder};
#[cfg(feature = "http")]
pub use response::{
    ApiError, get_json, get_json_or_404, get_text, parse_response, parse_response_or_404,
    parse_text,
};

#[cfg(all(test, feature = "http"))]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use tracing::Instrument;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id};
    use tracing_subscriber::layer::{Context as LayerContext, Layer};
    use tracing_subscriber::prelude::*;

    use super::*;
    use zeta_test_support::wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};
    use zeta_test_support::{assert_redactions, assert_timeout_redactions, refused_address};

    /// A writer that collects everything logged through it, for assertions.
    #[derive(Clone, Default)]
    struct LogBuffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Captures tracing output emitted while the returned guard is alive.
    fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(buffer.clone()),
        );

        (buffer, tracing::subscriber::set_default(subscriber))
    }

    /// One span as the recorder observed it: its name, its fields and when it closed.
    #[derive(Clone, Debug)]
    struct RecordedSpan {
        /// The span's id, used to match field updates and the close to the span.
        id: Id,
        /// The span's name.
        name: &'static str,
        /// The span that was current when this one was created.
        parent: Option<Id>,
        /// Every field with its last recorded value, rendered as text.
        fields: BTreeMap<String, String>,
        /// When the span was created.
        opened: Instant,
        /// When the span was dropped, if it has been.
        closed: Option<Instant>,
    }

    impl RecordedSpan {
        /// Returns the rendered value of `field`, or `None` when it was never recorded.
        fn field(&self, field: &str) -> Option<&str> {
            self.fields.get(field).map(String::as_str)
        }

        /// Returns how long the span was open, or `None` when it has not closed.
        fn duration(&self) -> Option<Duration> {
            self.closed
                .map(|closed| closed.saturating_duration_since(self.opened))
        }
    }

    /// Collects every span created while it is the active subscriber's layer, so a test can
    /// assert on the attributes of a span that logs no event of its own.
    #[derive(Clone, Default)]
    struct SpanRecorder(Arc<Mutex<Vec<RecordedSpan>>>);

    impl SpanRecorder {
        /// Returns the spans named `name`, in creation order.
        fn named(&self, name: &str) -> Vec<RecordedSpan> {
            self.0
                .lock()
                .expect("span recorder lock")
                .iter()
                .filter(|span| span.name == name)
                .cloned()
                .collect()
        }
    }

    /// Renders a span's fields as text: strings raw, integers as written, everything else
    /// through `Debug` (`%` display values delegate to it).
    struct FieldVisitor<'a>(&'a mut BTreeMap<String, String>);

    impl Visit for FieldVisitor<'_> {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().to_owned(), value.to_owned());
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.insert(field.name().to_owned(), value.to_string());
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().to_owned(), format!("{value:?}"));
        }
    }

    impl<S> Layer<S> for SpanRecorder
    where
        S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: LayerContext<'_, S>) {
            let mut fields = BTreeMap::new();

            attrs.record(&mut FieldVisitor(&mut fields));

            let parent = if attrs.is_contextual() {
                ctx.lookup_current().map(|span| span.id())
            } else {
                None
            };

            self.0
                .lock()
                .expect("span recorder lock")
                .push(RecordedSpan {
                    id: id.clone(),
                    name: attrs.metadata().name(),
                    parent,
                    fields,
                    opened: Instant::now(),
                    closed: None,
                });
        }

        fn on_record(&self, id: &Id, values: &tracing::span::Record<'_>, _: LayerContext<'_, S>) {
            let mut spans = self.0.lock().expect("span recorder lock");

            if let Some(span) = spans.iter_mut().find(|span| span.id == *id) {
                values.record(&mut FieldVisitor(&mut span.fields));
            }
        }

        fn on_close(&self, id: Id, _: LayerContext<'_, S>) {
            let mut spans = self.0.lock().expect("span recorder lock");

            if let Some(span) = spans.iter_mut().find(|span| span.id == id) {
                span.closed = Some(Instant::now());
            }
        }
    }

    #[tokio::test]
    async fn request_errors_redact_the_url_but_keep_the_cause() {
        let (error, address) = zeta_test_support::refused_get("key=secret").await;

        let error = RequestError::from(error);

        assert_redactions(
            &error.to_string(),
            error.full(),
            &format!("{error:?}"),
            &address,
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
        );

        assert_eq!(error.error_type(), "timeout");
    }

    #[tokio::test]
    async fn send_logs_a_query_free_url_and_returns_a_redacted_error() {
        let address = refused_address();
        let request_url = format!("http://{address}/?key=secret");

        let (buffer, guard) = capture_logs();

        let error = send(reqwest::Client::new().get(&request_url))
            .await
            .expect_err("nothing listens on that address");
        drop(guard);

        let logged = String::from_utf8(buffer.0.lock().expect("log buffer lock").clone())
            .expect("log is utf-8");

        let query_free = format!("http://{address}/");
        assert!(logged.contains("url.full"), "{logged}");
        assert!(logged.contains(&query_free), "{logged}");
        assert!(!logged.contains("key=secret"), "{logged}");

        // The boundary line carries the classified failure, the redacted URL and the cause.
        assert!(logged.contains("error.type"), "{logged}");
        assert!(logged.contains("connection_error"), "{logged}");
        assert!(logged.contains("refused"), "{logged}");

        assert!(!error.to_string().contains("secret"), "{error}");
        assert!(!error.full().contains("secret"), "{error}");
        assert!(!format!("{error:?}").contains("secret"), "{error:?}");
    }

    #[tokio::test]
    async fn parse_response_logs_the_status_even_when_the_body_read_fails() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");

        std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};

            let (mut stream, _) = listener.accept().expect("accept");

            // Drain the request before replying: closing a socket with unread bytes in its
            // receive buffer sends an RST instead of a FIN, which would race with — and
            // sometimes destroy — the response written below.
            let mut request = Vec::new();
            let mut chunk = [0_u8; 512];

            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).expect("read");

                if read == 0 {
                    break;
                }

                request.extend_from_slice(&chunk[..read]);
            }

            // Promise more bytes than are sent, then hang up: the body read fails mid-response.
            let response =
                "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 1024\r\n\r\nshort";
            stream.write_all(response.as_bytes()).expect("write");
        });

        let (buffer, guard) = capture_logs();

        let response = reqwest::Client::new()
            .get(format!("http://{address}/?key=secret"))
            .send()
            .await
            .expect("response");
        let error = parse_response::<serde_json::Value>(response)
            .await
            .expect_err("the body read must fail");
        drop(guard);

        assert!(matches!(error, ApiError::Request(_)), "{error:?}");

        let logged = String::from_utf8(buffer.0.lock().expect("log buffer lock").clone())
            .expect("log is utf-8");

        // The status event fires before the body read gives up...
        assert!(
            logged.contains("http response carries an error status"),
            "{logged}"
        );
        // ...`error.type` is recorded on the span before that event, so it is in the same line...
        assert!(logged.contains("error.type"), "{logged}");
        // ...and no credential from the query reaches the log.
        assert!(!logged.contains("key=secret"), "{logged}");
    }

    #[tokio::test]
    async fn get_text_records_the_response_attributes_on_the_client_span() {
        let server = MockServer::start().await;

        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>hello</html>"))
            .mount(&server)
            .await;

        let (buffer, guard) = capture_logs();

        let body = get_text(reqwest::Client::new().get(server.uri()))
            .await
            .expect("200 response");
        drop(guard);

        assert_eq!(body, "<html>hello</html>");

        let logged = String::from_utf8(buffer.0.lock().expect("log buffer lock").clone())
            .expect("log is utf-8");

        // A direct text read runs in the same span as a JSON read, so it carries the response
        // attributes and the success line too.
        assert!(logged.contains("http.client"), "{logged}");
        assert!(logged.contains("url.full"), "{logged}");
        assert!(logged.contains("http.response.status_code"), "{logged}");
        assert!(logged.contains("http response received"), "{logged}");
    }

    #[tokio::test]
    async fn get_json_opens_a_client_span_covering_the_whole_request() {
        let server = MockServer::start().await;

        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"hello":"world"}"#)
                    .set_delay(Duration::from_millis(100)),
            )
            .mount(&server)
            .await;

        let recorder = SpanRecorder::default();
        let subscriber = tracing_subscriber::registry().with(recorder.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        // The client span is created inside the calling operation, so the trace shows the API
        // call under it instead of as a root of its own.
        let operation = tracing::info_span!("test.operation");
        let operation_id = operation
            .id()
            .expect("the operation span should be enabled");

        let value: serde_json::Value = async {
            get_json(reqwest::Client::new().get(format!("{}/endpoint?key=secret", server.uri())))
                .await
        }
        .instrument(operation)
        .await
        .expect("200 response");

        assert_eq!(value["hello"], "world");

        let spans = recorder.named("http.client");
        assert_eq!(spans.len(), 1, "{spans:?}");

        let span = &spans[0];

        assert_eq!(span.name, "http.client");
        assert_eq!(span.parent, Some(operation_id));
        assert_eq!(span.field("otel.kind"), Some("client"));
        assert_eq!(span.field("http.request.method"), Some("GET"));
        assert_eq!(span.field("http.response.status_code"), Some("200"));
        assert_eq!(
            span.field("url.full"),
            Some(format!("{}/endpoint", server.uri()).as_str())
        );
        assert_eq!(span.field("server.address"), Some("127.0.0.1"));
        assert_eq!(
            span.field("server.port"),
            Some(server.address().port().to_string().as_str())
        );
        assert_eq!(span.field("error.type"), None);

        // The span is open while the server holds the response: it covers the request, not only
        // the moment the response headers were handed over.
        assert!(
            span.duration().expect("the client span closed") >= Duration::from_millis(90),
            "{span:?}"
        );

        // Redaction covers the span attributes, not only the log line: the API key travels in
        // the query string.
        assert!(
            !span.fields.values().any(|value| value.contains("secret")),
            "{span:?}"
        );
    }

    #[tokio::test]
    async fn a_refused_connection_still_produces_a_client_span_with_an_error_type() {
        let address = refused_address();

        let recorder = SpanRecorder::default();
        let subscriber = tracing_subscriber::registry().with(recorder.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        let error = send(reqwest::Client::new().get(format!("http://{address}/?key=secret")))
            .await
            .expect_err("nothing listens on that address");

        let spans = recorder.named("http.client");
        assert_eq!(spans.len(), 1, "{spans:?}");

        let span = &spans[0];

        assert_eq!(span.field("otel.kind"), Some("client"));
        assert_eq!(span.field("http.request.method"), Some("GET"));
        assert_eq!(span.field("error.type"), Some("connection_error"));
        assert_eq!(span.field("http.response.status_code"), None);
        assert_eq!(
            span.field("url.full"),
            Some(format!("http://{address}/").as_str())
        );

        // The failed span closes too, so it is exported instead of leaking until the process
        // exits; and the error stays redacted.
        assert!(span.duration().is_some(), "{span:?}");
        assert!(
            !span.fields.values().any(|value| value.contains("secret")),
            "{span:?}"
        );
        assert!(!error.full().contains("secret"), "{error}");
    }
}
