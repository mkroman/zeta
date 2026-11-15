//! One plugin's delivery endpoint: its mailbox, overflow state and handler span names.

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{Span, debug, warn};

use zeta_plugin::event::CommandEvent;
use zeta_plugin::{Event, EventKind};

use crate::dispatch::envelope::Envelope;
use crate::runtime::MAILBOX_CAPACITY;
use crate::telemetry::MessageTelemetry;

/// The exported handler span names of one plugin, built once when it is added to the index.
///
/// Every handler span is named `zeta.event.handle` for filtering and metrics; the exported
/// `otel.name` carries the bounded discriminator — plugin and event kind — so a trace with one
/// handler span per subscriber stays readable without opening each span.
#[derive(Debug)]
pub(super) struct SpanNames {
    /// The name for command events.
    command: Arc<str>,
    /// The name for URL events.
    url: Arc<str>,
    /// The names for the flag-based kinds, indexed like [`EventKind::ALL`].
    kind_names: [Arc<str>; EventKind::ALL.len()],
}

/// The per-delivery values a handler span is built from, computed once per delivery point when
/// handler spans are enabled.
///
/// The span itself stays per subscriber — it carries the plugin name and has to be created
/// while the dispatch span is current — but everything that only depends on the line and the
/// event batch is computed once and shared by the whole subscriber list, instead of repeating
/// the channel lookup, message-id scan and span-name selection for every plugin. The kind is
/// already known by the call site, so it is never re-derived from the event.
#[derive(Debug)]
pub(super) struct DeliveryTelemetry<'a> {
    /// The event's low-cardinality kind name, recorded as `zeta.event.kind`.
    kind_name: &'static str,
    /// The selector for a plugin's precomputed handler span name.
    span: SpanSelector,
    /// The line's telemetry, shared by the whole delivery batch.
    message: &'a MessageTelemetry<'a>,
}

/// The event-kind discriminator a subscriber turns into its precomputed span name.
#[derive(Clone, Copy, Debug)]
pub(super) enum SpanSelector {
    /// A command event.
    Command,
    /// A URL event.
    Url,
    /// A flag-based event kind.
    Event(EventKind),
}

impl<'a> DeliveryTelemetry<'a> {
    /// Computes the telemetry of one command batch.
    pub(super) const fn for_command(message: &'a MessageTelemetry<'a>) -> Self {
        Self {
            kind_name: "command",
            span: SpanSelector::Command,
            message,
        }
    }

    /// Computes the telemetry of one batch of flag-based events of `kind`.
    pub(super) const fn for_kind(kind: EventKind, message: &'a MessageTelemetry<'a>) -> Self {
        Self {
            kind_name: kind.name(),
            span: SpanSelector::Event(kind),
            message,
        }
    }

    /// Computes the telemetry of one URL batch.
    pub(super) const fn for_url(message: &'a MessageTelemetry<'a>) -> Self {
        Self {
            kind_name: "url",
            span: SpanSelector::Url,
            message,
        }
    }
}

impl SpanNames {
    /// Builds the handler span names for `plugin`.
    pub(super) fn new(plugin: &str) -> Self {
        let composite = |kind: &str| Arc::<str>::from(format!("zeta.event.handle {plugin} {kind}"));

        Self {
            command: composite("command"),
            url: composite("url"),
            kind_names: std::array::from_fn(|index| composite(EventKind::ALL[index].name())),
        }
    }

    /// Returns the exported name for the event with the given span selector.
    pub(super) fn name(&self, span: SpanSelector) -> &str {
        match span {
            SpanSelector::Event(kind) => &self.kind_names[kind.index()],
            SpanSelector::Command => &self.command,
            SpanSelector::Url => &self.url,
        }
    }
}

