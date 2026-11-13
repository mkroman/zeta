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

use irc::proto::Message;
use irc::proto::message::Tag;
use tokio::sync::mpsc;
use tracing::info_span;
use tracing::span::Id;
use tracing::subscriber::with_default;
use tracing_subscriber::prelude::*;
use zeta_plugin::event::Subscriptions;
use zeta_plugin::{CommandSpec, Event, FilterHandle};
use zeta_plugin_runtime::EventIndex;
use zeta_test_support::{RecordedSpan, SpanRecorder, assert_empty};

/// The handler span is created by the dispatcher while the IRC line's dispatch span is current;
/// it must therefore be a child of that span, not a new trace root. The plugin task relies on
/// this when it instruments the handler with the envelope's span.
///
/// The span also carries the OpenTelemetry messaging attributes alongside the `irc.*` keys: a
/// command and a channel message report `privmsg` and their destination, while a channel-less
/// `QUIT` reports `quit` and no destination. The IRCv3 `msgid` is recorded only when the message
/// carries one.
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
        .filter(|span| span.name() == "zeta.event.handle")
        .collect();
    assert_eq!(handlers.len(), 4, "recorded: {handlers:?}");

    for handler in &handlers {
        assert_eq!(
            handler.parent(),
            Some(dispatch_id),
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
    // The `irc.channel` key carries the same value as the `OTel` attribute.
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
        .find_map(|span| (span.name() == "irc.dispatch").then(|| span.parent()))
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

/// A full mailbox sheds the event before its handler span is built, so a delivery the plugin
/// never receives does not export an empty span.
///
/// The mailbox is given exactly one slot: the first dispatch is accepted and gets a handler
/// span, the second is shed before the span is created.
#[test]
fn shed_events_do_not_build_handler_spans() {
    let recorder = SpanRecorder::default();
    let subscriber = tracing_subscriber::registry().with(recorder.clone());

    with_default(subscriber, || {
        let mut subscriptions = Subscriptions::new();
        subscriptions.command(CommandSpec::new(".dig", "dig"));

        let (mailbox, mut receiver) = mpsc::channel(1);
        let mut index = EventIndex::default();
        index.add("dig", &subscriptions, mailbox);

        let stopped = index.dispatch(
            &FilterHandle::default(),
            message("nick!u@h", "PRIVMSG", &["#test", ".dig accepted"]),
        );
        assert_empty!(stopped);

        let stopped = index.dispatch(
            &FilterHandle::default(),
            message("nick!u@h", "PRIVMSG", &["#test", ".dig shed"]),
        );
        assert_empty!(stopped, "a full mailbox must not evict the plugin");

        let mut events = Vec::new();
        while let Ok(envelope) = receiver.try_recv() {
            events.push(envelope.event);
        }

        assert!(
            matches!(events.as_slice(), [Event::Command(command)] if command.args() == "accepted"),
            "the full mailbox should keep only the accepted event: {events:?}"
        );
    });

    let handlers: Vec<_> = recorder
        .spans()
        .into_iter()
        .filter(|span| span.name() == "zeta.event.handle")
        .collect();

    assert_eq!(
        handlers.len(),
        1,
        "a shed event must not build a handler span, recorded: {handlers:?}"
    );
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
