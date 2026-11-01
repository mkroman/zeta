//! The `http.client` span contract of the shared client, exercised in its own test binary.
//!
//! `tracing` caches one interest value per callsite for the whole process, computed from the
//! dispatcher that was current when the callsite was first used. A callsite first used on a thread
//! without a subscriber is cached as disabled, and a later thread-local subscriber cannot repair
//! it. Keeping these recorder tests in a binary of their own — with every test installing its
//! recorder before it touches the client — means nothing else can register the callsite first.

#![cfg(feature = "http")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::Instrument;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context as LayerContext, Layer};
use tracing_subscriber::prelude::*;

use zeta_http::{get_json, send};
use zeta_test_support::wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};

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
        get_json(reqwest::Client::new().get(format!("{}/endpoint?key=secret", server.uri()))).await
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
async fn a_failed_send_still_produces_a_client_span_with_an_error_type() {
    // `ftp` is not an HTTP scheme: the client rejects the request before the connector runs,
    // so this is a real failed send that never opens a socket — nothing can answer the port
    // between the test starting and the request, which is what a dropped listener could not
    // promise. The span wiring is the same for every failure; the connection-specific
    // `error.type` value is the wrapper's, covered by the refused-connection log tests.
    let url = reqwest::Url::parse("ftp://example.invalid/?key=secret").expect("parse the url");

    let recorder = SpanRecorder::default();
    let subscriber = tracing_subscriber::registry().with(recorder.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let error = send(reqwest::Client::new().get(url))
        .await
        .expect_err("the client rejects the scheme");

    let spans = recorder.named("http.client");
    assert_eq!(spans.len(), 1, "{spans:?}");

    let span = &spans[0];

    assert_eq!(span.field("otel.kind"), Some("client"));
    assert_eq!(span.field("http.request.method"), Some("GET"));
    assert_eq!(span.field("error.type"), Some("other"));
    assert_eq!(span.field("http.response.status_code"), None);
    assert_eq!(span.field("url.full"), Some("ftp://example.invalid/"));

    // The failed span closes too, so it is exported instead of leaking until the process
    // exits; and the error stays redacted.
    assert!(span.duration().is_some(), "{span:?}");
    assert!(
        !span.fields.values().any(|value| value.contains("secret")),
        "{span:?}"
    );
    assert!(!error.full().contains("secret"), "{error}");
}
