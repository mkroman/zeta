//! Gungraun benchmarks for the plugin dispatch pipeline.
//!
//! Run with `cargo bench -p zeta-benchmarks --bench dispatch`.
//!
//! The suite is split into three groups that share the same fixtures:
//!
//! - `index` measures [`EventIndex::dispatch`]: IRC command classification, command-trigger
//!   matching, message/CTCP construction, URL extraction and mailbox hand-off. The static
//!   dispatch refactor also touches this side (span-name/channel hoisting), so it is measured
//!   rather than assumed constant.
//! - `handler` measures invoking the plugin side for one already-built event: the generated
//!   trait forwarder, the boxed future before the static-dispatch refactor, and the handler itself.
//! - `pipeline` measures dispatch plus handler in one call — the complete synchronous path
//!   from an IRC message to a plugin handler.
//!
//! The scope is deliberately narrow so the numbers stay actionable:
//!
//! - No Tokio task scheduling: the mailbox receiver is drained in the same thread, so the
//!   measurements exclude task wakeups and worker scheduling.
//! - No tracing subscriber: the line- and handler-span paths are disabled, so neither span
//!   construction nor span export is measured. The default filter (`zeta=info`) enables those
//!   spans in production, so this suite measures the filtered-out dispatch path only. The
//!   enabled path is measured separately by the `dispatch_telemetry` suite.
//! - No network, database, clock or randomness: handlers are no-ops with fixed inputs.
//! - One dispatch per measured run; setup (context construction, mock client, event building)
//!   is excluded from the measurement.
//!
//! Save a reference baseline with `-- --save-baseline=<name>` and compare against it with
//! `-- --baseline=<name>`. The case ids are stable across the plugin dispatch refactor so a
//! pre-refactor baseline stays comparable with the new implementation.

// The benchmark harness passes setup results by value; borrowing them in the body is intended.
// Macro-generated items are benchmark scaffolding, not a public API, and their expanded paths
// trip the workspace lints.
#![allow(clippy::needless_pass_by_value, missing_docs, unused_qualifications)]

use std::hint::black_box;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll, Waker};

use gungraun::prelude::*;
use gungraun::{Callgrind, Dhat, DhatMetric, EventKind, FlamegraphConfig};
use irc::proto::Message;
use tokio::sync::mpsc::Receiver;
use tracing::{Instrument, Span};

use zeta_plugin::{
    Client, CommandSpec, Context, Event, FilterHandle, Plugin, Subscriptions, UrlScope, plugin,
};
use zeta_plugin_runtime::{Envelope, EventIndex};

/// The mailbox capacity; every case dispatches one message, so any non-zero capacity works.
const MAILBOX_CAPACITY: usize = 8;

/// The sender prefix of the benchmark messages.
const PREFIX: &str = "bench!user@host.example";

/// The channel the benchmark messages are posted to.
const CHANNEL: &str = "#bench";

/// A plugin exercising the macro-generated dispatch path.
///
/// The handler bodies are empty: the benchmark measures routing and invocation, not the
/// handler's work.
#[derive(Default)]
struct BenchPlugin;

#[plugin(name = "bench", urls(any))]
impl BenchPlugin {
    #[handler(message)]
    async fn message(
        &self,
        _: &Context,
        _: &Client,
        _: &zeta_plugin::MessageEvent,
    ) -> Result<(), zeta_plugin::Error> {
        Ok(())
    }

    #[command(".bench", "dispatch benchmark command")]
    async fn command(
        &self,
        _: &Context,
        _: &Client,
        _channel: &str,
    ) -> Result<(), zeta_plugin::Error> {
        Ok(())
    }

    #[handler(url)]
    async fn url(
        &self,
        _: &Context,
        _: &Client,
        _: &zeta_plugin::UrlEvent,
    ) -> Result<(), zeta_plugin::Error> {
        Ok(())
    }
}

/// Invokes one event through the plugin API.
///
/// Kept in one function because the plugin-API refactor changes the call shape; the case ids
/// and the measured scenario stay identical so instruction and allocation counts remain
/// comparable across the refactor.
fn invoke(
    plugin: &BenchPlugin,
    ctx: &Context,
    client: &Client,
    event: &Event,
    span: Span,
) -> Result<(), zeta_plugin::Error> {
    block_on(plugin.handle(ctx, client, event).instrument(span))
}

