//! The event envelope and the lazily shared IRC message of a dispatched line.

use std::sync::Arc;

use irc::proto::Message;
use tracing::Span;

use zeta_plugin::Event;

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

/// The IRC message being dispatched, shared with the events only when one is delivered.
///
/// Most incoming lines produce no event: no plugin subscribed to them, a `PRIVMSG` carries no
/// command or URL, or a `NOTICE` is not a CTCP message. Moving the message into an [`Arc`] on
/// the first delivery, instead of before routing, keeps those lines allocation-free; every
/// further event from the same line clones the shared handle.
pub(super) struct SharedMessage {
    /// The message, until the first event needs it.
    message: Option<Message>,
    /// The shared message, once an event needed it.
    shared: Option<Arc<Message>>,
}

impl SharedMessage {
    /// Wraps `message` without allocating.
    pub(super) const fn new(message: Message) -> Self {
        Self {
            message: Some(message),
            shared: None,
        }
    }

    /// Returns the message, whether or not it has been shared.
    pub(super) fn message(&self) -> &Message {
        self.shared
            .as_deref()
            .or(self.message.as_ref())
            .expect("the message is either unshared or shared")
    }

    /// Returns a handle to the shared message, allocating it on first use.
    pub(super) fn shared(&mut self) -> Arc<Message> {
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
