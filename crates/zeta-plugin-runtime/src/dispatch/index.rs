//! The subscription index and the routing path that walks it.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use irc::proto::{Command, Message};
use smallvec::SmallVec;
use tokio::sync::mpsc;
use tracing::{Span, debug, warn};

use zeta_plugin::event::{CommandEvent, Sender, Subscriptions, UrlEvent};
use zeta_plugin::{Event, EventKind, FilterHandle, UrlScope};
use zeta_url::{ExtractUrls, ExtractedUrl, HTTP_SCHEMES, RedactedUrlRef};

use crate::telemetry::{MessageTelemetry, dispatch_span, handler_spans_enabled};

use super::command::{CommandRoute, CommandTarget};
use super::envelope::{Envelope, SharedMessage};
use super::plan::DeliveryPlan;
use super::subscriber::{DeliveryTelemetry, Subscriber};
use super::url::UrlDispatch;
use super::{telemetry_enabled, trace_raw};

/// A hash map for the per-message dispatch lookups, keyed by command trigger and URL host.
///
/// `foldhash::fast` hashes these short keys faster than the std hasher. It is not
/// DoS-resistant: every key is inserted from plugin declarations, and message text is only
/// ever used to look an existing key up.
type IndexMap<K, V> = HashMap<K, V, foldhash::fast::RandomState>;

/// The dispatch index of every registered plugin's subscriptions.
#[derive(Default)]
pub struct EventIndex {
    /// Registered commands, indexed by their trigger — the first word of a message.
    pub(super) commands: IndexMap<&'static str, Vec<CommandTarget>>,
    /// A bit per possible first byte of a command trigger, offset by `b' '`.
    ///
    /// Non-text bytes cannot start a command, so a message whose first byte has no bit set is
    /// rejected before the first-word split and the command hash lookup. Stale bits left behind
    /// by evicted plugins only cause a lookup that finds nothing.
    command_initials: u128,
    /// Whether any registered command trigger starts with a byte outside the ASCII bitmap.
    ///
    /// Stale `true` left behind by an evicted plugin only causes a lookup that finds nothing,
    /// like a stale bitmap bit.
    command_initials_non_ascii: bool,
    /// Plugins subscribed to URLs on specific hosts, indexed by the lowercased host.
    pub(super) url_hosts: IndexMap<String, Vec<Subscriber>>,
    /// Plugins subscribed to every URL, including hosts other plugins handle.
    pub(super) url_any: Vec<Subscriber>,
    /// Plugins subscribed to plain-interest event kinds, indexed by [`EventKind::index`].
    kinds: [Vec<Subscriber>; EventKind::ALL.len()],
}

// Manual implementation: the index holds plugin mailboxes, so the routing tables are
// summarized by their sizes instead of exposing the subscribers.
impl fmt::Debug for EventIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventIndex")
            .field("commands", &self.commands.len())
            .field("url_hosts", &self.url_hosts.len())
            .field("url_any", &self.url_any.len())
            .field(
                "kind_subscribers",
                &self.kinds.iter().map(Vec::len).sum::<usize>(),
            )
            .finish_non_exhaustive()
    }
}