/// Polls `future` to completion on the current thread with a no-op waker.
///
/// The benchmark handlers are ready on the first poll, so this adds no scheduler noise.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::noop();
    let mut context = TaskContext::from_waker(waker);

    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
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

/// Creates the plugin context and the mock IRC client used by the handler benchmarks.
///
/// The mock client's runtime is dropped once the client is built; the handlers never touch the
/// client, so only its address is passed through. This keeps Tokio out of the measured path.
fn fixture() -> (Context, Arc<Client>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the benchmark runtime should build");
    let (context, client) = runtime.block_on(async {
        (
            zeta_test_support::test_context(),
            zeta_test_support::mock_client().await,
        )
    });

    (context, Arc::new(client))
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

fn setup_command_unsubscribed(
    text: &'static str,
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle, Message) {
    setup_command(text, false)
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

/// Builds the index and the event for the handler benchmarks, then extracts the delivered
/// event from the mailbox. Everything here runs in setup and is excluded from the measurement.
fn setup_handler(kind: &'static str) -> (BenchPlugin, Context, Arc<Client>, Envelope) {
    let (mut index, mut receiver, filters, message) = match kind {
        "message" => one_subscriber(
            |subscriptions| {
                subscriptions.receive_message();
            },
            privmsg("hello there"),
        ),
        "command" => one_subscriber(
            |subscriptions| {
                subscriptions.command(CommandSpec::new(".bench", "bench"));
            },
            privmsg(".bench now"),
        ),
        "url" => one_subscriber(
            |subscriptions| {
                subscriptions.urls(UrlScope::Any);
            },
            privmsg("see https://example.com/path"),
        ),
        other => panic!("unknown handler kind `{other}`"),
    };

    let stopped = index.dispatch(&filters, message);
    assert!(
        stopped.is_empty(),
        "the benchmark subscriber should be alive"
    );

    let envelope = receiver.try_recv().expect("the event should be delivered");
    let (ctx, client) = fixture();

    (BenchPlugin, ctx, client, envelope)
}

/// Builds an index with one subscriber; the caller dispatches the message.
fn one_subscriber(
    register: impl FnOnce(&mut Subscriptions),
    message: Message,
) -> (EventIndex, Receiver<Envelope>, FilterHandle, Message) {
    let mut index = EventIndex::default();
    let mut subscriptions = Subscriptions::new();
    register(&mut subscriptions);
    let (sender, receiver) = tokio::sync::mpsc::channel(MAILBOX_CAPACITY);
    index.add("bench", &subscriptions, sender);

    (index, receiver, FilterHandle::default(), message)
}

/// The complete pipeline state: index, mailbox, message and the plugin side.
struct Pipeline {
    index: EventIndex,
    receiver: Receiver<Envelope>,
    filters: FilterHandle,
    message: Message,
    plugin: BenchPlugin,
    ctx: Context,
    client: Arc<Client>,
}

/// Setup for the pipeline benchmarks.
fn setup_pipeline(kind: &'static str) -> Pipeline {
    let (index, receiver, filters, message) = match kind {
        "message" => one_subscriber(
            |subscriptions| {
                subscriptions.receive_message();
            },
            privmsg("hello there"),
        ),
        "command" => one_subscriber(
            |subscriptions| {
                subscriptions.command(CommandSpec::new(".bench", "bench"));
            },
            privmsg(".bench now"),
        ),
        "url" => one_subscriber(
            |subscriptions| {
                subscriptions.urls(UrlScope::Any);
            },
            privmsg("see https://example.com/path"),
        ),
        other => panic!("unknown pipeline kind `{other}`"),
    };
    let (ctx, client) = fixture();

    Pipeline {
        index,
        receiver,
        filters,
        message,
        plugin: BenchPlugin,
        ctx,
        client,
    }
}

#[library_benchmark]
#[bench::privmsg(args = ("hello there"), setup = setup_unhandled)]
fn bench_dispatch_unhandled(
    (mut index, filters, message): (EventIndex, FilterHandle, Message),
) -> (EventIndex, FilterHandle) {
    let stopped = index.dispatch(&filters, message);
    black_box(stopped);
    black_box((index, filters))
}

#[library_benchmark]
#[bench::one_subscriber(args = ("hello there"), setup = setup_message_one)]
#[bench::eight_subscribers(args = ("hello there"), setup = setup_message_eight)]
fn bench_dispatch_message(
    (mut index, receivers, filters, message): (
        EventIndex,
        Vec<Receiver<Envelope>>,
        FilterHandle,
        Message,
    ),
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle) {
    let stopped = index.dispatch(&filters, message);
    black_box(stopped);
    black_box((index, receivers, filters))
}

#[library_benchmark]
#[bench::hit(args = (".bench now"), setup = setup_command_subscribed)]
#[bench::miss(args = (".other now"), setup = setup_command_subscribed)]
#[bench::unsubscribed(args = (".bench now"), setup = setup_command_unsubscribed)]
fn bench_dispatch_command(
    (mut index, receivers, filters, message): (
        EventIndex,
        Vec<Receiver<Envelope>>,
        FilterHandle,
        Message,
    ),
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle) {
    let stopped = index.dispatch(&filters, message);
    black_box(stopped);
    black_box((index, receivers, filters))
}

#[library_benchmark]
#[bench::ctcp(args = (true), setup = setup_ctcp)]
#[bench::plain_notice(args = (false), setup = setup_ctcp)]
fn bench_dispatch_ctcp(
    (mut index, receivers, filters, message): (
        EventIndex,
        Vec<Receiver<Envelope>>,
        FilterHandle,
        Message,
    ),
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle) {
    let stopped = index.dispatch(&filters, message);
    black_box(stopped);
    black_box((index, receivers, filters))
}

#[library_benchmark]
#[bench::host_match(args = ("see https://example.com/path"), setup = setup_url_host)]
#[bench::host_miss(args = ("see https://other.example/path"), setup = setup_url_host)]
#[bench::any(args = ("see https://example.com/path"), setup = setup_url_any)]
fn bench_dispatch_url(
    (mut index, receivers, filters, message): (
        EventIndex,
        Vec<Receiver<Envelope>>,
        FilterHandle,
        Message,
    ),
) -> (EventIndex, Vec<Receiver<Envelope>>, FilterHandle) {
    let stopped = index.dispatch(&filters, message);
    black_box(stopped);
    black_box((index, receivers, filters))
}

#[library_benchmark]
#[bench::message(args = ("message"), setup = setup_handler)]
#[bench::command(args = ("command"), setup = setup_handler)]
#[bench::url(args = ("url"), setup = setup_handler)]
fn bench_handler(
    (plugin, ctx, client, envelope): (BenchPlugin, Context, Arc<Client>, Envelope),
) -> (BenchPlugin, Context, Arc<Client>, Envelope) {
    let result = invoke(
        &plugin,
        &ctx,
        &client,
        &envelope.event,
        envelope.span.clone(),
    );
    black_box(result).expect("the benchmark handler should succeed");
    black_box((plugin, ctx, client, envelope))
}

#[library_benchmark]
#[bench::message(args = ("message"), setup = setup_pipeline)]
#[bench::command(args = ("command"), setup = setup_pipeline)]
#[bench::url(args = ("url"), setup = setup_pipeline)]
fn bench_pipeline(
    pipeline: Pipeline,
) -> (
    EventIndex,
    Receiver<Envelope>,
    FilterHandle,
    Envelope,
    BenchPlugin,
    Context,
    Arc<Client>,
) {
    let Pipeline {
        mut index,
        receiver: mut mailbox,
        filters,
        message,
        plugin,
        ctx,
        client,
    } = pipeline;

    let stopped = index.dispatch(&filters, message);
    assert!(
        stopped.is_empty(),
        "the benchmark subscriber should be alive"
    );

    let envelope = mailbox.try_recv().expect("the event should be delivered");
    let result = invoke(
        &plugin,
        &ctx,
        &client,
        &envelope.event,
        envelope.span.clone(),
    );
    black_box(result).expect("the benchmark handler should succeed");
    black_box((index, mailbox, filters, envelope, plugin, ctx, client))
}

library_benchmark_group!(
    name = dispatch,
    compare_by_id = false,
    benchmarks = [
        bench_dispatch_unhandled,
        bench_dispatch_message,
        bench_dispatch_command,
        bench_dispatch_ctcp,
        bench_dispatch_url,
        bench_handler,
        bench_pipeline,
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
    library_benchmark_groups = dispatch
);
