//! Event dispatching: routing incoming IRC messages to the plugins that registered interest.
//!
//! The [`EventIndex`] maps the subscriptions every plugin declared during initialization to
//! delivery endpoints. It matches incoming IRC messages against the index — commands by their
//! trigger through a hash lookup on the message's first word, URLs by their host through a
//! hash lookup, and the remaining event kinds through subscriber lists — and queues the
//! matching events on each plugin's mailbox. A plugin is only woken for events it asked for;
//! for the indexed kinds the lookup cost is independent of the number of plugins.

use std::collections::HashMap;
use std::sync::Arc;

use irc::proto::{Command, Message};
use smallvec::SmallVec;
use tokio::sync::mpsc;
use tracing::{Span, debug, warn};
use url::Url;
use zeta_plugin::event::{CommandEvent, Sender, Subscriptions, UrlEvent};
use zeta_plugin::{CommandSpec, Event, EventKind, Filters, UrlScope};
use zeta_url::{ExtractUrls, ExtractedUrl, HTTP_SCHEMES, RedactedUrl};

/// An event queued for a plugin together with the handler span it must run inside.
///
/// The span is created while the dispatch span is current, so it is a child of the IRC line's
/// trace. It travels with the event because the plugin task has no inherited `tracing` context.
#[derive(Debug)]
pub struct Envelope {
    /// The event to deliver.
    pub event: Event,
    /// The consumer span to enter while handling the event.
    pub span: Span,
}

/// The exported handler span names of one plugin, built once when it is added to the index.
///
/// Every handler span is named `zeta.event.handle` for filtering and metrics; the exported
/// `otel.name` carries the bounded discriminator — plugin and event kind — so a trace with one
/// handler span per subscriber stays readable without opening each span.
#[derive(Debug)]
struct SpanNames {
    /// The name for command events.
    command: Arc<str>,
    /// The name for URL events.
    url: Arc<str>,
    /// The names for the flag-based kinds, in [`EventKind::ALL`] order.
    kinds: [Arc<str>; EventKind::ALL.len()],
}

impl SpanNames {
    /// Builds the handler span names for `plugin`.
    fn new(plugin: &str) -> Self {
        let composite = |kind: &str| Arc::<str>::from(format!("zeta.event.handle {plugin} {kind}"));

        Self {
            command: composite("command"),
            url: composite("url"),
            kinds: [
                composite("message"),
                composite("join"),
                composite("part"),
                composite("quit"),
                composite("nick"),
                composite("kick"),
                composite("ctcp"),
                composite("raw"),
            ],
        }
    }

    /// Returns the exported name for `event`.
    fn event(&self, event: &Event) -> &str {
        match event.kind() {
            Some(kind) => &self.kinds[kind_slot(kind)],
            None if matches!(event, Event::Command(_)) => &self.command,
            None => &self.url,
        }
    }
}

/// Returns the slot of `kind` in [`SpanNames::kinds`], which follows [`EventKind::ALL`].
const fn kind_slot(kind: EventKind) -> usize {
    match kind {
        EventKind::Message => 0,
        EventKind::Join => 1,
        EventKind::Part => 2,
        EventKind::Quit => 3,
        EventKind::Nick => 4,
        EventKind::Kick => 5,
        EventKind::Ctcp => 6,
        EventKind::Raw => 7,
    }
}

/// A delivery endpoint for events: the mailbox of one plugin's task.
#[derive(Clone, Debug)]
struct Subscriber {
    /// The plugin's name, used for logging.
    name: &'static str,
    /// The precomputed handler span names of the plugin.
    names: Arc<SpanNames>,
    /// The mailbox events are queued on.
    mailbox: mpsc::UnboundedSender<Envelope>,
}

impl Subscriber {
    /// Queues `event` for the plugin, recording the plugin in `stopped` when its mailbox is
    /// closed (i.e. the plugin's task has stopped).
    fn send(&self, event: Event, stopped: &mut Vec<&'static str>) {
        // Created here, while the dispatch span is current, so the handler joins the message's
        // trace. The span also covers the time the event waits in the mailbox. `or_current`
        // falls back to the dispatch span when the handler span is disabled by the filter, so
        // the handler logs still have a trace context.
        //
        // The name is precomputed: formatting it here would allocate on every event even when
        // `tracing` is disabled and the macro never evaluates its fields.
        let name = self.names.event(&event);

        let span = tracing::info_span!(
            "zeta.event.handle",
            otel.name = %name,
            otel.kind = "consumer",
            zeta.plugin.name = %self.name,
            zeta.event.kind = event.kind_name(),
            irc.channel = tracing::field::Empty,
        );

        if let Some(channel) = event.channel() {
            span.record("irc.channel", channel);
        }

        let span = span.or_current();
        let envelope = Envelope { event, span };

        if self.mailbox.send(envelope).is_err() {
            stopped.push(self.name);
        }
    }
}

/// A registered command in the dispatch index.
#[derive(Clone, Debug)]
struct CommandTarget {
    subscriber: Subscriber,
    spec: CommandSpec,
}