impl EventIndex {
    /// Adds a plugin to the index with the `subscriptions` it declared and its `mailbox`.
    ///
    /// Empty command triggers, and triggers containing whitespace, cannot be matched by the
    /// first-word index and are skipped with a warning.
    pub fn add(
        &mut self,
        name: &'static str,
        subscriptions: &Subscriptions,
        mailbox: mpsc::Sender<Envelope>,
    ) {
        let subscriber = Subscriber::new(name, mailbox);

        for command in subscriptions.commands() {
            if command.trigger().is_empty() || command.trigger().contains(char::is_whitespace) {
                warn!(
                    zeta.plugin.name = %name,
                    zeta.plugin.command_trigger = command.trigger(),
                    "command trigger is empty or contains whitespace; it cannot be dispatched"
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

            match command.trigger().as_bytes().first() {
                Some(&byte) if (b' '..0x80).contains(&byte) => {
                    self.command_initials |= 1 << (byte - b' ');
                }
                Some(_) => self.command_initials_non_ascii = true,
                None => {}
            }
        }

        match subscriptions.url_scope() {
            UrlScope::Hosts(hosts) => {
                for host in hosts {
                    self.url_hosts
                        .entry((*host).to_ascii_lowercase())
                        .or_default()
                        .push(subscriber.clone());
                }
            }
            UrlScope::Any => self.url_any.push(subscriber.clone()),
            // `None`, and scopes added after this runtime was written, are not routed; a
            // plugin using a newer scope needs a runtime that understands it.
            _ => {}
        }

        for kind in subscriptions.events() {
            self.kinds[kind.index()].push(subscriber.clone());
        }
    }

    /// Routes `message` to the plugins that subscribed to events it matches.
    ///
    /// The `irc.dispatch` span of the line is created and entered here, before routing, so every
    /// handler span is its child and the raw line and the stopped-plugin warnings are logged
    /// inside it. Lines no plugin can receive get no span at all
    /// ([`DeliveryPlan::may_deliver`]).
    ///
    /// Returns the names of the plugins whose mailboxes are closed — their tasks have stopped.
    /// They are logged and evicted before this returns, so the caller may ignore the list.
    #[must_use]
    pub fn dispatch(&mut self, filters: &FilterHandle, message: Message) -> Vec<&'static str> {
        let mut message = SharedMessage::new(message);
        let route = CommandRoute::classify(&message.message().command);
        let plan = self.plan(route, message.message());

        if !plan.may_deliver() {
            trace_raw(message.message());

            return Vec::new();
        }

        // The line span is built from the still-unshared message. Its telemetry borrows the
        // message, so the whole computation stays in this block and is dropped before routing
        // shares the message. Sharing eagerly would allocate on every delivering line,
        // including the ones no route ends up delivering to.
        let enabled = handler_spans_enabled();
        let dispatch = {
            let telemetry = if enabled {
                MessageTelemetry::from_route(route, message.message())
            } else {
                None
            };

            telemetry.as_ref().map(dispatch_span)
        };

        // The gate is already known; the delivery points reuse it instead of consulting the
        // filter again.
        let mut telemetry_enabled = Some(enabled);
        let _entered = dispatch.as_ref().map(Span::enter);

        trace_raw(message.message());

        let mut stopped = Vec::new();

        match route {
            CommandRoute::Privmsg => {
                self.dispatch_privmsg(
                    filters,
                    route,
                    plan,
                    &mut telemetry_enabled,
                    &mut message,
                    &mut stopped,
                );
            }
            CommandRoute::Event(kind) => {
                self.deliver_kind(
                    kind,
                    route,
                    &mut telemetry_enabled,
                    &mut message,
                    &mut stopped,
                );
            }
            CommandRoute::Ignored => {}
        }

        // A stopped plugin can match several of the message's events (and several commands or
        // URLs); report and evict it once. The warning runs inside the line span, like every
        // other message routed here.
        if !stopped.is_empty() {
            stopped.sort_unstable();
            stopped.dedup();

            for name in &stopped {
                warn!(zeta.plugin.name = %name, "plugin task has stopped");
            }

            self.evict_stopped(&stopped);
        }

        stopped
    }

    /// Builds the per-line dispatch plan for `message` under `route`.
    #[inline]
    pub(super) fn plan<'a>(&'a self, route: CommandRoute, message: &Message) -> DeliveryPlan<'a> {
        DeliveryPlan::new(self, route, message)
    }

    /// Returns the registered command targets the first word of `text` matches.
    ///
    /// The first word must be an exact trigger, and leading whitespace makes the first word
    /// non-matching; this is the command branch of [`Self::dispatch_privmsg`] with the targets
    /// kept for the caller.
    #[inline]
    pub(super) fn matched_command_targets(&self, text: &str) -> Option<&Vec<CommandTarget>> {
        if !self.may_start_command(text) {
            return None;
        }

        let word = text.split_whitespace().next()?;

        if !text.starts_with(word) {
            return None;
        }

        self.commands.get(word)
    }

    /// Removes the named stopped plugins from the index.
    ///
    /// Outlined from [`Self::dispatch`]: it is the rare path, and the eviction walks every
    /// subscription map, which is not worth inlining into every dispatched line.
    #[cold]
    #[inline(never)]
    fn evict_stopped(&mut self, stopped: &[&'static str]) {
        for name in stopped {
            self.evict(name);
        }
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
    pub(super) fn subscribers(&self, kind: EventKind) -> &[Subscriber] {
        &self.kinds[kind.index()]
    }

    /// Delivers `message` as an event of `kind` to its subscribers.
    ///
    /// The delivery plan has already established that a subscriber exists and, for CTCP, that
    /// the message carries CTCP text, so this only builds the event and telemetry.
    #[inline]
    fn deliver_kind(
        &self,
        kind: EventKind,
        route: CommandRoute,
        cache: &mut Option<bool>,
        message: &mut SharedMessage,
        stopped: &mut Vec<&'static str>,
    ) {
        let subscribers = self.subscribers(kind);

        let Some(event) = kind.event(message.shared()) else {
            return;
        };

        // Computed once for the whole batch; the message stays shared, so borrowing it for the
        // telemetry needs no extra handle.
        let telemetry = telemetry_enabled(cache)
            .then(|| MessageTelemetry::from_route(route, message.message()))
            .flatten();
        let telemetry = telemetry
            .as_ref()
            .map(|telemetry| DeliveryTelemetry::for_kind(kind, telemetry));

        Self::deliver(&event, telemetry.as_ref(), subscribers, stopped);
    }

    /// Delivers one event to every subscriber of a list.
    ///
    /// The delivery telemetry depends only on the line and the already-known kind, so it is
    /// computed once and shared by the subscriber list instead of re-deriving the kind per
    /// plugin.
    fn deliver(
        event: &Event,
        telemetry: Option<&DeliveryTelemetry<'_>>,
        subscribers: &[Subscriber],
        stopped: &mut Vec<&'static str>,
    ) {
        for subscriber in subscribers {
            subscriber.send(event.clone(), telemetry, stopped);
        }
    }

    /// Routes a `PRIVMSG` to the plugins whose subscriptions match.
    ///
    /// The plan already parsed the CTCP payload and matched the command triggers, so this only
    /// applies the sender filter and delivers the planned events.
    fn dispatch_privmsg(
        &self,
        filters: &FilterHandle,
        route: CommandRoute,
        plan: DeliveryPlan<'_>,
        cache: &mut Option<bool>,
        message: &mut SharedMessage,
        stopped: &mut Vec<&'static str>,
    ) {
        let Command::PRIVMSG(target, _) = &message.message().command else {
            return;
        };

        // CTCP messages are their own event kind and are routed to nothing else.
        if plan.is_ctcp() {
            self.deliver_kind(EventKind::Ctcp, route, cache, message, stopped);

            return;
        }

        let sender = Sender::from_message(message.message());

        // Senders matching a filter without URL criteria are ignored entirely: their commands
        // do not trigger, their messages are not delivered to message subscribers, and their
        // URLs are not dispatched.
        if filters.ignores(target, sender) {
            debug!("ignoring filtered sender");

            return;
        }

        // Commands — matched by the first word of the message, verified with the exact
        // trigger semantics. The message is shared only when a target exists.
        if let Some(targets) = plan.command_targets {
            let message = message.shared();
            let telemetry = telemetry_enabled(cache)
                .then(|| MessageTelemetry::from_route(route, message.as_ref()))
                .flatten();
            let telemetry = telemetry.as_ref().map(DeliveryTelemetry::for_command);

            for target in targets {
                if let Some(event) = CommandEvent::new(Arc::clone(&message), target.spec) {
                    target
                        .subscriber
                        .send_command(event, telemetry.as_ref(), stopped);
                }
            }
        }

        // Message subscribers observe every remaining non-CTCP message.
        if plan.has_message_subscribers() {
            self.deliver_kind(EventKind::Message, route, cache, message, stopped);
        }

        // URLs — the plan already checked for a URL subscriber and a scheme colon.
        if plan.urls_possible() {
            self.dispatch_urls(filters, route, cache, message, stopped);
        }
    }

    /// Returns whether `text` can start a registered command, judging by its first byte alone.
    ///
    /// The bitmap has no bit for a byte ≥ 0x80; only a trigger starting with one can match such
    /// a message, so the index tracks whether any such trigger exists.
    #[inline]
    pub(super) fn may_start_command(&self, text: &str) -> bool {
        match text.as_bytes().first() {
            Some(&byte) if !(b' '..0x80).contains(&byte) => self.command_initials_non_ascii,
            Some(&byte) => (self.command_initials >> (byte - b' ')) & 1 != 0,
            None => false,
        }
    }

    /// Routes the URLs of a `PRIVMSG` to their host subscribers, then to the generic ones.
    ///
    /// Extracted from [`Self::dispatch_privmsg`] to keep the URL-free path of an IRC line small.
    #[inline(never)]
    fn dispatch_urls(
        &self,
        filters: &FilterHandle,
        route: CommandRoute,
        cache: &mut Option<bool>,
        message: &mut SharedMessage,
        stopped: &mut Vec<&'static str>,
    ) {
        let Command::PRIVMSG(target, text) = &message.message().command else {
            return;
        };

        let sender = Sender::from_message(message.message());

        // The retained URLs are collected before the message is shared, so a `PRIVMSG` that
        // carries no URL (or only filtered ones) still does not allocate the shared message.
        // The subscriber list is resolved by borrowed host while the URL is still unshared.
        let mut seen: SmallVec<[UrlDispatch<'_>; 4]> = SmallVec::new();

        // Deduplication shares the parsed URL with the event instead of hashing a deep copy
        // into a set; a message rarely carries more than a handful of URLs.
        for ExtractedUrl { url, repaired_from } in ExtractUrls::with_schemes(text, HTTP_SCHEMES) {
            if seen.iter().any(|seen| seen.url.as_str() == url.as_str()) {
                continue;
            }

            if filters.is_filtered(target, sender, &url) {
                debug!(url.full = %RedactedUrlRef::from_url(&url), "skipping filtered url");

                continue;
            }

            // A URL nobody subscribed to is not an event; drop it before sharing the message.
            let subscribers: &[Subscriber] = url
                .host_str()
                .and_then(|host| self.url_hosts.get(host))
                .map_or(&[], Vec::as_slice);

            if subscribers.is_empty() && self.url_any.is_empty() {
                continue;
            }

            seen.push(UrlDispatch {
                url: Arc::new(url),
                repaired_from,
                subscribers,
            });
        }

        if seen.is_empty() {
            return;
        }

        let message = message.shared();
        let telemetry = telemetry_enabled(cache)
            .then(|| MessageTelemetry::from_route(route, message.as_ref()))
            .flatten();
        let telemetry = telemetry.as_ref().map(DeliveryTelemetry::for_url);

        for UrlDispatch {
            url,
            repaired_from,
            subscribers,
        } in seen
        {
            let event = UrlEvent::new(Arc::clone(&message), url, repaired_from);

            for subscriber in subscribers {
                subscriber.send(Event::Url(event.clone()), telemetry.as_ref(), stopped);
            }

            for subscriber in &self.url_any {
                subscriber.send(Event::Url(event.clone()), telemetry.as_ref(), stopped);
            }
        }
    }
}
