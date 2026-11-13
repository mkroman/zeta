//! Dispatch-time span parenting and messaging attributes, exercised in a test binary of its own.
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

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use irc::proto::Message;
use irc::proto::message::Tag;
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
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
///
/// The span also carries the OpenTelemetry messaging attributes alongside the transitional
/// `irc.*` keys: a command and a channel message report `privmsg` and their destination, while a
/// channel-less `QUIT` reports `quit` and no destination. The IRCv3 `msgid` is recorded only when
/// the message carries one.
#[test]
fn handler_spans_carry_parenting_and_messaging_attributes() {
    let recorder = SpanRecorder::default();
    let subscriber = tracing_subscriber::registry().with(recorder.clone());

    let dispatch_id = with_default(subscriber, || {
        let dispatch = info_span!("irc.dispatch");
        let dispatch_id = dispatch.id().expect("the dispatch span should be enabled");
        let _entered = dispatch.enter();

        let mut subscriptions = Subscriptions::new();
        subscriptions
            .command(CommandSpec::new(".dig", "dig"))
            .receive_message()
            .receive_quit();

        let (mailbox, mut receiver) = mpsc::channel(8);
        let mut index = EventIndex::default();
        index.add("dig", &subscriptions, mailbox);

        // A command, a plain message (both tagged `PRIVMSG`s) and a channel-less quit.
        let stopped = index.dispatch(
            &FilterHandle::default(),
            tagged_privmsg(".dig example.com", "m1"),
        );
        assert_empty!(stopped);
        let stopped = index.dispatch(&FilterHandle::default(), tagged_privmsg("hello", "m2"));
        assert_empty!(stopped);
        let stopped = index.dispatch(
            &FilterHandle::default(),
            message("nick!u@h", "QUIT", &["gone"]),
        );
        assert_empty!(stopped);

        let mut events = Vec::new();
        while let Ok(envelope) = receiver.try_recv() {
            events.push(envelope.event);
        }

        assert!(
            matches!(
                events.as_slice(),
                [
                    Event::Command(..),
                    Event::Message(..),
                    Event::Message(..),
                    Event::Quit(..),
                ]
            ),
            "unexpected events: {events:?}"
        );

        dispatch_id
    });

    let spans = recorder.spans();
    assert_handler_spans(&spans, &dispatch_id);
}

/// Asserts the parenting and messaging attributes of the handler spans recorded for the
/// command, plain message and quit dispatched by the test.
fn assert_handler_spans(spans: &[RecordedSpan], dispatch_id: &Id) {
    let handlers: Vec<_> = spans
        .iter()
        .filter(|span| span.name == "zeta.event.handle")
        .collect();
    assert_eq!(handlers.len(), 4, "recorded: {handlers:?}");

    for handler in &handlers {
        assert_eq!(
            handler.parent,
            Some(dispatch_id.clone()),
            "zeta.event.handle must be a child of irc.dispatch, recorded: {handler:?}"
        );
        assert_eq!(
            handler.field("messaging.system.name"),
            Some("irc"),
            "recorded: {handler:?}"
        );
        assert_eq!(
            handler.field("messaging.operation.type"),
            Some("process"),
            "recorded: {handler:?}"
        );
    }

    let command = handler_with_kind(&handlers, "command");
    assert_eq!(
        command.field("otel.name"),
        Some("zeta.event.handle dig command"),
        "recorded: {command:?}"
    );
    assert_eq!(
        command.field("messaging.operation.name"),
        Some("privmsg"),
        "recorded: {command:?}"
    );
    assert_eq!(
        command.field("messaging.destination.name"),
        Some("#test"),
        "recorded: {command:?}"
    );
    // The transitional key stays for one release, with the same value as the `OTel` attribute.
    assert_eq!(
        command.field("irc.channel"),
        Some("#test"),
        "recorded: {command:?}"
    );
    assert_eq!(
        command.field("messaging.message.id"),
        Some("m1"),
        "recorded: {command:?}"
    );

    let message = handlers
        .iter()
        .copied()
        .find(|span| span.field("messaging.message.id") == Some("m2"))
        .unwrap_or_else(|| panic!("the plain message span should be recorded: {handlers:?}"));
    assert_eq!(
        message.field("otel.name"),
        Some("zeta.event.handle dig message"),
        "recorded: {message:?}"
    );
    assert_eq!(
        message.field("messaging.operation.name"),
        Some("privmsg"),
        "recorded: {message:?}"
    );
    assert_eq!(
        message.field("messaging.destination.name"),
        Some("#test"),
        "recorded: {message:?}"
    );

    let quit = handler_with_kind(&handlers, "quit");
    assert_eq!(
        quit.field("otel.name"),
        Some("zeta.event.handle dig quit"),
        "recorded: {quit:?}"
    );
    assert_eq!(
        quit.field("messaging.operation.name"),
        Some("quit"),
        "recorded: {quit:?}"
    );
    assert!(
        quit.field("messaging.destination.name").is_none(),
        "a channel-less event has no destination, recorded: {quit:?}"
    );
    assert!(
        quit.field("irc.channel").is_none(),
        "a channel-less event has no channel, recorded: {quit:?}"
    );
    assert!(
        quit.field("messaging.message.id").is_none(),
        "the quit message carries no msgid, recorded: {quit:?}"
    );

    let dispatch_parent = spans
        .iter()
        .find_map(|span| (span.name == "irc.dispatch").then_some(&span.parent))
        .expect("irc.dispatch should be recorded");
    assert!(
        dispatch_parent.is_none(),
        "irc.dispatch must be a root span, recorded: {spans:?}"
    );
}

