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
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use irc::proto::{Command, Message};
use smallvec::SmallVec;
use tokio::sync::mpsc;
use tracing::{Span, debug, warn};
use url::Url;
use zeta_plugin::event::{CommandEvent, Sender, Subscriptions, UrlEvent, is_ctcp};
use zeta_plugin::{CommandSpec, Event, EventKind, FilterHandle, UrlScope};
use zeta_url::{ExtractUrls, ExtractedUrl, HTTP_SCHEMES, RedactedUrlRef};

use crate::runtime::MAILBOX_CAPACITY;

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
    /// The names for the flag-based kinds, indexed like [`EventKind::ALL`].
    kinds: [Arc<str>; EventKind::ALL.len()],
}

/// The per-event values a handler span is built from, computed once per delivered event.
///
/// The span itself stays per subscriber — it carries the plugin name and has to be created
/// while the dispatch span is current — but everything that only depends on the event is
/// computed once and shared by the whole subscriber list, instead of repeating the channel
/// lookup and span-name selection for every plugin. The kind is already known by the call site,
/// so it is never re-derived from the event.
#[derive(Clone, Copy, Debug)]
struct EventMeta<'a> {
    /// The event's low-cardinality kind name, recorded as `zeta.event.kind`.
    kind_name: &'static str,
    /// The channel the event belongs to, recorded as `irc.channel`.
    channel: Option<&'a str>,
    /// The selector for a plugin's precomputed handler span name.
    span: EventSpan,
}

/// The event-kind discriminator a subscriber turns into its precomputed span name.
#[derive(Clone, Copy, Debug)]
enum EventSpan {
    /// A command event.
    Command,
    /// A URL event.
    Url,
    /// A flag-based event kind.
    Kind(EventKind),
}

impl<'a> EventMeta<'a> {
    /// Computes the metadata of a command event.
    fn command(event: &'a CommandEvent) -> Self {
        Self {
            kind_name: "command",
            channel: Some(event.channel()),
            span: EventSpan::Command,
        }
    }

    /// Computes the metadata of a flag-based event of `kind`.
    fn kind(kind: EventKind, event: &'a Event) -> Self {
        Self {
            kind_name: kind.name(),
            channel: event.channel(),
            span: EventSpan::Kind(kind),
        }
    }

    /// Computes the metadata of a URL event.
    fn url(event: &'a UrlEvent) -> Self {
        Self {
            kind_name: "url",
            channel: Some(event.channel()),
            span: EventSpan::Url,
        }
    }
}

impl SpanNames {
    /// Builds the handler span names for `plugin`.
    fn new(plugin: &str) -> Self {
        let composite = |kind: &str| Arc::<str>::from(format!("zeta.event.handle {plugin} {kind}"));

        Self {
            command: composite("command"),
            url: composite("url"),
            kinds: std::array::from_fn(|index| composite(EventKind::ALL[index].name())),
        }
    }

