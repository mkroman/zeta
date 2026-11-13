//! Gungraun benchmarks for the enabled dispatch telemetry path.
//!
//! Run with `cargo bench -p zeta-benchmarks --bench dispatch_telemetry`.
//!
//! The main `dispatch` suite runs without a tracing subscriber, so `tracing::enabled!` is false
//! and the handler-span metadata path is skipped. The default filter (`zeta=info`) enables
//! those spans in production, so this suite installs a no-op subscriber in setup: the span
//! macros evaluate their fields exactly as they do in production, while the subscriber stores
//! and exports nothing, keeping span export out of the measurement.
//!
//! The subscriber is installed in setup (excluded from the measured body), not with a per-call
//! `with_default`, so the dispatcher allocation is not counted as span-construction cost.
//! `register_callsite` reports [`Interest::sometimes`] so the enabled state follows the thread's
//! current dispatcher; a cached interest would leak the disabled cases into the global no-op
//! subscriber. The enabled cases therefore also pay the `enabled` checks that a permanently
//! installed production subscriber may cache at the callsite.
//!
//! The fixtures mirror the `dispatch` suite. They are duplicated rather than shared so adding
//! this suite does not change the compiled `dispatch` binary and its recorded baseline.

// The benchmark harness passes setup results by value; borrowing them in the body is intended.
// Macro-generated items are benchmark scaffolding, not a public API, and their expanded paths
// trip the workspace lints.
#![allow(clippy::needless_pass_by_value, missing_docs, unused_qualifications)]

use std::hint::black_box;

use gungraun::prelude::*;
use gungraun::{Callgrind, Dhat, DhatMetric, EventKind, FlamegraphConfig};
use irc::proto::Message;
use tokio::sync::mpsc::Receiver;
use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id};
use tracing::subscriber::{DefaultGuard, Interest};
use tracing::{Metadata, Subscriber};

use zeta_plugin::{CommandSpec, FilterHandle, Subscriptions, UrlScope};
use zeta_plugin_runtime::{Envelope, EventIndex};

/// The mailbox capacity; every case dispatches one message, so any non-zero capacity works.
const MAILBOX_CAPACITY: usize = 8;

/// The sender prefix of the benchmark messages.
const PREFIX: &str = "bench!user@host.example";

/// The channel the benchmark messages are posted to.
const CHANNEL: &str = "#bench";

/// A `tracing` subscriber that enables INFO spans without storing or exporting them.
///
/// The benchmark needs the span macros to evaluate their fields the way production does, but it
/// must not measure span storage or export. `new_span` therefore returns an id that refers to
/// nothing, and every lifecycle method is a no-op.
///
/// `register_callsite` reports [`Interest::sometimes`] on purpose: the case runs under the
/// global no-op subscriber until setup installs this one, and a cached `never`/`always` interest
/// would stop `tracing::enabled!` from following the current dispatcher. With `sometimes`, every
/// call asks the installed subscriber.
struct NoopSubscriber;

impl Subscriber for NoopSubscriber {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= &tracing::Level::INFO
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::INFO)
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, _: &tracing::Event<'_>) {}

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// Installs the no-op subscriber for the setup lifetime, so the measured body runs with
/// `tracing::enabled!` true without paying for the dispatcher in the benchmark itself.
///
/// # Panics
///
/// Panics when the subscriber does not enable the handler spans, which would silently turn the
/// case into a copy of the filtered-out dispatch path.
fn enabling<T>(setup: impl FnOnce() -> T) -> (DefaultGuard, T) {
    let guard = tracing::subscriber::set_default(NoopSubscriber);

    assert!(
        tracing::enabled!(target: "zeta.plugins", tracing::Level::INFO),
        "the no-op subscriber must enable the handler spans"
    );

    (guard, setup())
}

/// Builds an IRC message from the benchmark sender.
fn message(command: &str, text: &str) -> Message {
    Message::new(Some(PREFIX), command, vec![CHANNEL, text])
        .expect("the benchmark message should parse")
}

fn privmsg(text: &str) -> Message {
    message("PRIVMSG", text)
}

fn notice(text: &str) -> Message {
    message("NOTICE", text)
}

/// Adds a subscriber with its own mailbox to `index`.
fn add_subscriber(
    index: &mut EventIndex,
    receivers: &mut Vec<Receiver<Envelope>>,
    register: impl FnOnce(&mut Subscriptions),
) {
    let mut subscriptions = Subscriptions::new();
    register(&mut subscriptions);

    let (sender, receiver) = tokio::sync::mpsc::channel(MAILBOX_CAPACITY);
    index.add("bench", &subscriptions, sender);
    receivers.push(receiver);
}

