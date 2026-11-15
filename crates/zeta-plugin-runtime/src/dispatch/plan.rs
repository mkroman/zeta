//! The per-line dispatch checks shared by the span guard and the routing path.

use irc::proto::{Command, Message};

use zeta_plugin::EventKind;
use zeta_plugin::event::is_ctcp;

use super::command::{CommandRoute, CommandTarget};
use super::index::EventIndex;

/// The per-line dispatch plan: the checks the span guard and the routing path share.
///
/// A line's CTCP status, matching command targets, message interest and URL potential are
/// computed once, from the still-unshared message, instead of being derived by the span guard
/// and then again by [`EventIndex::dispatch_privmsg`]. The command targets borrow the index, not
/// the message, so building the plan does not keep the message borrowed.
#[derive(Clone, Copy, Debug)]
pub(super) struct DeliveryPlan<'a> {
    /// The registered command targets the line's first word matches, if any.
    ///
    /// Kept as `&Vec` rather than `&[T]` so the option is one word: the plan then fits the
    /// register-return ABI, which the dispatch benchmark measures as a per-line saving.
    pub(super) command_targets: Option<&'a Vec<CommandTarget>>,
    /// The line's dispatch flags; see the bit constants below.
    flags: u8,
}

impl<'a> DeliveryPlan<'a> {
    /// The line carries a CTCP payload; CTCP lines bypass commands, messages and URLs.
    const CTCP: u8 = 1 << 0;
    /// Any plugin observes plain channel messages.
    const MESSAGE_SUBSCRIBERS: u8 = 1 << 1;
    /// The line could carry a URL for a URL subscriber: a scheme colon and a subscription.
    const URLS_POSSIBLE: u8 = 1 << 2;
    /// Any subscription can receive the line at all.
    const DELIVERABLE: u8 = 1 << 3;

    /// Returns `flag` when `condition` holds, or `0`.
    const fn bit(condition: bool, flag: u8) -> u8 {
        if condition { flag } else { 0 }
    }

    /// Builds the plan of `message` under `route`.
    #[inline]
    pub(super) fn new(index: &'a EventIndex, route: CommandRoute, message: &Message) -> Self {
        match route {
            CommandRoute::Ignored => Self::undeliverable(),
            CommandRoute::Event(kind) => {
                let is_ctcp = kind == EventKind::Ctcp && is_ctcp(message);
                let deliverable =
                    !index.subscribers(kind).is_empty() && (kind != EventKind::Ctcp || is_ctcp);

                Self {
                    command_targets: None,
                    flags: Self::bit(is_ctcp, Self::CTCP)
                        | Self::bit(deliverable, Self::DELIVERABLE),
                }
            }
            CommandRoute::Privmsg => {
                let Command::PRIVMSG(_, text) = &message.command else {
                    return Self::undeliverable();
                };

                let is_ctcp = is_ctcp(message);
                // A CTCP message bypasses commands, so matching one would be wasted work.
                let command_targets = if is_ctcp {
                    None
                } else {
                    index.matched_command_targets(text)
                };
                let has_message_subscribers = !index.subscribers(EventKind::Message).is_empty();
                // URL extraction needs a scheme, and a scheme contains `:`; a host-scoped
                // subscriber may still miss, which only costs one undelivered span. The map
                // checks come first so a line with no URL subscriber never scans its text.
                let urls_possible = (!index.url_hosts.is_empty() || !index.url_any.is_empty())
                    && text.contains(':');
                let deliverable = if is_ctcp {
                    !index.subscribers(EventKind::Ctcp).is_empty()
                } else {
                    has_message_subscribers || command_targets.is_some() || urls_possible
                };
                let flags = Self::bit(is_ctcp, Self::CTCP)
                    | Self::bit(has_message_subscribers, Self::MESSAGE_SUBSCRIBERS)
                    | Self::bit(urls_possible, Self::URLS_POSSIBLE)
                    | Self::bit(deliverable, Self::DELIVERABLE);

                Self {
                    command_targets,
                    flags,
                }
            }
        }
    }

    /// Returns a plan for a line no subscription can receive.
    const fn undeliverable() -> Self {
        Self {
            command_targets: None,
            flags: 0,
        }
    }

    /// Returns whether the line carries a CTCP payload.
    pub(super) const fn is_ctcp(&self) -> bool {
        self.flags & Self::CTCP != 0
    }

    /// Returns whether any plugin observes plain channel messages.
    pub(super) const fn has_message_subscribers(&self) -> bool {
        self.flags & Self::MESSAGE_SUBSCRIBERS != 0
    }

    /// Returns whether the line could carry a URL for a URL subscriber.
    pub(super) const fn urls_possible(&self) -> bool {
        self.flags & Self::URLS_POSSIBLE != 0
    }

    /// Returns whether any subscription can receive the planned line.
    ///
    /// The dispatcher uses this to skip the line span for lines that cannot deliver anything: a
    /// `PING`, a numeric, a `PRIVMSG` with no command, message or URL subscriber, or a plain
    /// `NOTICE` with no CTCP subscriber. It is deliberately conservative: a false positive only
    /// creates a span for a line that ends up delivering nothing, while a false negative would
    /// leave every handler span of the line without its parent. Filters are not consulted here;
    /// they can only remove deliveries, so ignoring them stays conservative.
    #[must_use]
    pub(super) const fn may_deliver(&self) -> bool {
        self.flags & Self::DELIVERABLE != 0
    }
}
