//! Dispatch-time span parenting, exercised in a test binary of its own.
//!
//! `tracing` caches one interest value per callsite for the lifetime of the process, computed
//! from the dispatcher that was current when the callsite was first used. The dispatcher's other
//! tests dispatch events without a subscriber, so in a shared test binary the `zeta.event.handle`
//! callsite would already be cached as disabled before this test installs its recorder, and a
//! thread-local subscriber cannot re-enable it. A separate binary starts with an empty callsite
//! cache: this file is the only place the dispatch span test runs, so the callsite is first used
//! under the recorder's subscriber and no global subscriber is needed.
//!
//! The index is built through the public `Subscriptions`/`CommandSpec` API and driven directly,
//! so the test needs no plugin runtime.

use std::sync::{Arc, Mutex};

use irc::proto::Message;
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::subscriber::with_default;
use tracing::{Subscriber, info_span};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use zeta_plugin::event::Subscriptions;
use zeta_plugin::{CommandSpec, Event, FilterHandle};
use zeta_plugin_runtime::EventIndex;
use zeta_test_support::assert_empty;

/// The handler span is created by the dispatcher while the IRC line's dispatch span is current;
/// it must therefore be a child of that span, not a new trace root. The plugin task relies on
/// this when it instruments the handler with the envelope's span.
#[test]
fn handler_span_is_a_child_of_the_dispatch_span() {
    let recorder = SpanRecorder::default();
    let subscriber = tracing_subscriber::registry().with(recorder.clone());

    with_default(subscriber, || {
        let dispatch = info_span!("irc.dispatch");
        let dispatch_id = dispatch.id().expect("the dispatch span should be enabled");
        let _entered = dispatch.enter();

        let mut subscriptions = Subscriptions::new();
        subscriptions.command(CommandSpec::new(".dig", "dig"));

        let (mailbox, mut receiver) = mpsc::unbounded_channel();
        let mut index = EventIndex::default();
        index.add("dig", &subscriptions, mailbox);

        let stopped = index.dispatch(&FilterHandle::default(), privmsg(".dig example.com"));
        assert_empty!(stopped);

        let envelope = receiver.try_recv().expect("the event should be dispatched");
        assert!(matches!(&envelope.event, Event::Command(..)));

        let spans = recorder.spans();

        let handler = spans
            .iter()
            .find(|span| span.name == "zeta.event.handle")
            .unwrap_or_else(|| panic!("zeta.event.handle should be recorded: {spans:?}"));
        assert_eq!(
            handler.parent,
            Some(dispatch_id),
            "zeta.event.handle must be a child of irc.dispatch, recorded: {spans:?}"
        );
        // The exported name carries the plugin and event kind, so the span is identifiable in
        // the trace view without opening it.
        assert_eq!(
            handler.otel_name.as_deref(),
            Some("zeta.event.handle dig command"),
            "recorded: {spans:?}"
        );

        let dispatch_parent = spans
            .iter()
            .find_map(|span| (span.name == "irc.dispatch").then_some(&span.parent))
            .expect("irc.dispatch should be recorded");
        assert!(
            dispatch_parent.is_none(),
            "irc.dispatch must be a root span, recorded: {spans:?}"
        );
    });
}

/// Builds a `PRIVMSG` from `nick` to `#test` carrying `text`.
fn privmsg(text: &str) -> Message {
    Message::new(
        Some("nick!user@host.example"),
        "PRIVMSG",
        vec!["#test", text],
    )
    .expect("message should parse")
}

/// One span as the recorder observed it.
#[derive(Clone, Debug)]
struct RecordedSpan {
    /// The span's static name.
    name: String,
    /// The span that was current when this one was created; a root has no parent.
    parent: Option<Id>,
    /// The span's `otel.name` field, when it declared one.
    otel_name: Option<String>,
}

/// Records every span created while it is installed.
#[derive(Clone, Default)]
struct SpanRecorder(Arc<Mutex<Vec<RecordedSpan>>>);

impl SpanRecorder {
    /// Returns a copy of the spans recorded so far.
    fn spans(&self) -> Vec<RecordedSpan> {
        self.0
            .lock()
            .expect("span recorder should not be poisoned")
            .clone()
    }
}

/// Collects the `otel.name` field of a span, rendered as its exported name.
#[derive(Default)]
struct OtelName(Option<String>);

impl Visit for OtelName {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "otel.name" {
            self.0 = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "otel.name" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

impl<S> Layer<S> for SpanRecorder
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, ctx: Context<'_, S>) {
        let parent = if attrs.is_contextual() {
            ctx.lookup_current().map(|span| span.id())
        } else {
            None
        };

        let mut otel_name = OtelName::default();
        attrs.record(&mut otel_name);

        let mut spans = self.0.lock().expect("span recorder should not be poisoned");
        spans.push(RecordedSpan {
            name: attrs.metadata().name().to_owned(),
            parent,
            otel_name: otel_name.0,
        });
    }
}