/// A delivery endpoint for events: the mailbox of one plugin's task.
#[derive(Clone, Debug)]
pub(super) struct Subscriber {
    /// The plugin's name, used for logging.
    pub(super) name: &'static str,
    /// The precomputed handler span names of the plugin.
    span_names: Arc<SpanNames>,
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
pub(super) struct Overflow {
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
    pub(super) fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Records a drop and returns the cumulative count when a warning is due.
    ///
    /// A warning is due on the first drop and then at most once per [`WARN_INTERVAL`]; an
    /// accepted event does not move the window, so a saturated-but-draining mailbox cannot warn
    /// at its drain rate.
    pub(super) fn record_drop(&self, now_secs: u64) -> Option<usize> {
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
    /// Builds the delivery endpoint of one plugin from its `name` and mailbox.
    pub(super) fn new(name: &'static str, mailbox: mpsc::Sender<Envelope>) -> Self {
        Self {
            name,
            span_names: Arc::new(SpanNames::new(name)),
            mailbox,
            overflow: Arc::new(Overflow::default()),
        }
    }

    /// Queues `event` for the plugin, recording the plugin in `stopped` when its mailbox is
    /// closed (i.e. the plugin's task has stopped).
    ///
    /// A full mailbox is not a stopped plugin: the event is shed, the overflow is counted, and
    /// the plugin stays registered. Evicting it here would let a burst permanently unregister a
    /// healthy plugin.
    ///
    /// `meta` is computed once per delivery batch by the dispatcher and shared by the subscriber
    /// list, so the kind name, channel and span-name selector are not looked up again per
    /// plugin. It is [`None`] when handler spans are disabled, in which case no metadata was
    /// computed.
    ///
    /// Forced inline into the subscriber loops: the reservation/build/hand-off split otherwise
    /// turns every delivery into a call, which the dispatch benchmark measures as a regression.
    #[allow(clippy::inline_always)]
    #[inline(always)]
    pub(super) fn send(
        &self,
        event: Event,
        meta: Option<&DeliveryTelemetry<'_>>,
        stopped: &mut Vec<&'static str>,
    ) {
        let Some(permit) = self.reserve(stopped) else {
            return;
        };

        let span = self.handler_span(meta);
        permit.send(Envelope { event, span });
    }

    /// Queues a command event that is delivered to this plugin only.
    ///
    /// Taking the event by value saves the clone a shared delivery would need. The metadata is
    /// computed once per message by the dispatcher and shared by the command's subscribers.
    pub(super) fn send_command(
        &self,
        event: CommandEvent,
        meta: Option<&DeliveryTelemetry<'_>>,
        stopped: &mut Vec<&'static str>,
    ) {
        let Some(permit) = self.reserve(stopped) else {
            return;
        };

        let span = self.handler_span(meta);
        permit.send(Envelope {
            event: Event::Command(event),
            span,
        });
    }

    /// Builds the handler span for one delivery.
    ///
    /// Created here, while the dispatch span is current, so the handler joins the message's
    /// trace. The span also covers the time the event waits in the mailbox. When `meta` is
    /// [`None`] — the filter disabled handler spans — the current span, the IRC line's dispatch
    /// span, is carried instead, so the handler logs still have a trace context; with no
    /// subscriber at all that is [`Span::none()`].
    ///
    /// Kept small and inline so the disabled path stays a cheap check in the per-subscriber send
    /// path; the span construction lives in [`Self::build_handler_span`].
    #[inline]
    fn handler_span(&self, meta: Option<&DeliveryTelemetry<'_>>) -> Span {
        let Some(meta) = meta else {
            return Span::current();
        };

        self.build_handler_span(meta)
    }

    /// Creates the enabled handler span from `meta`.
    ///
    /// Outlined from [`Self::handler_span`]: the span macro is large, and inlining it into every
    /// delivery would make the disabled path pay for the code size.
    #[inline(never)]
    fn build_handler_span(&self, meta: &DeliveryTelemetry<'_>) -> Span {
        // The name is precomputed: formatting it here would allocate on every event even when
        // `tracing` is disabled and the macro never evaluates its fields.
        let name = self.span_names.name(meta.span);
        let message = meta.message;

        let span = tracing::info_span!(
            target: "zeta.plugins",
            "zeta.event.handle",
            otel.name = %name,
            otel.kind = "consumer",
            zeta.plugin.name = %self.name,
            zeta.event.kind = meta.kind_name,
            irc.channel = tracing::field::Empty,
            messaging.system.name = "irc",
            messaging.operation.name = %message.operation,
            messaging.operation.type = "process",
            messaging.destination.name = tracing::field::Empty,
            messaging.message.id = tracing::field::Empty,
        );

        if let Some(channel) = message.channel {
            span.record("irc.channel", channel);
            span.record("messaging.destination.name", channel);
        }

        if let Some(message_id) = message.message_id {
            span.record("messaging.message.id", message_id);
        }

        span.or_current()
    }

    /// Reserves a mailbox slot for one delivery.
    ///
    /// Reserving before building the handler span means a shed or closed delivery never creates
    /// a span for an event the plugin will not receive. A full mailbox is not a stopped plugin:
    /// the event is shed, the overflow is counted, and the plugin stays registered. Evicting it
    /// here would let a burst permanently unregister a healthy plugin.
    ///
    /// Returns [`None`] after shedding the event or recording the stopped plugin.
    ///
    /// Inlined together with [`Self::send`] so the reservation and the hand-off stay one hot
    /// path.
    #[allow(clippy::inline_always)]
    #[inline(always)]
    fn reserve(&self, stopped: &mut Vec<&'static str>) -> Option<mpsc::Permit<'_, Envelope>> {
        match self.mailbox.try_reserve() {
            Ok(permit) => Some(permit),
            Err(mpsc::error::TrySendError::Closed(())) => {
                stopped.push(self.name);

                None
            }
            Err(mpsc::error::TrySendError::Full(())) => {
                self.shed();

                None
            }
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
