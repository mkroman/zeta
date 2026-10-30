//! Event dispatching: routing incoming IRC messages to the plugins that registered interest.
//!
//! The [`EventIndex`] maps the subscriptions every plugin declared during initialization to
//! delivery endpoints. It matches incoming IRC messages against the index — commands by their
//! trigger through a hash lookup on the message's first word, URLs by their host through a
//! hash lookup, and the remaining event kinds through subscriber lists — and queues the
//! matching events on each plugin's mailbox. A plugin is only woken for events it asked for;
//! for the indexed kinds the lookup cost is independent of the number of plugins.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use irc::proto::{Command, Message};
use tokio::sync::mpsc;
use tracing::{Span, debug, warn};
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

/// A delivery endpoint for events: the mailbox of one plugin's task.
#[derive(Clone, Debug)]
struct Subscriber {
    /// The plugin's name, used for logging.
    name: &'static str,
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
        // The exported name carries the bounded discriminator — plugin and event kind — so a
        // trace with one handler span per subscriber stays readable without opening each span.
        // The `tracing` name stays `zeta.event.handle` for filtering and metrics.
        let name = format!("zeta.event.handle {} {}", self.name, event.kind_name());

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

/// The dispatch index of every registered plugin's subscriptions.
#[derive(Default)]
pub struct EventIndex {
    /// Registered commands, indexed by their trigger — the first word of a message.
    commands: HashMap<&'static str, Vec<CommandTarget>>,
    /// Plugins subscribed to URLs on specific hosts, indexed by the lowercased host.
    url_hosts: HashMap<String, Vec<Subscriber>>,
    /// Plugins subscribed to every URL, including hosts other plugins handle.
    url_any: Vec<Subscriber>,
    /// Plugins subscribed to plain-interest event kinds, keyed by the kind.
    kinds: HashMap<EventKind, Vec<Subscriber>>,
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
        let subscriber = Subscriber { name, mailbox };

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
            self.kinds
                .entry(*kind)
                .or_default()
                .push(subscriber.clone());
        }
    }

    /// Routes `message` to the plugins that subscribed to events it matches.
    ///
    /// Returns the names of the plugins whose mailboxes are closed — their tasks have stopped.
    /// They are already evicted from the index, so the caller only needs to report them.
    #[must_use]
    pub fn dispatch(&mut self, filters: &Filters, message: Message) -> Vec<&'static str> {
        let message = Arc::new(message);
        let mut stopped = Vec::new();

        match &message.command {
            Command::PRIVMSG(..) => {
                self.dispatch_privmsg(filters, &message, &mut stopped);
            }
            // CTCP replies arrive as NOTICE-wrapped messages; only CTCP subscribers see them —
            // ordinary notices are not an event kind.
            Command::NOTICE(..) => {
                self.deliver_kind(EventKind::Ctcp, &message, &mut stopped);
            }
            Command::JOIN(..) => {
                self.deliver_kind(EventKind::Join, &message, &mut stopped);
            }
            Command::PART(..) => {
                self.deliver_kind(EventKind::Part, &message, &mut stopped);
            }
            Command::QUIT(..) => {
                self.deliver_kind(EventKind::Quit, &message, &mut stopped);
            }
            Command::NICK(..) => {
                self.deliver_kind(EventKind::Nick, &message, &mut stopped);
            }
            Command::KICK(..) => {
                self.deliver_kind(EventKind::Kick, &message, &mut stopped);
            }
            Command::Raw(..) => {
                self.deliver_kind(EventKind::Raw, &message, &mut stopped);
            }
            // Everything else is connection protocol the plugins have no events for.
            _ => {}
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

        self.kinds.retain(|_, subscribers| {
            subscribers.retain(|subscriber| other(subscriber));
            !subscribers.is_empty()
        });
    }

    /// Returns the subscriber list of a plain-interest event kind, if any are registered.
    fn subscribers(&self, kind: EventKind) -> Option<&[Subscriber]> {
        self.kinds.get(&kind).map(Vec::as_slice)
    }

    /// Delivers `message` as an event of `kind` to its subscribers, if any are registered.
    ///
    /// The event is built only when a subscriber exists: most messages match no subscription,
    /// and some kinds (CTCP) cannot be built from every message.
    fn deliver_kind(
        &self,
        kind: EventKind,
        message: &Arc<Message>,
        stopped: &mut Vec<&'static str>,
    ) {
        let Some(subscribers) = self.subscribers(kind) else {
            return;
        };
        let Some(event) = kind.event(Arc::clone(message)) else {
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
        message: &Arc<Message>,
        stopped: &mut Vec<&'static str>,
    ) {
        let Command::PRIVMSG(target, text) = &message.command else {
            return;
        };

        // CTCP messages are their own event kind and are routed to nothing else.
        if text.starts_with('\x01') {
            self.deliver_kind(EventKind::Ctcp, message, stopped);

            return;
        }

        let sender = Sender::from_message(message);

        // Senders matching a filter without URL criteria are ignored entirely: their commands
        // do not trigger, their messages are not delivered to message subscribers, and their
        // URLs are not dispatched.
        if filters.ignores(target, sender) {
            debug!("ignoring filtered sender");

            return;
        }

        // Commands — matched by the first word of the message, verified with the exact
        // trigger semantics.
        if let Some(word) = text.split_whitespace().next()
            && let Some(targets) = self.commands.get(word)
        {
            for target in targets {
                if let Some(event) = CommandEvent::new(Arc::clone(message), target.spec) {
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

        let mut seen = HashSet::new();

        for ExtractedUrl { url, repaired_from } in ExtractUrls::with_schemes(text, HTTP_SCHEMES) {
            if !seen.insert(url.clone()) {
                continue;
            }

            if filters.is_filtered(target, sender, &url) {
                debug!(url.full = %RedactedUrl::from_url(&url), "skipping filtered url");

                continue;
            }

            let event = UrlEvent::new(Arc::clone(message), Arc::new(url), repaired_from);

            if let Some(host) = event.url().host_str()
                && let Some(subscribers) = self.url_hosts.get(&host.to_ascii_lowercase())
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, OnceLock};

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
}