/// The IRC message being dispatched, shared with the events only when one is delivered.
///
/// Most incoming lines produce no event: no plugin subscribed to them, a `PRIVMSG` carries no
/// command or URL, or a `NOTICE` is not a CTCP message. Moving the message into an [`Arc`] on
/// the first delivery, instead of before routing, keeps those lines allocation-free; every
/// further event from the same line clones the shared handle.
struct SharedMessage {
    /// The message, until the first event needs it.
    message: Option<Message>,
    /// The shared message, once an event needed it.
    shared: Option<Arc<Message>>,
}

impl SharedMessage {
    /// Wraps `message` without allocating.
    const fn new(message: Message) -> Self {
        Self {
            message: Some(message),
            shared: None,
        }
    }

    /// Returns the message, whether or not it has been shared.
    fn get(&self) -> &Message {
        self.shared
            .as_deref()
            .or(self.message.as_ref())
            .expect("the message is either unshared or shared")
    }

    /// Returns a handle to the shared message, allocating it on first use.
    fn shared(&mut self) -> Arc<Message> {
        if let Some(shared) = &self.shared {
            return Arc::clone(shared);
        }

        let shared = Arc::new(
            self.message
                .take()
                .expect("the message is shared at most once"),
        );
        self.shared = Some(Arc::clone(&shared));

        shared
    }
}

/// What [`EventIndex::dispatch`] should do with a message's IRC command.
#[derive(Clone, Copy)]
enum Route {
    /// A channel message: commands, messages, or URLs may match.
    Privmsg,
    /// A flag-based event kind.
    Kind(EventKind),
    /// Connection protocol or another command no plugin can observe.
    Ignore,
}

impl Route {
    /// Classifies `message` by the events its command can produce.
    const fn of(message: &Message) -> Self {
        match &message.command {
            Command::PRIVMSG(..) => Self::Privmsg,
            // CTCP replies arrive as NOTICE-wrapped messages; only CTCP subscribers see them —
            // ordinary notices are not an event kind.
            Command::NOTICE(..) => Self::Kind(EventKind::Ctcp),
            Command::JOIN(..) => Self::Kind(EventKind::Join),
            Command::PART(..) => Self::Kind(EventKind::Part),
            Command::QUIT(..) => Self::Kind(EventKind::Quit),
            Command::NICK(..) => Self::Kind(EventKind::Nick),
            Command::KICK(..) => Self::Kind(EventKind::Kick),
            Command::Raw(..) => Self::Kind(EventKind::Raw),
            // Everything else is connection protocol the plugins have no events for.
            _ => Self::Ignore,
        }
    }
}

/// The dispatch index of every registered plugin's subscriptions.
pub struct EventIndex {
    /// Registered commands, indexed by their trigger — the first word of a message.
    commands: HashMap<&'static str, Vec<CommandTarget>>,
    /// Plugins subscribed to URLs on specific hosts, indexed by the lowercased host.
    url_hosts: HashMap<String, Vec<Subscriber>>,
    /// Plugins subscribed to every URL, including hosts other plugins handle.
    url_any: Vec<Subscriber>,
    /// Plugins subscribed to plain-interest event kinds, indexed by [`kind_slot`].
    kinds: [Vec<Subscriber>; EventKind::ALL.len()],
}

impl Default for EventIndex {
    fn default() -> Self {
        Self {
            commands: HashMap::new(),
            url_hosts: HashMap::new(),
            url_any: Vec::new(),
            kinds: std::array::from_fn(|_| Vec::new()),
        }
    }
}

impl EventIndex {
    /// Adds a plugin to the index with the `subscriptions` it declared and its `mailbox`.
    ///
    /// Command triggers containing whitespace cannot be matched by the first-word index and
    /// are skipped with a warning.
    pub fn add(
        &mut self,
        name: &'static str,
        subscriptions: &Subscriptions,
        mailbox: mpsc::UnboundedSender<Envelope>,
    ) {
        let subscriber = Subscriber {
            name,
            names: Arc::new(SpanNames::new(name)),
            mailbox,
        };

        for command in subscriptions.commands() {
            if command.trigger().contains(char::is_whitespace) {
                warn!(
                    zeta.plugin.name = %name,
                    command = command.trigger(),
                    "command trigger contains whitespace; it cannot be dispatched"
                );

                continue;
            }

            self.commands
                .entry(command.trigger())
                .or_default()
                .push(CommandTarget {
                    subscriber: subscriber.clone(),
                    spec: *command,
                });
        }

        match subscriptions.url_scope() {
            UrlScope::None => {}
            UrlScope::Hosts(hosts) => {
                for host in hosts {
                    self.url_hosts
                        .entry((*host).to_ascii_lowercase())
                        .or_default()
                        .push(subscriber.clone());
                }
            }
            UrlScope::Any => self.url_any.push(subscriber.clone()),
        }

        for kind in subscriptions.events() {
            self.kinds[kind_slot(*kind)].push(subscriber.clone());
        }
    }