    /// Returns the exported name for the event with the given span selector.
    fn event(&self, span: EventSpan) -> &str {
        match span {
            EventSpan::Kind(kind) => &self.kinds[kind.index()],
            EventSpan::Command => &self.command,
            EventSpan::Url => &self.url,
        }
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
    mailbox: mpsc::Sender<Envelope>,
    /// The mailbox's overflow state, shared by every subscription of the plugin.
    overflow: Arc<Overflow>,
}

/// The minimum interval between mailbox-overflow warnings.
const WARN_INTERVAL: Duration = Duration::from_secs(30);

/// The overflow state of one plugin mailbox.
///
/// The dispatcher is the only writer, but the state is shared by every subscription that targets
/// the mailbox, so it lives in atomics rather than in one dispatch call's locals. The drop count
/// is cumulative for the lifetime of the mailbox: an accepted event does not reset it, so the
/// warning reports how many events the plugin has shed in total.
#[derive(Debug)]
struct Overflow {
    /// Events this mailbox has shed since the process started.
    dropped: AtomicUsize,
    /// Seconds since the process start when the last warning was emitted; [`NEVER`] if none was.
    last_warn: AtomicU64,
}

/// The `last_warn` of a mailbox that has not warned yet.
///
/// Zero is a valid timestamp — every drop during the first second of the process would otherwise
/// look unwarned — so the sentinel is the largest value instead.
const NEVER: u64 = u64::MAX;

impl Default for Overflow {
    fn default() -> Self {
        Self {
            dropped: AtomicUsize::new(0),
            last_warn: AtomicU64::new(NEVER),
        }
    }
}

impl Overflow {
    /// Returns the number of events the mailbox has shed so far.
    fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Records a drop and returns the cumulative count when a warning is due.
    ///
    /// A warning is due on the first drop and then at most once per [`WARN_INTERVAL`]; an
    /// accepted event does not move the window, so a saturated-but-draining mailbox cannot warn
    /// at its drain rate.
    fn record_drop(&self, now_secs: u64) -> Option<usize> {
        let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        let last = self.last_warn.load(Ordering::Relaxed);
        let due = last == NEVER || now_secs.saturating_sub(last) >= WARN_INTERVAL.as_secs();

        if due && self.last_warn.swap(now_secs, Ordering::Relaxed) == last {
            Some(dropped)
        } else {
            None
        }
    }
}

/// Returns the whole seconds elapsed since the process-wide base instant.
///
/// The base is set on the first call: nothing needs a clock before the first overflow, and the
/// function only runs on the cold drop path.
fn now_secs() -> u64 {
    static BASE: OnceLock<tokio::time::Instant> = OnceLock::new();

    BASE.get_or_init(tokio::time::Instant::now)
        .elapsed()
        .as_secs()
}

impl Subscriber {
    /// Queues `event` for the plugin, recording the plugin in `stopped` when its mailbox is
    /// closed (i.e. the plugin's task has stopped).
    ///
    /// A full mailbox is not a stopped plugin: the event is shed, the overflow is counted, and
    /// the plugin stays registered. Evicting it here would let a burst permanently unregister a
    /// healthy plugin.
    ///
    /// `meta` is computed once per event by the dispatcher and shared by the subscriber list, so
    /// the kind name, channel and span-name selector are not looked up again per plugin.
    fn send(&self, event: Event, meta: EventMeta<'_>, stopped: &mut Vec<&'static str>) {
        let span = self.handler_span(meta);

        self.enqueue(Envelope { event, span }, stopped);
    }

    /// Queues a command event that is delivered to this plugin only.
    ///
    /// Taking the event by value saves the clone a shared delivery would need. The metadata is
    /// derived here from the already-known command route, with a single channel lookup.
    fn send_owned(&self, event: CommandEvent, stopped: &mut Vec<&'static str>) {
        let span = self.handler_span(EventMeta::command(&event));

        self.enqueue(
            Envelope {
                event: Event::Command(event),
                span,
            },
            stopped,
        );
    }

    /// Builds the handler span for one delivery.
    ///
    /// Created here, while the dispatch span is current, so the handler joins the message's
    /// trace. The span also covers the time the event waits in the mailbox. When the handler
    /// span is disabled by the filter, the current span — the IRC line's dispatch span — is
    /// carried instead, so the handler logs still have a trace context; with no subscriber at
    /// all that is [`Span::none()`].
    fn handler_span(&self, meta: EventMeta<'_>) -> Span {
        if !tracing::enabled!(target: "zeta.plugins", tracing::Level::INFO) {
            return Span::current();
        }

        // The name is precomputed: formatting it here would allocate on every event even when
        // `tracing` is disabled and the macro never evaluates its fields.
        let name = self.names.event(meta.span);

        let span = tracing::info_span!(
            target: "zeta.plugins",
            "zeta.event.handle",
            otel.name = %name,
            otel.kind = "consumer",
            zeta.plugin.name = %self.name,
            zeta.event.kind = meta.kind_name,
            irc.channel = tracing::field::Empty,
        );

        if let Some(channel) = meta.channel {
            span.record("irc.channel", channel);
        }

        span.or_current()
    }

    /// Queues `envelope` on the mailbox, shedding it or reporting the stopped plugin.
    fn enqueue(&self, envelope: Envelope, stopped: &mut Vec<&'static str>) {
        match self.mailbox.try_send(envelope) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => stopped.push(self.name),
            Err(mpsc::error::TrySendError::Full(_)) => self.shed(),
        }
    }