/// Setup for a dispatch with no subscriptions: classification only, no allocation expected.
fn setup_unhandled(text: &'static str) -> (EventIndex, FilterHandle, Message) {
    (
        EventIndex::default(),
        FilterHandle::default(),
        privmsg(text),
    )
}

/// Setup for a message dispatch to `subscribers` message subscribers.
fn setup_message(
    text: &'static str,
    subscribers: usize,
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle, Message) {
    let mut index = EventIndex::default();
    let mut receivers = Vec::with_capacity(subscribers);

    for _ in 0..subscribers {
        add_subscriber(&mut index, &mut receivers, |subscriptions| {
            subscriptions.receive_message();
        });
    }

    (index, receivers, FilterHandle::default(), privmsg(text))
}

fn setup_message_one(
    text: &'static str,
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle, Message) {
    setup_message(text, 1)
}

fn setup_message_eight(
    text: &'static str,
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle, Message) {
    setup_message(text, 8)
}

/// Setup for a command dispatch, with the `.bench` command registered or not.
fn setup_command(
    text: &'static str,
    subscribed: bool,
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle, Message) {
    let mut index = EventIndex::default();
    let mut receivers = Vec::with_capacity(1);

    if subscribed {
        add_subscriber(&mut index, &mut receivers, |subscriptions| {
            subscriptions.command(CommandSpec::new(".bench", "bench"));
        });
    }

    (index, receivers, FilterHandle::default(), privmsg(text))
}

fn setup_command_subscribed(
    text: &'static str,
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle, Message) {
    setup_command(text, true)
}

/// Setup for a CTCP dispatch (or a plain notice that must not produce a CTCP event).
fn setup_ctcp(ctcp: bool) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle, Message) {
    let mut index = EventIndex::default();
    let mut receivers = Vec::with_capacity(1);
    add_subscriber(&mut index, &mut receivers, |subscriptions| {
        subscriptions.receive_ctcp();
    });

    let message = if ctcp {
        privmsg("\u{1}VERSION\u{1}")
    } else {
        notice("maintenance")
    };

    (index, receivers, FilterHandle::default(), message)
}

/// Setup for a URL dispatch, with a host-scoped or generic subscriber.
fn setup_url(
    scope: &'static str,
    text: &'static str,
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle, Message) {
    let mut index = EventIndex::default();
    let mut receivers = Vec::with_capacity(1);

    add_subscriber(&mut index, &mut receivers, |subscriptions| {
        subscriptions.urls(match scope {
            "host" => UrlScope::Hosts(&["example.com"]),
            "any" => UrlScope::Any,
            other => panic!("unknown URL scope `{other}`"),
        });
    });

    (index, receivers, FilterHandle::default(), privmsg(text))
}

fn setup_url_host(
    text: &'static str,
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle, Message) {
    setup_url("host", text)
}

fn setup_url_any(
    text: &'static str,
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle, Message) {
    setup_url("any", text)
}

/// Setup for the enabled cases: the plain fixture with the no-op subscriber installed so the
/// measured call constructs the handler spans.
fn setup_unhandled_enabled(
    text: &'static str,
) -> (DefaultGuard, EventIndex, FilterHandle, Message) {
    let (guard, (index, filters, message)) = enabling(|| setup_unhandled(text));

    (guard, index, filters, message)
}

fn setup_message_one_enabled(
    text: &'static str,
) -> (
    DefaultGuard,
    EventIndex,
    Vec<Receiver<Envelope>>,
    FilterHandle,
    Message,
) {
    let (guard, (index, receivers, filters, message)) = enabling(|| setup_message_one(text));

    (guard, index, receivers, filters, message)
}

fn setup_message_eight_enabled(
    text: &'static str,
) -> (
    DefaultGuard,
    EventIndex,
    Vec<Receiver<Envelope>>,
    FilterHandle,
    Message,
) {
    let (guard, (index, receivers, filters, message)) = enabling(|| setup_message_eight(text));

    (guard, index, receivers, filters, message)
}

fn setup_command_subscribed_enabled(
    text: &'static str,
) -> (
    DefaultGuard,
    EventIndex,
    Vec<Receiver<Envelope>>,
    FilterHandle,
    Message,
) {
    let (guard, (index, receivers, filters, message)) = enabling(|| setup_command_subscribed(text));

    (guard, index, receivers, filters, message)
}

fn setup_ctcp_enabled(
    ctcp: bool,
) -> (
    DefaultGuard,
    EventIndex,
    Vec<Receiver<Envelope>>,
    FilterHandle,
    Message,
) {
    let (guard, (index, receivers, filters, message)) = enabling(|| setup_ctcp(ctcp));

    (guard, index, receivers, filters, message)
}