    /// Routes `message` to the plugins that subscribed to events it matches.
    ///
    /// Returns the names of the plugins whose mailboxes are closed — their tasks have stopped.
    /// They are already evicted from the index, so the caller only needs to report them.
    #[must_use]
    pub fn dispatch(&mut self, filters: &Filters, message: Message) -> Vec<&'static str> {
        let mut message = SharedMessage::new(message);
        let mut stopped = Vec::new();

        match Route::of(message.get()) {
            Route::Privmsg => self.dispatch_privmsg(filters, &mut message, &mut stopped),
            Route::Kind(kind) => self.deliver_kind(kind, &mut message, &mut stopped),
            Route::Ignore => {}
        }

        // A stopped plugin can match several of the message's events (and several commands or
        // URLs); report and evict it once.
        stopped.sort_unstable();
        stopped.dedup();

        for name in &stopped {
            self.evict(name);
        }

        stopped
    }

    /// Removes every trace of the named plugin from the index.
    ///
    /// Called when one of its deliveries failed because the plugin's task stopped.
    fn evict(&mut self, name: &str) {
        let other = |subscriber: &Subscriber| subscriber.name != name;

        self.commands.retain(|_, targets| {
            targets.retain(|target| other(&target.subscriber));
            !targets.is_empty()
        });

        self.url_hosts.retain(|_, subscribers| {
            subscribers.retain(|subscriber| other(subscriber));
            !subscribers.is_empty()
        });
        self.url_any.retain(|subscriber| other(subscriber));

        for subscribers in &mut self.kinds {
            subscribers.retain(|subscriber| other(subscriber));
        }
    }

    /// Returns the subscriber list of a plain-interest event kind.
    fn subscribers(&self, kind: EventKind) -> &[Subscriber] {
        &self.kinds[kind_slot(kind)]
    }

    /// Delivers `message` as an event of `kind` to its subscribers, if any are registered.
    ///
    /// The event, and the shared message it needs, is built only when a subscriber exists and
    /// the message actually carries the event: most messages match no subscription, and CTCP
    /// events only come from CTCP text.
    fn deliver_kind(
        &self,
        kind: EventKind,
        message: &mut SharedMessage,
        stopped: &mut Vec<&'static str>,
    ) {
        let subscribers = self.subscribers(kind);

        if subscribers.is_empty() || (kind == EventKind::Ctcp && !is_ctcp(message.get())) {
            return;
        }

        let Some(event) = kind.event(message.shared()) else {
            return;
        };

        Self::deliver(&event, subscribers, stopped);
    }

    /// Delivers one event to every subscriber of a list.
    fn deliver(event: &Event, subscribers: &[Subscriber], stopped: &mut Vec<&'static str>) {
        for subscriber in subscribers {
            subscriber.send(event.clone(), stopped);
        }
    }

    /// Routes a `PRIVMSG` to the plugins whose subscriptions match.
    fn dispatch_privmsg(
        &self,
        filters: &Filters,
        message: &mut SharedMessage,
        stopped: &mut Vec<&'static str>,
    ) {
        let Command::PRIVMSG(target, text) = &message.get().command else {
            return;
        };

        // CTCP messages are their own event kind and are routed to nothing else.
        if text.starts_with('\x01') {
            self.deliver_kind(EventKind::Ctcp, message, stopped);

            return;
        }

        let sender = Sender::from_message(message.get());

        // Senders matching a filter without URL criteria are ignored entirely: their commands
        // do not trigger, their messages are not delivered to message subscribers, and their
        // URLs are not dispatched.
        if filters.ignores(target, sender) {
            debug!("ignoring filtered sender");

            return;
        }

        // Commands — matched by the first word of the message, verified with the exact
        // trigger semantics. Leading whitespace makes the first word non-matching, and checking
        // it before sharing keeps such a message allocation-free.
        if let Some(word) = text.split_whitespace().next()
            && text.starts_with(word)
            && let Some(targets) = self.commands.get(word)
        {
            let message = message.shared();

            for target in targets {
                if let Some(event) = CommandEvent::new(Arc::clone(&message), target.spec) {
                    target.subscriber.send(Event::Command(event), stopped);
                }
            }
        }

        // Message subscribers observe every remaining non-CTCP message.
        self.deliver_kind(EventKind::Message, message, stopped);

        // URLs — extracted once per message, deduplicated and filtered before routing by host.
        if self.url_hosts.is_empty() && self.url_any.is_empty() {
            return;
        }

        let Command::PRIVMSG(target, text) = &message.get().command else {
            return;
        };
        let sender = Sender::from_message(message.get());

        // The retained URLs are collected before the message is shared, so a `PRIVMSG` that
        // carries no URL (or only filtered ones) still does not allocate the shared message.
        let mut seen: SmallVec<[(Arc<Url>, Option<&'static str>); 4]> = SmallVec::new();

        // Deduplication shares the parsed URL with the event instead of hashing a deep copy
        // into a set; a message rarely carries more than a handful of URLs.
        for ExtractedUrl { url, repaired_from } in ExtractUrls::with_schemes(text, HTTP_SCHEMES) {
            let url = Arc::new(url);

            if seen.iter().any(|(seen, _)| seen.as_str() == url.as_str()) {
                continue;
            }

            if filters.is_filtered(target, sender, &url) {
                debug!(url.full = %RedactedUrl::from_url(&url), "skipping filtered url");

                continue;
            }

            // A URL nobody subscribed to is not an event; drop it before sharing the message.
            let claimed = !self.url_any.is_empty()
                || url
                    .host_str()
                    .is_some_and(|host| self.url_hosts.contains_key(host));

            if !claimed {
                continue;
            }

            seen.push((url, repaired_from));
        }

        if seen.is_empty() {
            return;
        }

        let message = message.shared();

        for (url, repaired_from) in seen {
            let event = UrlEvent::new(Arc::clone(&message), url, repaired_from);

            if let Some(host) = event.url().host_str()
                && let Some(subscribers) = self.url_hosts.get(host)
            {
                for subscriber in subscribers {
                    subscriber.send(Event::Url(event.clone()), stopped);
                }
            }

            for subscriber in &self.url_any {
                subscriber.send(Event::Url(event.clone()), stopped);
            }
        }
    }
}

/// Whether `message` carries a CTCP payload: a `PRIVMSG` or `NOTICE` whose text is
/// `\x01`-wrapped.
fn is_ctcp(message: &Message) -> bool {
    matches!(
        &message.command,
        Command::PRIVMSG(_, text) | Command::NOTICE(_, text) if text.starts_with('\x01')
    )
}

#[cfg(test)]
mod tests {
    use std::alloc::System;
    use std::mem::size_of;
    use std::sync::{Arc, Mutex, OnceLock};

    use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};
    use tracing::Subscriber;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id};
    use tracing_subscriber::layer::{Context, Layer};
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::registry::LookupSpan;
    use url::Url;
    use zeta_plugin::CtcpKind;
    use zeta_plugin::UrlFilter;
    use zeta_test_support::assert_empty;

    use super::*;

    // The allocation test needs to observe the global heap. A safe allocator implementation does
    // not require `unsafe` here; the counters are process-wide, so the test runs alone.
    #[global_allocator]
    static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

    /// Measures the allocations and requested bytes of one `dispatch` call.
    fn measure(label: &str, index: &mut EventIndex, filters: &Filters, message: Message) -> Stats {
        let mut region = Region::new(GLOBAL);

        let _ = index.dispatch(filters, message);

        let stats = region.change_and_reset();

        println!(
            "dispatch_allocations: {label}: allocations={} bytes_allocated={}",
            stats.allocations, stats.bytes_allocated
        );

        stats
    }

    /// Dispatching a message that produces no event must not touch the heap.
    ///
    /// The allocator counters are process-wide, so this test is ignored by default and must be
    /// run alone:
    ///
    /// `cargo test -p zeta-plugin-runtime --lib --all-features -- --ignored --test-threads=1 dispatch_without_events`
    #[test]
    #[ignore = "process-wide allocator counters; run with --ignored --test-threads=1"]
    fn dispatch_without_events_does_not_allocate() {
        println!(
            "sizes: EventIndex={} Message={} Arc<Message>={} ArcInner<Message>-equivalent={}",
            size_of::<EventIndex>(),
            size_of::<Message>(),
            size_of::<Arc<Message>>(),
            size_of::<Message>() + 2 * size_of::<usize>(),
        );

        let filters = Filters::default();
        let mut index = EventIndex::default();

        let unhandled = measure("unhandled privmsg", &mut index, &filters, privmsg("hello"));
        let numeric = measure(
            "numeric 001",
            &mut index,
            &filters,
            message("server.example", "001", &["zeta", "Welcome"]),
        );
        let join = measure(
            "join",
            &mut index,
            &filters,
            message("nick!u@h", "JOIN", &["#test"]),
        );

        let mut ctcp_index = EventIndex::default();
        let _ctcp = subscribe(&mut ctcp_index, "ctcp", |subscriptions| {
            subscriptions.receive_ctcp();
        });
        let plain_notice = measure(
            "plain notice",
            &mut ctcp_index,
            &filters,
            message("server.example", "NOTICE", &["zeta", "maintenance"]),
        );

        let mut url_index = EventIndex::default();
        let _titles = subscribe(&mut url_index, "titles", |subscriptions| {
            subscriptions.urls(UrlScope::Any);
        });
        let urlless_privmsg = measure(
            "url-less privmsg",
            &mut url_index,
            &filters,
            privmsg("no link here"),
        );

        for (label, stats) in [
            ("unhandled privmsg", unhandled),
            ("numeric 001", numeric),
            ("join", join),
            ("plain notice", plain_notice),
            ("url-less privmsg", urlless_privmsg),
        ] {
            assert_eq!(stats.allocations, 0, "{label} allocated");
            assert_eq!(stats.bytes_allocated, 0, "{label} allocated bytes");
        }
    }

    /// Collects every event a test subscriber received from its mailbox.
    fn drain(mailbox: &mut mpsc::UnboundedReceiver<Envelope>) -> Vec<Event> {
        let mut events = Vec::new();

        while let Ok(envelope) = mailbox.try_recv() {
            events.push(envelope.event);
        }

        events
    }

    fn message(prefix: &str, command: &str, args: &[&str]) -> Message {
        let prefix = (!prefix.is_empty()).then_some(prefix);

        Message::new(prefix, command, args.to_vec()).expect("message should parse")
    }

    fn privmsg(text: &str) -> Message {
        privmsg_from("nick", text)
    }

    fn privmsg_from(nick: &str, text: &str) -> Message {
        message(
            &format!("{nick}!user@host.example"),
            "PRIVMSG",
            &["#test", text],
        )
    }

    /// Registers a test subscriber and returns the mailbox its events arrive on.
    fn subscribe(
        index: &mut EventIndex,
        name: &'static str,
        register: impl FnOnce(&mut Subscriptions),
    ) -> mpsc::UnboundedReceiver<Envelope> {
        let mut subscriptions = Subscriptions::new();
        register(&mut subscriptions);

        let (mailbox, receiver) = mpsc::unbounded_channel();
        index.add(name, &subscriptions, mailbox);

        receiver
    }

    #[test]
    fn registered_command_is_dispatched() {
        let mut index = EventIndex::default();
        let mut mailbox = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig"));
        });

        let stopped = index.dispatch(&Filters::default(), privmsg(".dig example.com"));

        assert_empty!(stopped);
        assert!(matches!(
            drain(&mut mailbox)[..],
            [Event::Command(ref command)] if command.args() == "example.com"
        ));
    }

    #[test]
    fn command_is_matched_by_first_word() {
        let mut index = EventIndex::default();
        let mut mailbox = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig"));
        });

        // Longer words are not matched, and leading whitespace breaks the first word.
        for text in [".digg example.com", "  .dig example.com", "look at .dig"] {
            let _ = index.dispatch(&Filters::default(), privmsg(text));

            assert_empty!(drain(&mut mailbox), "`{text}` must not dispatch");
        }
    }

    #[test]
    fn multiple_registered_commands_dispatch_independently() {
        let mut index = EventIndex::default();
        let mut kagi = subscribe(&mut index, "kagi", |subscriptions| {
            subscriptions
                .command(CommandSpec::new(".g", "search"))
                .command(CommandSpec::new(".gis", "images"));
        });

        let _ = index.dispatch(&Filters::default(), privmsg(".gis black cats"));

        assert!(matches!(
            drain(&mut kagi)[..],
            [Event::Command(ref command)] if command.spec.trigger() == ".gis"
        ));
    }

    #[test]
    fn urls_route_to_registered_hosts_and_generic_handlers() {
        let mut index = EventIndex::default();
        let mut youtube = subscribe(&mut index, "youtube", |subscriptions| {
            subscriptions.urls(UrlScope::Hosts(&["youtube.com", "www.youtube.com"]));
        });
        let mut titles = subscribe(&mut index, "titles", |s| {
            s.urls(UrlScope::Any);
        });

        // A claimed host is delivered to its plugin and to generic handlers.
        let _ = index.dispatch(
            &Filters::default(),
            privmsg("https://youtube.com/watch?v=1"),
        );

        assert!(matches!(drain(&mut youtube)[..], [Event::Url(..)]));
        assert!(matches!(drain(&mut titles)[..], [Event::Url(..)]));

        // Host matching is case-insensitive.
        let _ = index.dispatch(&Filters::default(), privmsg("https://WWW.YOUTUBE.COM/x"));

        assert_eq!(drain(&mut youtube).len(), 1);
        assert_eq!(drain(&mut titles).len(), 1);

        // An unclaimed host only reaches generic handlers.
        let _ = index.dispatch(&Filters::default(), privmsg("https://example.com/x"));

        assert_empty!(drain(&mut youtube));
        assert_eq!(drain(&mut titles).len(), 1);
    }

    #[test]
    fn multiple_urls_yield_one_event_each() {
        let mut index = EventIndex::default();
        let mut titles = subscribe(&mut index, "titles", |s| {
            s.urls(UrlScope::Any);
        });

        let _ = index.dispatch(
            &Filters::default(),
            privmsg("https://a.example/x https://a.example/x https://b.example/y"),
        );

        assert_eq!(
            drain(&mut titles).len(),
            2,
            "duplicate URLs are deduplicated"
        );
    }

    #[test]
    fn url_hosts_are_lowercased_by_the_url_parser() {
        let mut index = EventIndex::default();
        let mut ipv6 = subscribe(&mut index, "ipv6", |subscriptions| {
            subscriptions.urls(UrlScope::Hosts(&["[2001:db8::1]"]));
        });

        // The registered host is lowercased, and `Url::host_str` already returns the parsed,
        // normalized host — hexadecimal IPv6 digits included — so the lookup needs no copy.
        let _ = index.dispatch(&Filters::default(), privmsg("https://[2001:DB8::1]/x"));

        assert_eq!(drain(&mut ipv6).len(), 1);
    }

    /// A filter that ignores the `ignored` nick entirely and matches every URL on the
    /// `filtered.example` host.
    struct TestFilter;

    impl UrlFilter for TestFilter {
        fn is_filtered(&self, _channel: &str, _sender: Option<Sender<'_>>, url: &Url) -> bool {
            url.host_str() == Some("filtered.example")
        }

        fn ignores(&self, _channel: &str, sender: Option<Sender<'_>>) -> bool {
            sender.is_some_and(|sender| sender.nick == "ignored")
        }
    }
    #[tokio::test]
    async fn filtered_urls_are_not_dispatched() {
        // The facade only sees the filter through the type-erased handle published to the
        // shared state — the same route the filter plugin takes at construction.
        let ctx = zeta_test_support::test_context();
        ctx.shared
            .publish(Arc::new(Filters::new(Arc::new(TestFilter))));

        let mut index = EventIndex::default();
        let mut filtered = subscribe(&mut index, "filtered", |subscriptions| {
            subscriptions.urls(UrlScope::Hosts(&["filtered.example"]));
        });
        let mut titles = subscribe(&mut index, "titles", |s| {
            s.urls(UrlScope::Any);
        });

        let filters = Filters::from_context(&ctx);
        let _ = index.dispatch(
            &filters,
            privmsg("https://filtered.example/x https://open.example/y"),
        );

        assert_empty!(drain(&mut filtered));
        assert!(matches!(drain(&mut titles)[..], [Event::Url(..)]));
    }

    #[tokio::test]
    async fn ignored_senders_trigger_nothing() {
        let ctx = zeta_test_support::test_context();
        ctx.shared
            .publish(Arc::new(Filters::new(Arc::new(TestFilter))));

        let mut index = EventIndex::default();
        let mut dig = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig"));
        });
        let mut messages = subscribe(&mut index, "messages", |s| {
            s.receive_message();
        });
        let mut titles = subscribe(&mut index, "titles", |s| {
            s.urls(UrlScope::Any);
        });

        let filters = Filters::from_context(&ctx);

        // The ignored sender's commands do not trigger, their messages are not delivered and
        // their URLs are not dispatched.
        let _ = index.dispatch(&filters, privmsg_from("ignored", ".dig example.com"));
        let _ = index.dispatch(
            &filters,
            privmsg_from("ignored", "hello https://open.example/y"),
        );

        assert_empty!(drain(&mut dig));
        assert_empty!(drain(&mut messages));
        assert_empty!(drain(&mut titles));

        // Everyone else is dispatched normally.
        let _ = index.dispatch(&filters, privmsg_from("someone", ".dig example.com"));

        assert!(matches!(
            drain(&mut dig)[..],
            [Event::Command(ref command)] if command.args() == "example.com"
        ));
        assert!(matches!(drain(&mut messages)[..], [Event::Message(_)]));
    }

    #[test]
    fn ctcp_messages_bypass_everything_else() {
        let mut index = EventIndex::default();
        let mut ctcp = subscribe(&mut index, "ctcp", |s| {
            s.receive_ctcp();
        });
        let mut messages = subscribe(&mut index, "messages", |s| {
            s.receive_message();
        });
        let mut dig = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig"));
        });

        let _ = index.dispatch(&Filters::default(), privmsg("\x01ACTION slaps .dig\x01"));

        assert!(matches!(
            drain(&mut ctcp)[..],
            [Event::Ctcp(ref event)] if event.kind == CtcpKind::Action
        ));
        assert_empty!(drain(&mut messages));
        assert_empty!(drain(&mut dig));
    }

    #[test]
    fn ctcp_replies_arrive_through_notices() {
        let mut index = EventIndex::default();
        let mut ctcp = subscribe(&mut index, "ctcp", |s| {
            s.receive_ctcp();
        });
        let mut watcher = subscribe(&mut index, "watcher", |s| {
            s.receive_message();
        });

        let _ = index.dispatch(
            &Filters::default(),
            message(
                "service.example",
                "NOTICE",
                &["zeta", "\x01VERSION 1.0\x01"],
            ),
        );

        assert!(matches!(
            drain(&mut ctcp)[..],
            [Event::Ctcp(ref event)] if event.kind == CtcpKind::Version
        ));
        assert_empty!(drain(&mut watcher));
    }

    #[test]
    fn plain_notices_are_not_delivered() {
        let mut index = EventIndex::default();
        let mut ctcp = subscribe(&mut index, "ctcp", |s| {
            s.receive_ctcp();
        });

        let _ = index.dispatch(
            &Filters::default(),
            message("server.example", "NOTICE", &["zeta", "server maintenance"]),
        );

        assert_empty!(drain(&mut ctcp));
    }

    #[test]
    fn messages_reach_subscribers_regardless_of_matches() {
        let mut index = EventIndex::default();
        let mut watcher = subscribe(&mut index, "watcher", |s| {
            s.receive_message();
        });
        let mut dig = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig"));
        });

        let _ = index.dispatch(&Filters::default(), privmsg(".dig example.com"));

        assert!(matches!(drain(&mut dig)[..], [Event::Command(..)]));
        assert!(matches!(drain(&mut watcher)[..], [Event::Message(..)]));

        let _ = index.dispatch(&Filters::default(), privmsg("no events at all?"));

        assert_empty!(drain(&mut dig));
        assert_eq!(drain(&mut watcher).len(), 1);
    }

    #[test]
    fn presence_events_route_by_kind() {
        let mut index = EventIndex::default();
        let mut joins = subscribe(&mut index, "joins", |s| {
            s.receive_join();
        });
        let mut quits = subscribe(&mut index, "quits", |s| {
            s.receive_quit();
        });
        let mut parts = subscribe(&mut index, "parts", |s| {
            s.receive_part();
        });

        let _ = index.dispatch(&Filters::default(), message("nick!u@h", "JOIN", &["#test"]));
        let _ = index.dispatch(&Filters::default(), message("nick!u@h", "QUIT", &["gone"]));

        assert!(matches!(
            drain(&mut joins)[..],
            [Event::Join(ref join)] if join.channel() == "#test"
        ));
        assert!(matches!(
            drain(&mut quits)[..],
            [Event::Quit(ref quit)] if quit.reason() == Some("gone")
        ));
        assert_empty!(drain(&mut parts));
    }

    #[test]
    fn unmodeled_commands_reach_raw_subscribers() {
        let mut index = EventIndex::default();
        let mut raw = subscribe(&mut index, "raw", |s| {
            s.receive_raw();
        });

        let _ = index.dispatch(
            &Filters::default(),
            message("server.example", "SOMECMD", &["#test", "data"]),
        );

        assert!(matches!(
            drain(&mut raw)[..],
            [Event::Raw(ref event)] if event.command() == "SOMECMD"
        ));
    }

    /// Every flag-based event kind must have a dispatch arm: a kind added to the subscription
    /// table without routing would otherwise compile and never be delivered.
    #[test]
    fn every_event_kind_is_dispatched() {
        let cases: &[(EventKind, Message)] = &[
            (EventKind::Message, privmsg("hello")),
            (EventKind::Join, message("nick!u@h", "JOIN", &["#test"])),
            (EventKind::Part, message("nick!u@h", "PART", &["#test"])),
            (EventKind::Quit, message("nick!u@h", "QUIT", &["bye"])),
            (EventKind::Nick, message("nick!u@h", "NICK", &["nick_"])),
            (
                EventKind::Kick,
                message("op!u@h", "KICK", &["#test", "nick", "flood"]),
            ),
            (EventKind::Ctcp, privmsg("\x01VERSION\x01")),
            (
                EventKind::Raw,
                message("server.example", "SOMECMD", &["data"]),
            ),
        ];

        assert_eq!(cases.len(), EventKind::ALL.len());

        for (kind, message) in cases {
            let mut index = EventIndex::default();
            let mut mailbox = subscribe(&mut index, "sub", |subscriptions| match kind {
                EventKind::Message => {
                    subscriptions.receive_message();
                }
                EventKind::Join => {
                    subscriptions.receive_join();
                }
                EventKind::Part => {
                    subscriptions.receive_part();
                }
                EventKind::Quit => {
                    subscriptions.receive_quit();
                }
                EventKind::Nick => {
                    subscriptions.receive_nick();
                }
                EventKind::Kick => {
                    subscriptions.receive_kick();
                }
                EventKind::Ctcp => {
                    subscriptions.receive_ctcp();
                }
                EventKind::Raw => {
                    subscriptions.receive_raw();
                }
            });

            let _ = index.dispatch(&Filters::default(), message.clone());

            let events = drain(&mut mailbox);

            assert_eq!(events.len(), 1, "{kind:?} was not dispatched exactly once");
            assert_eq!(events[0].kind(), Some(*kind), "{kind:?} was not dispatched");
        }
    }

    #[test]
    fn stopped_plugins_are_evicted() {
        let mut index = EventIndex::default();
        let dig = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions
                .command(CommandSpec::new(".dig", "dig"))
                .receive_join();
        });

        drop(dig);

        let stopped = index.dispatch(&Filters::default(), privmsg(".dig example.com"));
        assert_eq!(stopped, ["dig"]);

        // The evicted plugin receives nothing further.
        let stopped = index.dispatch(&Filters::default(), message("n!u@h", "JOIN", &["#test"]));

        assert_empty!(stopped);
    }

    #[test]
    fn stopped_plugins_are_reported_once_per_message() {
        let mut index = EventIndex::default();
        let dig = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions
                .command(CommandSpec::new(".dig", "dig"))
                .urls(UrlScope::Any)
                .receive_message();
        });

        drop(dig);

        // The message matches the command, the message subscription, and the URL
        // subscription; the stopped plugin is still reported only once.
        let stopped = index.dispatch(&Filters::default(), privmsg(".dig https://example.com/x"));

        assert_eq!(stopped, ["dig"]);
    }

    /// The precomputed handler span names must follow the event kinds: the array is ordered
    /// like [`EventKind::ALL`] while the composite strings are written out by hand.
    #[test]
    fn span_names_match_the_event_kinds() {
        let names = SpanNames::new("dig");
        let message = Arc::new(privmsg(".dig example.com"));
        let command = CommandEvent::new(Arc::clone(&message), CommandSpec::new(".dig", "dig"))
            .expect("command should match");
        let url = UrlEvent::new(
            Arc::clone(&message),
            Arc::new("https://example.com/".parse().expect("url")),
            None,
        );

        assert_eq!(
            names.event(&Event::Command(command)),
            "zeta.event.handle dig command"
        );
        assert_eq!(names.event(&Event::Url(url)), "zeta.event.handle dig url");

        for kind in EventKind::ALL {
            // Only a CTCP event is conditional on the message; the other kinds wrap any of them.
            let message = match kind {
                EventKind::Ctcp => privmsg("\x01VERSION\x01"),
                _ => privmsg("hello"),
            };
            let event = kind
                .event(Arc::new(message))
                .expect("every flag-based kind builds an event from its message");

            assert_eq!(
                names.event(&event),
                format!("zeta.event.handle dig {}", event.kind_name())
            );
        }
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

    /// Makes `tracing` consider every callsite enabled for the rest of the test process.
    ///
    /// `tracing` caches one interest value per callsite for the whole process, computed from the
    /// dispatcher that was current when the callsite was first used. The other dispatch tests run
    /// without a subscriber, so `zeta.event.handle` can already be cached as disabled before this
    /// test installs its recorder; a thread-local subscriber cannot repair that cache. A global
    /// no-op subscriber makes every registration and interest rebuild resolve to "enabled".
    ///
    /// The recorder itself stays thread-local, so the recorded spans are this test's only.
    fn install_global_tracing_subscriber() {
        static INSTALL: OnceLock<()> = OnceLock::new();

        INSTALL.get_or_init(|| {
            // Another global subscriber is fine as long as it is enabled; the test only needs
            // the cache to stop reporting the callsites as disabled.
            let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
            tracing::callsite::rebuild_interest_cache();
        });
    }

    /// The handler span is created by the dispatcher while the IRC line's dispatch span is
    /// current; it must therefore be a child of that span, not a new trace root. The plugin
    /// task relies on this when it instruments the handler with the envelope's span.
    #[test]
    fn handler_span_is_a_child_of_the_dispatch_span() {
        install_global_tracing_subscriber();

        let recorder = SpanRecorder::default();
        let subscriber = tracing_subscriber::registry().with(recorder.clone());

        tracing::subscriber::with_default(subscriber, || {
            // The callsite may have been cached before this subscriber existed; re-evaluate it.
            tracing::callsite::rebuild_interest_cache();

            let dispatch = tracing::info_span!("irc.dispatch");
            let dispatch_id = dispatch.id().expect("the dispatch span should be enabled");
            let _entered = dispatch.enter();

            let mut index = EventIndex::default();
            let mut mailbox = subscribe(&mut index, "dig", |subscriptions| {
                subscriptions.command(CommandSpec::new(".dig", "dig"));
            });

            let stopped = index.dispatch(&Filters::default(), privmsg(".dig example.com"));
            assert_empty!(stopped);

            let envelope = mailbox.try_recv().expect("the event should be dispatched");
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
            // The exported name carries the plugin and event kind, so the span is identifiable
            // in the trace view without opening it.
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

    /// Prints the per-dispatch cost of representative messages against a small index.
    ///
    /// The numbers are only comparable with another run in the same profile (debug by default):
    ///
    /// `cargo test -p zeta-plugin-runtime --lib --all-features -- --ignored --nocapture dispatch_benchmark`
    #[test]
    #[ignore = "benchmark; run with --ignored --nocapture"]
    fn dispatch_benchmark() {
        const DISPATCHES: usize = 100_000;

        let mut index = EventIndex::default();
        let _dig = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig"));
        });
        let _joiner = subscribe(&mut index, "joiner", |subscriptions| {
            subscriptions.receive_join();
        });
        let _ctcp = subscribe(&mut index, "ctcp", |subscriptions| {
            subscriptions.receive_ctcp();
        });

        let filters = Filters::default();

        benchmark(
            "unhandled privmsg",
            DISPATCHES,
            &mut index,
            &filters,
            || privmsg("hello there"),
        );
        benchmark("handled command", DISPATCHES, &mut index, &filters, || {
            privmsg(".dig example.com")
        });
        benchmark("join", DISPATCHES, &mut index, &filters, || {
            message("nick!u@h", "JOIN", &["#test"])
        });
    }

    /// Times `dispatches` calls to [`EventIndex::dispatch`] with pre-built messages.
    ///
    /// The messages are built before the timer starts, so the measurement covers dispatch only.
    fn benchmark(
        label: &str,
        dispatches: usize,
        index: &mut EventIndex,
        filters: &Filters,
        make: impl Fn() -> Message,
    ) {
        let messages: Vec<Message> = (0..dispatches).map(|_| make()).collect();
        let start = std::time::Instant::now();

        for message in messages {
            let stopped = index.dispatch(filters, message);
            std::hint::black_box(&stopped);
        }

        let elapsed = start.elapsed();
        let count = f64::from(u32::try_from(dispatches).expect("dispatch count fits in u32"));
        let ns_per_dispatch = elapsed.as_secs_f64() * 1_000_000_000.0 / count;

        println!(
            "dispatch_benchmark: {label}: {dispatches} dispatches in {elapsed:?} \
             ({ns_per_dispatch:.1} ns/dispatch)"
        );
    }
}