    /// Records that the mailbox was full and the event was shed.
    ///
    /// A warning is emitted at most once per [`WARN_INTERVAL`]; the drops in between only log at
    /// `debug` with the cumulative count. Warning on every drop would let a stalled plugin flood
    /// the log at the same rate as the traffic it is failing to handle, and resetting the episode
    /// on the next accepted event let a saturated-but-draining mailbox warn at its drain rate.
    fn shed(&self) {
        if let Some(dropped) = self.overflow.record_drop(now_secs()) {
            warn!(
                zeta.plugin.name = %self.name,
                zeta.plugin.mailbox.dropped = dropped,
                zeta.plugin.mailbox.capacity = MAILBOX_CAPACITY,
                "plugin mailbox is full; dropping events"
            );
        } else {
            debug!(
                zeta.plugin.name = %self.name,
                zeta.plugin.mailbox.dropped = self.overflow.dropped(),
                zeta.plugin.mailbox.capacity = MAILBOX_CAPACITY,
                "plugin mailbox is still full; dropping event"
            );
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
    fn message(&self) -> &Message {
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
    /// Classifies `command` by the events it can produce.
    const fn of(command: &Command) -> Self {
        match command {
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

    /// Returns the IRC command name of a routed command, for telemetry.
    fn command_name(self, command: &Command) -> Option<&str> {
        match self {
            Self::Privmsg => Some("PRIVMSG"),
            // A NOTICE is the only command routed as a CTCP event.
            Self::Kind(EventKind::Ctcp) => Some("NOTICE"),
            Self::Kind(EventKind::Join) => Some("JOIN"),
            Self::Kind(EventKind::Part) => Some("PART"),
            Self::Kind(EventKind::Quit) => Some("QUIT"),
            Self::Kind(EventKind::Nick) => Some("NICK"),
            Self::Kind(EventKind::Kick) => Some("KICK"),
            Self::Kind(EventKind::Raw) => match command {
                Command::Raw(name, _) => Some(name),
                _ => None,
            },
            // `Message` has no IRC command of its own, and future kinds need a runtime that
            // knows how to route them.
            Self::Kind(_) | Self::Ignore => None,
        }
    }
}

/// Returns the IRC command name when [`EventIndex::dispatch`] routes the command, for telemetry.
///
/// The classification comes from the dispatcher's route table, so the host's `irc.dispatch`
/// span cannot drift from what the dispatcher actually handles. `Command::ERROR` is not
/// routed; the host adds it because the disconnect reason is logged inside the span.
#[must_use]
pub fn routed_command_name(command: &Command) -> Option<&str> {
    Route::of(command).command_name(command)
}

/// A URL retained for dispatch: its shared handle, the broken scheme prefix it was repaired
/// from, and the host subscribers it must reach, resolved before the URL was shared.
type RetainedUrl<'a> = (Arc<Url>, Option<&'static str>, &'a [Subscriber]);

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
    commands: IndexMap<&'static str, Vec<CommandTarget>>,
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
    url_hosts: IndexMap<String, Vec<Subscriber>>,
    /// Plugins subscribed to every URL, including hosts other plugins handle.
    url_any: Vec<Subscriber>,
    /// Plugins subscribed to plain-interest event kinds, indexed by [`EventKind::index`].
    kinds: [Vec<Subscriber>; EventKind::ALL.len()],
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
        let subscriber = Subscriber {
            name,
            names: Arc::new(SpanNames::new(name)),
            mailbox,
            overflow: Arc::new(Overflow::default()),
        };

        for command in subscriptions.commands() {
            if command.trigger().is_empty() || command.trigger().contains(char::is_whitespace) {
                warn!(
                    zeta.plugin.name = %name,
                    command = command.trigger(),
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
    /// Returns the names of the plugins whose mailboxes are closed — their tasks have stopped.
    /// They are already evicted from the index, so the caller only needs to report them.
    #[must_use]
    pub fn dispatch(&mut self, filters: &FilterHandle, message: Message) -> Vec<&'static str> {
        let mut message = SharedMessage::new(message);
        let mut stopped = Vec::new();

        match Route::of(&message.message().command) {
            Route::Privmsg => self.dispatch_privmsg(filters, &mut message, &mut stopped),
            Route::Kind(kind) => self.deliver_kind(kind, &mut message, &mut stopped),
            Route::Ignore => {}
        }

        // A stopped plugin can match several of the message's events (and several commands or
        // URLs); report and evict it once.
        if !stopped.is_empty() {
            stopped.sort_unstable();
            stopped.dedup();

            self.evict_stopped(&stopped);
        }

        stopped
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
    fn subscribers(&self, kind: EventKind) -> &[Subscriber] {
        &self.kinds[kind.index()]
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

        if subscribers.is_empty() || (kind == EventKind::Ctcp && !is_ctcp(message.message())) {
            return;
        }

        let Some(event) = kind.event(message.shared()) else {
            return;
        };

        Self::deliver(&event, kind, subscribers, stopped);
    }

    /// Delivers one event to every subscriber of a list.
    ///
    /// The metadata depends only on the event and its already-known kind, so it is computed once
    /// and shared by the subscriber list instead of re-deriving the kind per plugin.
    fn deliver(
        event: &Event,
        kind: EventKind,
        subscribers: &[Subscriber],
        stopped: &mut Vec<&'static str>,
    ) {
        let meta = EventMeta::kind(kind, event);

        for subscriber in subscribers {
            subscriber.send(event.clone(), meta, stopped);
        }
    }

    /// Routes a `PRIVMSG` to the plugins whose subscriptions match.
    fn dispatch_privmsg(
        &self,
        filters: &FilterHandle,
        message: &mut SharedMessage,
        stopped: &mut Vec<&'static str>,
    ) {
        let Command::PRIVMSG(target, text) = &message.message().command else {
            return;
        };

        // CTCP messages are their own event kind and are routed to nothing else.
        if is_ctcp(message.message()) {
            self.deliver_kind(EventKind::Ctcp, message, stopped);

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
        // trigger semantics. Leading whitespace makes the first word non-matching, and checking
        // it before sharing keeps such a message allocation-free.
        if self.may_start_command(text)
            && let Some(word) = text.split_whitespace().next()
            && text.starts_with(word)
            && let Some(targets) = self.commands.get(word)
        {
            let message = message.shared();

            for target in targets {
                if let Some(event) = CommandEvent::new(Arc::clone(&message), target.spec) {
                    target.subscriber.send_owned(event, stopped);
                }
            }
        }

        // Message subscribers observe every remaining non-CTCP message.
        self.deliver_kind(EventKind::Message, message, stopped);

        // URLs — extracted once per message, deduplicated and filtered before routing by host.
        if self.url_hosts.is_empty() && self.url_any.is_empty() {
            return;
        }

        self.dispatch_urls(filters, message, stopped);
    }

    /// Returns whether `text` can start a registered command, judging by its first byte alone.
    ///
    /// The bitmap has no bit for a byte ≥ 0x80; only a trigger starting with one can match such
    /// a message, so the index tracks whether any such trigger exists.
    #[inline]
    fn may_start_command(&self, text: &str) -> bool {
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
        message: &mut SharedMessage,
        stopped: &mut Vec<&'static str>,
    ) {
        let Command::PRIVMSG(target, text) = &message.message().command else {
            return;
        };

        // An extracted URL always carries a scheme, and a scheme is terminated by `:`; text
        // without one cannot yield a URL. Skipping the word scan here keeps the link-free
        // message — the overwhelming majority of channel traffic — free of it.
        if !text.contains(':') {
            return;
        }

        let sender = Sender::from_message(message.message());

        // The retained URLs are collected before the message is shared, so a `PRIVMSG` that
        // carries no URL (or only filtered ones) still does not allocate the shared message.
        // The subscriber list is resolved by borrowed host while the URL is still unshared.
        let mut seen: SmallVec<[RetainedUrl<'_>; 4]> = SmallVec::new();

        // Deduplication shares the parsed URL with the event instead of hashing a deep copy
        // into a set; a message rarely carries more than a handful of URLs.
        for ExtractedUrl { url, repaired_from } in ExtractUrls::with_schemes(text, HTTP_SCHEMES) {
            if seen.iter().any(|(seen, ..)| seen.as_str() == url.as_str()) {
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

            seen.push((Arc::new(url), repaired_from, subscribers));
        }

        if seen.is_empty() {
            return;
        }

        let message = message.shared();

        for (url, repaired_from, subscribers) in seen {
            let event = UrlEvent::new(Arc::clone(&message), url, repaired_from);
            let meta = EventMeta::url(&event);

            for subscriber in subscribers {
                subscriber.send(Event::Url(event.clone()), meta, stopped);
            }

            for subscriber in &self.url_any {
                subscriber.send(Event::Url(event.clone()), meta, stopped);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use url::Url;
    use zeta_plugin::CtcpKind;
    use zeta_plugin::EventFilter;
    use zeta_test_support::assert_empty;

    use super::*;

    /// Measures the allocations and requested bytes of one `dispatch` call.
    fn measure(
        label: &str,
        index: &mut EventIndex,
        filters: &FilterHandle,
        message: Message,
    ) -> allocation_counter::AllocationInfo {
        let info = allocation_counter::measure(|| {
            let stopped = index.dispatch(filters, message);
            std::hint::black_box(&stopped);
        });

        println!(
            "dispatch_allocations: {label}: allocations={} bytes_allocated={}",
            info.count_total, info.bytes_total
        );

        info
    }

    /// Dispatching a message that produces no event must not touch the heap, whether or not a
    /// filter handle is published.
    ///
    /// A posted URL is excluded: extraction parses and allocates it before the filter can drop
    /// it, so URL filtering is covered by [`filtered_urls_are_not_dispatched`] instead.
    #[test]
    fn dispatch_without_events_does_not_allocate() {
        println!(
            "sizes: EventIndex={} Message={} Arc<Message>={} ArcInner<Message>-equivalent={}",
            size_of::<EventIndex>(),
            size_of::<Message>(),
            size_of::<Arc<Message>>(),
            size_of::<Message>() + 2 * size_of::<usize>(),
        );

        let no_filters = FilterHandle::default();
        let mut index = EventIndex::default();

        let unhandled = measure(
            "unhandled privmsg",
            &mut index,
            &no_filters,
            privmsg("hello"),
        );
        let numeric = measure(
            "numeric 001",
            &mut index,
            &no_filters,
            message("server.example", "001", &["zeta", "Welcome"]),
        );
        let join = measure(
            "join",
            &mut index,
            &no_filters,
            message("nick!u@h", "JOIN", &["#test"]),
        );

        let mut ctcp_index = EventIndex::default();
        let _ctcp = subscribe(&mut ctcp_index, "ctcp", |subscriptions| {
            subscriptions.receive_ctcp();
        });
        let plain_notice = measure(
            "plain notice",
            &mut ctcp_index,
            &no_filters,
            message("server.example", "NOTICE", &["zeta", "maintenance"]),
        );

        let mut url_index = EventIndex::default();
        let _titles = subscribe(&mut url_index, "titles", |subscriptions| {
            subscriptions.urls(UrlScope::Any);
        });
        let urlless_privmsg = measure(
            "url-less privmsg",
            &mut url_index,
            &no_filters,
            privmsg("no link here"),
        );

        // A published filter handle is consulted before routing; an ignored sender still takes
        // the allocation-free path out of dispatch.
        let filters = FilterHandle::new(Arc::new(TestFilter));
        let ignored = measure(
            "ignored sender",
            &mut url_index,
            &filters,
            privmsg_from("ignored", ".dig https://example.com/x"),
        );

        for (label, info) in [
            ("unhandled privmsg", unhandled),
            ("numeric 001", numeric),
            ("join", join),
            ("plain notice", plain_notice),
            ("url-less privmsg", urlless_privmsg),
            ("ignored sender", ignored),
        ] {
            assert_eq!(info.count_total, 0, "{label} allocated");
            assert_eq!(info.bytes_total, 0, "{label} allocated bytes");
        }
    }

    /// Collects every event a test subscriber received from its mailbox.
    fn drain(mailbox: &mut mpsc::Receiver<Envelope>) -> Vec<Event> {
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
    ) -> mpsc::Receiver<Envelope> {
        let mut subscriptions = Subscriptions::new();
        register(&mut subscriptions);

        let (mailbox, receiver) = mpsc::channel(8);
        index.add(name, &subscriptions, mailbox);

        receiver
    }

    /// Registers a subscriber for [`dispatch_benchmark`] with a `capacity`-slot mailbox.
    ///
    /// The benchmark never drains its mailboxes. The regular [`subscribe`] helper uses the
    /// 8-slot mailbox of the unit tests, which would make the benchmark measure the overflow path
    /// after the eighth dispatch instead of accepted delivery.
    fn benchmark_subscriber(
        index: &mut EventIndex,
        name: &'static str,
        capacity: usize,
        register: impl FnOnce(&mut Subscriptions),
    ) -> mpsc::Receiver<Envelope> {
        let mut subscriptions = Subscriptions::new();
        register(&mut subscriptions);

        let (mailbox, receiver) = mpsc::channel(capacity);
        index.add(name, &subscriptions, mailbox);

        receiver
    }

    #[test]
    fn registered_command_is_dispatched() {
        let mut index = EventIndex::default();
        let mut mailbox = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig"));
        });

        let stopped = index.dispatch(&FilterHandle::default(), privmsg(".dig example.com"));

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
            let _ = index.dispatch(&FilterHandle::default(), privmsg(text));

            assert_empty!(drain(&mut mailbox), "`{text}` must not dispatch");
        }
    }

    #[test]
    fn empty_and_whitespace_triggers_are_not_indexed() {
        let mut index = EventIndex::default();
        let _ = subscribe(&mut index, "invalid", |subscriptions| {
            subscriptions
                .command(CommandSpec::new("", "empty"))
                .command(CommandSpec::new(".two words", "whitespace"));
        });

        assert_empty!(index.commands);
    }

    /// Triggers whose first byte is outside the command-initial mask cannot be pre-filtered by
    /// it; they must still take the exact first-word path.
    #[test]
    fn non_ascii_triggers_are_still_dispatched() {
        let mut index = EventIndex::default();
        let mut mailbox = subscribe(&mut index, "unicode", |subscriptions| {
            subscriptions.command(CommandSpec::new("écho", "echo"));
        });

        let _ = index.dispatch(&FilterHandle::default(), privmsg("écho bonjour"));

        assert!(matches!(
            drain(&mut mailbox)[..],
            [Event::Command(ref command)] if command.args() == "bonjour"
        ));
    }

    /// Without a trigger starting outside the ASCII bitmap, a non-ASCII message must not take
    /// the first-word split and command hash lookup at all.
    #[test]
    fn non_ascii_messages_are_rejected_without_non_ascii_triggers() {
        let mut index = EventIndex::default();
        let _dig = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig"));
        });

        assert!(!index.may_start_command("æble grød"));
        assert!(!index.may_start_command("🦀 rust"));
        assert!(!index.may_start_command(""));
        assert!(index.may_start_command(".dig example.com"));
    }

    #[test]
    fn multiple_registered_commands_dispatch_independently() {
        let mut index = EventIndex::default();
        let mut kagi = subscribe(&mut index, "kagi", |subscriptions| {
            subscriptions
                .command(CommandSpec::new(".g", "search"))
                .command(CommandSpec::new(".gis", "images"));
        });

        let _ = index.dispatch(&FilterHandle::default(), privmsg(".gis black cats"));

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
            &FilterHandle::default(),
            privmsg("https://youtube.com/watch?v=1"),
        );

        assert!(matches!(drain(&mut youtube)[..], [Event::Url(..)]));
        assert!(matches!(drain(&mut titles)[..], [Event::Url(..)]));

        // Host matching is case-insensitive.
        let _ = index.dispatch(
            &FilterHandle::default(),
            privmsg("https://WWW.YOUTUBE.COM/x"),
        );

        assert_eq!(drain(&mut youtube).len(), 1);
        assert_eq!(drain(&mut titles).len(), 1);

        // An unclaimed host only reaches generic handlers.
        let _ = index.dispatch(&FilterHandle::default(), privmsg("https://example.com/x"));

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
            &FilterHandle::default(),
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
        let _ = index.dispatch(&FilterHandle::default(), privmsg("https://[2001:DB8::1]/x"));

        assert_eq!(drain(&mut ipv6).len(), 1);
    }

    /// A filter that ignores the `ignored` nick entirely and matches every URL on the
    /// `filtered.example` host.
    struct TestFilter;

    impl EventFilter for TestFilter {
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
            .publish(Arc::new(FilterHandle::new(Arc::new(TestFilter))));

        let mut index = EventIndex::default();
        let mut filtered = subscribe(&mut index, "filtered", |subscriptions| {
            subscriptions.urls(UrlScope::Hosts(&["filtered.example"]));
        });
        let mut titles = subscribe(&mut index, "titles", |s| {
            s.urls(UrlScope::Any);
        });

        let filters = FilterHandle::from_context(&ctx);
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
            .publish(Arc::new(FilterHandle::new(Arc::new(TestFilter))));

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

        let filters = FilterHandle::from_context(&ctx);

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

        let _ = index.dispatch(
            &FilterHandle::default(),
            privmsg("\x01ACTION slaps .dig\x01"),
        );

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
            &FilterHandle::default(),
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
            &FilterHandle::default(),
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

        let _ = index.dispatch(&FilterHandle::default(), privmsg(".dig example.com"));

        assert!(matches!(drain(&mut dig)[..], [Event::Command(..)]));
        assert!(matches!(drain(&mut watcher)[..], [Event::Message(..)]));

        let _ = index.dispatch(&FilterHandle::default(), privmsg("no events at all?"));

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

        let _ = index.dispatch(
            &FilterHandle::default(),
            message("nick!u@h", "JOIN", &["#test"]),
        );
        let _ = index.dispatch(
            &FilterHandle::default(),
            message("nick!u@h", "QUIT", &["gone"]),
        );

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
            &FilterHandle::default(),
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
                // `cases` mirrors `EventKind::ALL`, so a new kind fails the length assertion
                // above before it can reach this arm.
                _ => unreachable!("every event kind has a subscription method"),
            });

            let _ = index.dispatch(&FilterHandle::default(), message.clone());

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

        let stopped = index.dispatch(&FilterHandle::default(), privmsg(".dig example.com"));
        assert_eq!(stopped, ["dig"]);

        // The evicted plugin receives nothing further.
        let stopped = index.dispatch(
            &FilterHandle::default(),
            message("n!u@h", "JOIN", &["#test"]),
        );

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
        let stopped = index.dispatch(
            &FilterHandle::default(),
            privmsg(".dig https://example.com/x"),
        );

        assert_eq!(stopped, ["dig"]);
    }

    /// A full mailbox sheds events instead of growing without bound, and `Full` must not be
    /// treated as `Closed`: a burst would otherwise permanently unregister a healthy plugin.
    #[test]
    fn a_full_mailbox_drops_events_without_evicting_the_plugin() {
        const CAPACITY: usize = 8;

        let mut index = EventIndex::default();
        let mut subscriptions = Subscriptions::new();
        subscriptions.command(CommandSpec::new(".dig", "dig"));

        // The mailbox is registered directly instead of through `subscribe`, so the test can
        // fill its 8 slots and reach the full state in a few dispatches.
        let (mailbox, mut receiver) = mpsc::channel(CAPACITY);
        index.add("dig", &subscriptions, mailbox);

        for argument in 0..CAPACITY {
            let stopped = index.dispatch(
                &FilterHandle::default(),
                privmsg(&format!(".dig {argument}")),
            );

            assert_empty!(stopped, "accepted event {argument}");
        }

        let stopped = index.dispatch(&FilterHandle::default(), privmsg(".dig overflow"));
        assert_empty!(stopped, "a full mailbox must not evict the plugin");

        let arguments = drain(&mut receiver)
            .into_iter()
            .map(|event| match event {
                Event::Command(command) => command.args().to_string(),
                other => panic!("expected a command event, got {other:?}"),
            })
            .collect::<Vec<_>>();

        assert_eq!(arguments.len(), CAPACITY);
        assert_eq!(
            arguments,
            (0..CAPACITY)
                .map(|argument| argument.to_string())
                .collect::<Vec<_>>(),
            "accepted events must preserve FIFO order"
        );

        // Draining the mailbox frees a slot: the plugin is still registered and receives the
        // next event.
        let stopped = index.dispatch(&FilterHandle::default(), privmsg(".dig after"));
        assert_empty!(stopped);
        assert!(matches!(
            drain(&mut receiver)[..],
            [Event::Command(ref command)] if command.args() == "after"
        ));
    }

    /// The first drop of a mailbox overflow warns with the cumulative count.
    #[test]
    fn the_first_drop_warns() {
        let overflow = Overflow::default();

        assert_eq!(overflow.record_drop(1), Some(1));
        assert_eq!(overflow.dropped(), 1);
    }

    /// Further drops inside the warning interval only count; they do not warn.
    #[test]
    fn drops_within_the_warning_interval_stay_at_debug() {
        let overflow = Overflow::default();

        assert_eq!(overflow.record_drop(1), Some(1));
        assert_eq!(overflow.record_drop(2), None);
        assert_eq!(overflow.record_drop(30), None);
        assert_eq!(overflow.dropped(), 3);
    }

    /// A drop at or after the interval warns again, carrying the cumulative count, so the warning
    /// alone says how many events were shed in total.
    #[test]
    fn a_drop_after_the_warning_interval_warns_again() {
        let overflow = Overflow::default();

        assert_eq!(overflow.record_drop(10), Some(1));
        assert_eq!(overflow.record_drop(39), None);
        assert_eq!(overflow.record_drop(40), Some(3));
        assert_eq!(overflow.record_drop(70), Some(4));
    }

    /// An accepted event does not touch the overflow state: it neither resets the cumulative
    /// count nor re-arms the warning. `Subscriber::send` only calls into [`Overflow`] on `Full`,
    /// so the old episode reset — a warning per accepted event — cannot reappear here.
    #[test]
    fn accepted_events_do_not_reset_the_overflow_state() {
        let overflow = Overflow::default();

        assert_eq!(overflow.record_drop(1), Some(1));

        // The mailbox accepted an event: `send`'s `Ok` arm does nothing.
        assert_eq!(overflow.record_drop(2), None);
        assert_eq!(overflow.record_drop(15), None);
        assert_eq!(overflow.dropped(), 3);

        // The window still starts at the first warning, so the count keeps growing across it.
        assert_eq!(overflow.record_drop(31), Some(4));
    }

    /// A mailbox that keeps accepting an event between drops is the case the old episode logic
    /// mishandled: every accepted event re-armed the warning, so it warned once per accepted
    /// event. The time window caps that at one warning per [`WARN_INTERVAL`], while the count
    /// still reflects every drop.
    #[test]
    fn drops_interleaved_with_accepts_warn_once_per_window() {
        const EVENTS: u64 = 100_000;

        let overflow = Overflow::default();
        let mut drops = 0;
        let mut warnings = 0;

        // One event per millisecond for 100 s; every second event is accepted, the rest shed.
        // The timestamps are the event's second, so the drops of one second share a window slot.
        for event in 0..EVENTS {
            if event % 2 == 0 {
                continue;
            }

            drops += 1;

            if overflow.record_drop(event / 1_000).is_some() {
                warnings += 1;
            }
        }

        assert_eq!(drops, EVENTS / 2);
        assert_eq!(warnings, 4, "one warning per 30 s window over 100 s");
        assert_eq!(overflow.dropped(), 50_000);
    }

    /// The precomputed handler span names must follow the event kinds: the array is derived
    /// from [`EventKind::ALL`], so it cannot drift from the enum.
    #[test]
    fn span_names_match_the_event_kinds() {
        let names = SpanNames::new("dig");

        assert_eq!(
            names.event(EventSpan::Command),
            "zeta.event.handle dig command"
        );
        assert_eq!(names.event(EventSpan::Url), "zeta.event.handle dig url");

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
                names.event(EventSpan::Kind(*kind)),
                format!("zeta.event.handle dig {}", event.kind_name())
            );
        }
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

        // The benchmark never drains a mailbox, so each subscriber gets one that accepts every
        // event its case dispatches; the 8-slot unit-test mailbox would shed after eight sends.
        // Each subscriber receives at most `DISPATCHES` events, one per dispatched message.

        let mut index = EventIndex::default();
        let _dig = benchmark_subscriber(&mut index, "dig", DISPATCHES, |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig"));
        });
        let _joiner = benchmark_subscriber(&mut index, "joiner", DISPATCHES, |subscriptions| {
            subscriptions.receive_join();
        });
        let _ctcp = benchmark_subscriber(&mut index, "ctcp", DISPATCHES, |subscriptions| {
            subscriptions.receive_ctcp();
        });

        let filters = FilterHandle::default();

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

        // The index above has no URL subscribers, so all of its cases return before extraction.
        // This second index reaches the scheme-colon gate and, past it, the URL scan: a generic
        // subscriber like `titles` and a host-scoped one like `youtube`.
        let mut url_index = EventIndex::default();
        let _url_dig = benchmark_subscriber(&mut url_index, "dig", DISPATCHES, |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig"));
        });
        let _titles = benchmark_subscriber(&mut url_index, "titles", DISPATCHES, |subscriptions| {
            subscriptions.urls(UrlScope::Any);
        });
        let _example =
            benchmark_subscriber(&mut url_index, "example", DISPATCHES, |subscriptions| {
                subscriptions.urls(UrlScope::Hosts(&["example.com"]));
            });

        benchmark(
            "unhandled privmsg, url subscriber",
            DISPATCHES,
            &mut url_index,
            &filters,
            || privmsg("hello there"),
        );
        benchmark(
            "colon but no url",
            DISPATCHES,
            &mut url_index,
            &filters,
            || privmsg("note: no link here"),
        );
        benchmark("one url", DISPATCHES, &mut url_index, &filters, || {
            privmsg("look at https://example.com/page")
        });
        benchmark(
            "non-ascii initial, ascii triggers",
            DISPATCHES,
            &mut url_index,
            &filters,
            || privmsg("æble grød"),
        );
    }

    /// Times `dispatches` calls to [`EventIndex::dispatch`] with pre-built messages.
    ///
    /// The messages are built before the timer starts, so the measurement covers dispatch only.
    fn benchmark(
        label: &str,
        dispatches: usize,
        index: &mut EventIndex,
        filters: &FilterHandle,
        make: impl Fn() -> Message,
    ) {
        let messages: Vec<Message> = Vec::from_fn(dispatches, |_| make());
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