fn setup_url_host_enabled(
    text: &'static str,
) -> (
    DefaultGuard,
    EventIndex,
    Vec<Receiver<Envelope>>,
    FilterHandle,
    Message,
) {
    let (guard, (index, receivers, filters, message)) = enabling(|| setup_url_host(text));

    (guard, index, receivers, filters, message)
}

fn setup_url_any_enabled(
    text: &'static str,
) -> (
    DefaultGuard,
    EventIndex,
    Vec<Receiver<Envelope>>,
    FilterHandle,
    Message,
) {
    let (guard, (index, receivers, filters, message)) = enabling(|| setup_url_any(text));

    (guard, index, receivers, filters, message)
}

#[library_benchmark]
#[bench::privmsg(args = ("hello there"), setup = setup_unhandled_enabled)]
fn bench_telemetry_unhandled(
    (guard, mut index, filters, message): (DefaultGuard, EventIndex, FilterHandle, Message),
) -> (DefaultGuard, EventIndex, FilterHandle) {
    let stopped = index.dispatch(&filters, message);
    black_box(stopped);
    black_box((guard, index, filters))
}

#[library_benchmark]
#[bench::one_subscriber(args = ("hello there"), setup = setup_message_one_enabled)]
#[bench::eight_subscribers(args = ("hello there"), setup = setup_message_eight_enabled)]
fn bench_telemetry_message(
    (guard, mut index, receivers, filters, message): (
        DefaultGuard,
        EventIndex,
        Vec<Receiver<Envelope>>,
        FilterHandle,
        Message,
    ),
) -> (
    DefaultGuard,
    EventIndex,
    Vec<Receiver<Envelope>>,
    FilterHandle,
) {
    let stopped = index.dispatch(&filters, message);
    black_box(stopped);
    black_box((guard, index, receivers, filters))
}

#[library_benchmark]
#[bench::hit(args = (".bench now"), setup = setup_command_subscribed_enabled)]
fn bench_telemetry_command(
    (guard, mut index, receivers, filters, message): (
        DefaultGuard,
        EventIndex,
        Vec<Receiver<Envelope>>,
        FilterHandle,
        Message,
    ),
) -> (
    DefaultGuard,
    EventIndex,
    Vec<Receiver<Envelope>>,
    FilterHandle,
) {
    let stopped = index.dispatch(&filters, message);
    black_box(stopped);
    black_box((guard, index, receivers, filters))
}

#[library_benchmark]
#[bench::ctcp(args = (true), setup = setup_ctcp_enabled)]
fn bench_telemetry_ctcp(
    (guard, mut index, receivers, filters, message): (
        DefaultGuard,
        EventIndex,
        Vec<Receiver<Envelope>>,
        FilterHandle,
        Message,
    ),
) -> (
    DefaultGuard,
    EventIndex,
    Vec<Receiver<Envelope>>,
    FilterHandle,
) {
    let stopped = index.dispatch(&filters, message);
    black_box(stopped);
    black_box((guard, index, receivers, filters))
}

#[library_benchmark]
#[bench::host_match(args = ("see https://example.com/path"), setup = setup_url_host_enabled)]
#[bench::any(args = ("see https://example.com/path"), setup = setup_url_any_enabled)]
fn bench_telemetry_url(
    (guard, mut index, receivers, filters, message): (
        DefaultGuard,
        EventIndex,
        Vec<Receiver<Envelope>>,
        FilterHandle,
        Message,
    ),
) -> (
    DefaultGuard,
    EventIndex,
    Vec<Receiver<Envelope>>,
    FilterHandle,
) {
    let stopped = index.dispatch(&filters, message);
    black_box(stopped);
    black_box((guard, index, receivers, filters))
}

library_benchmark_group!(
    name = dispatch_telemetry,
    compare_by_id = false,
    benchmarks = [
        bench_telemetry_unhandled,
        bench_telemetry_message,
        bench_telemetry_command,
        bench_telemetry_ctcp,
        bench_telemetry_url,
    ]
);

main!(
    config = LibraryBenchmarkConfig::default()
        .tool(
            Callgrind::default()
                .soft_limits([(EventKind::Ir, 1.0)])
                .flamegraph(FlamegraphConfig::default())
        )
        .tool(Dhat::default().soft_limits([
            (DhatMetric::TotalBlocks, 0.0),
            (DhatMetric::TotalBytes, 0.0),
        ])),
    library_benchmark_groups = dispatch_telemetry
);
