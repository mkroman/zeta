//! Event dispatching: routing incoming IRC messages to the plugins that registered interest.
//!
//! The [`EventIndex`] maps the subscriptions every plugin declared during initialization to
//! delivery endpoints. It matches incoming IRC messages against the index — commands by their
//! trigger through a hash lookup on the message's first word, URLs by their host through a
//! hash lookup, and the remaining event kinds through subscriber lists — and queues the
//! matching events on each plugin's mailbox. A plugin is only woken for events it asked for;
//! for the indexed kinds the lookup cost is independent of the number of plugins.
//!
//! The module is split by concern: [`envelope`] carries an event and its handler span,
//! [`subscriber`] owns one plugin's mailbox and delivery telemetry, [`command`] classifies the
//! line's IRC command, [`plan`] computes the per-line delivery checks, [`url`] retains the URLs
//! of a line for dispatch, and [`index`] holds the subscription index and the routing path.

mod command;
mod envelope;
mod index;
mod plan;
mod subscriber;
#[cfg(test)]
mod tests;
mod url;

use irc::proto::Message;

// `telemetry` classifies routes through `CommandRoute`, and both modules live inside this
// private module, so the re-export stays crate-internal.
pub(crate) use command::CommandRoute;
pub use envelope::Envelope;
pub use index::EventIndex;

use crate::telemetry::handler_spans_enabled;

/// Returns the cached handler-span gate of the current line.
///
/// `EventIndex::dispatch` resolves the filter once, after
/// [`DeliveryPlan::may_deliver`](plan::DeliveryPlan::may_deliver), and seeds this cache before
/// routing; every delivery point reads the same value instead of consulting the filter again.
/// A line no subscription can receive returns before the gate is resolved.
#[inline]
fn telemetry_enabled(cache: &mut Option<bool>) -> bool {
    *cache.get_or_insert_with(handler_spans_enabled)
}

/// Emits the raw IRC line at trace level.
///
/// One callsite is shared by the no-delivery early return and the routed path. On a routed
/// line the event is inside `irc.dispatch`; on a line no subscription can receive it is outside
/// a span, so it reaches stdout but not the span exporter.
///
/// `event!` rather than `trace!`: the shorthand macro cannot parse the dotted `irc.raw` field
/// together with a `target:` prefix.
#[inline]
fn trace_raw(message: &Message) {
    tracing::event!(
        target: "zeta.plugins",
        tracing::Level::TRACE,
        { irc.raw = %message },
        "processing irc message"
    );
}