/// Returns the handler span with the given `zeta.event.kind` field.
fn handler_with_kind<'a>(handlers: &[&'a RecordedSpan], kind: &str) -> &'a RecordedSpan {
    handlers
        .iter()
        .copied()
        .find(|span| span.field("zeta.event.kind") == Some(kind))
        .unwrap_or_else(|| panic!("no handler span with kind `{kind}`, recorded: {handlers:?}"))
}

/// Builds a `PRIVMSG` from `nick` to `#test` carrying `text` and an IRCv3 `msgid` tag.
fn tagged_privmsg(text: &str, msgid: &str) -> Message {
    Message::with_tags(
        Some(vec![Tag("msgid".to_owned(), Some(msgid.to_owned()))]),
        Some("nick!user@host.example"),
        "PRIVMSG",
        vec!["#test", text],
    )
    .expect("message should parse")
}

/// Builds a message from `prefix` carrying `command` with `args`.
fn message(prefix: &str, command: &str, args: &[&str]) -> Message {
    let prefix = (!prefix.is_empty()).then_some(prefix);

    Message::new(prefix, command, args.to_vec()).expect("message should parse")
}

/// One span as the recorder observed it.
#[derive(Clone, Debug)]
struct RecordedSpan {
    /// The span's identity, used to merge fields recorded after creation.
    id: Id,
    /// The span's static name.
    name: String,
    /// The span that was current when this one was created; a root has no parent.
    parent: Option<Id>,
    /// Every field recorded on the span, at creation or later.
    fields: BTreeMap<String, String>,
}

impl RecordedSpan {
    /// Returns the string value of `field`.
    fn field(&self, field: &str) -> Option<&str> {
        self.fields.get(field).map(String::as_str)
    }
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

/// Collects the fields of a span, rendered as their exported values.
#[derive(Default)]
struct Fields(BTreeMap<String, String>);

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
}

impl<S> Layer<S> for SpanRecorder
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let parent = if attrs.is_contextual() {
            ctx.lookup_current().map(|span| span.id())
        } else {
            None
        };

        let mut fields = Fields::default();
        attrs.record(&mut fields);

        let mut spans = self.0.lock().expect("span recorder should not be poisoned");
        spans.push(RecordedSpan {
            id: id.clone(),
            name: attrs.metadata().name().to_owned(),
            parent,
            fields: fields.0,
        });
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        values.record(&mut fields);

        self.0
            .lock()
            .expect("span recorder should not be poisoned")
            .iter_mut()
            .find(|span| span.id == *id)
            .expect("recorded spans should exist")
            .fields
            .extend(fields.0);
    }
}
