//! One export-level assertion for the OpenTelemetry span contract.
//!
//! The span tests in `zeta-plugin-runtime` and `zeta` assert on `tracing` fields such as
//! `otel.name` and `otel.kind`. This test runs one real dispatch through
//! `tracing-opentelemetry` and an in-memory exporter, so the layer's name override, kind
//! parsing and parent mapping are pinned once instead of trusted. Keep it to one test: the
//! point is to validate the layer boundary, not to repeat every attribute assertion.

use irc::proto::Message;
use opentelemetry::trace::{SpanKind, TraceContextExt, TracerProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use tokio::sync::mpsc;
use tracing::info_span;
use tracing::subscriber::with_default;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::prelude::*;

use zeta_plugin::event::Subscriptions;
use zeta_plugin::{CommandSpec, FilterHandle};
use zeta_plugin_runtime::EventIndex;
use zeta_test_support::assert_empty;

/// The handler span reaches the exporter with its `otel.name`, `otel.kind`, parent and
/// messaging operation, which is the contract the dashboards consume.
#[test]
fn exported_handler_span_carries_name_kind_parent_and_messaging_operation() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("zeta-otel-export-test");
    let subscriber =
        tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));

    let (dispatch_id, handler) = with_default(subscriber, || {
        // The host creates the line span before routing; the delivered handler span must join
        // that trace. The OTel span id is captured to assert the exported parent id.
        let dispatch = info_span!("irc.dispatch", irc.command = "PRIVMSG");
        let dispatch_id = dispatch.context().span().span_context().span_id();
        let _entered = dispatch.enter();

        let mut subscriptions = Subscriptions::new();
        subscriptions.command(CommandSpec::new(".bench", "bench"));

        let (mailbox, mut receiver) = mpsc::channel(8);
        let mut index = EventIndex::default();
        index.add("bench", &subscriptions, mailbox);

        let stopped = index.dispatch(
            &FilterHandle::default(),
            Message::new(
                Some("nick!user@host.example"),
                "PRIVMSG",
                vec!["#test", ".bench now"],
            )
            .expect("the IRC message should parse"),
        );
        assert_empty!(stopped);

        let envelope = receiver
            .try_recv()
            .expect("the command should be delivered");

        (dispatch_id, envelope.span)
    });

    // Closing the handler span exports it through the provider's simple processor.
    drop(handler);

    let spans = exporter
        .get_finished_spans()
        .expect("the in-memory exporter should return the finished spans");
    let span = spans
        .iter()
        .find(|span| span.name == "zeta.event.handle bench command")
        .unwrap_or_else(|| {
            panic!(
                "the handler span should be exported under its otel.name, exported: {:?}",
                spans.iter().map(|span| &span.name).collect::<Vec<_>>()
            )
        });

    assert_eq!(span.span_kind, SpanKind::Consumer, "{span:?}");
    assert_eq!(
        span.parent_span_id, dispatch_id,
        "the handler span must be a child of the line span: {span:?}"
    );
    assert_eq!(
        attribute(span, "messaging.operation.name"),
        Some("privmsg"),
        "{span:?}"
    );
    assert_eq!(
        attribute(span, "messaging.destination.name"),
        Some("#test"),
        "{span:?}"
    );
    assert_eq!(
        attribute(span, "messaging.operation.type"),
        Some("process"),
        "{span:?}"
    );
}

/// Returns the string value of the span attribute named `key`.
fn attribute<'a>(span: &'a SpanData, key: &str) -> Option<&'a str> {
    match &span
        .attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == key)?
        .value
    {
        opentelemetry::Value::String(value) => Some(value.as_str()),
        _ => None,
    }
}
