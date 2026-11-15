//! The event set, its flag-based kinds, and the subscriptions a plugin registers.

use std::collections::BTreeSet;
use std::sync::Arc;

use irc::proto::{Command, Message};

use crate::command::CommandSpec;

use super::{
    CommandEvent, CtcpEvent, FromMessage, JoinEvent, KickEvent, MessageEvent, MessagingOperation,
    NickEvent, PartEvent, QuitEvent, RawEvent, UrlEvent, UrlScope, message_id,
};

/// Generates the [`EventKind`] flag enum, its [`EventKind::ALL`] list, and the matching
/// [`Subscriptions::receive_*`] registration methods from a single table — the enum, the list
/// and the registration methods cannot drift apart.
///
/// Each entry pairs the `EventKind` variant with its `Subscriptions::receive_*` method, the
/// doc fragments of both (the variant summary and "Registers interest in *…*."), and the
/// low-cardinality name the dispatcher puts in `otel.name` and `zeta.event.kind`.
///
/// The generated `Event` enum and the `#[handler(kind)]` routing in `zeta-plugin-macros` share
/// this table's vocabulary, but not this macro: the macro crate cannot invoke a
/// `macro_rules!` macro from here.
macro_rules! event_kind_flags {
    ($(
        $kind:ident(
            $event:ident, $receive:ident, $kind_doc:literal, $receive_doc:literal, $name:literal
            $(,)?
        )
    ),* $(,)?) => {
        /// An event delivered to a plugin.
        ///
        /// A plugin receives only the event kinds it registered during initialization; every
        /// event is passed to the plugin's single [`Plugin::handle`](crate::Plugin::handle),
        /// which is only called for the kinds the plugin registered.
        #[derive(Clone, Debug)]
        #[non_exhaustive]
        pub enum Event {
            /// A registered command was invoked.
            Command(CommandEvent),
            /// A URL on a registered host (or any URL, for generic handlers) was posted.
            Url(UrlEvent),
            $(
                #[doc = $kind_doc]
                $kind($event),
            )*
        }

        impl Event {
            /// Returns the flag-based kind of this event, or [`None`] for commands and URLs.
            ///
            /// Commands and URLs are subscribed to with payloads rather than a plain flag, so
            /// they have no [`EventKind`].
            #[must_use]
            pub const fn kind(&self) -> Option<EventKind> {
                match self {
                    Self::Command(_) | Self::Url(_) => None,
                    $(Self::$kind(_) => Some(EventKind::$kind),)*
                }
            }

            /// Returns a low-cardinality name for the event kind, for telemetry.
            #[must_use]
            pub const fn kind_name(&self) -> &'static str {
                match self {
                    Self::Command(_) => "command",
                    Self::Url(_) => "url",
                    $(Self::$kind(_) => EventKind::$kind.name(),)*
                }
            }

            /// Returns the channel the event belongs to, when it has one.
            #[must_use]
            pub fn channel(&self) -> Option<&str> {
                match self {
                    Self::Command(event) => Some(event.channel()),
                    Self::Url(event) => Some(event.channel()),
                    Self::Message(event) => Some(event.channel()),
                    Self::Join(event) => Some(event.channel()),
                    Self::Part(event) => Some(event.channel()),
                    Self::Kick(event) => Some(event.channel()),
                    Self::Ctcp(event) => Some(event.target()),
                    Self::Quit(_) | Self::Nick(_) | Self::Raw(_) => None,
                }
            }
        }

        /// The kind of an event a plugin subscribes to with a plain flag — one per kind-specific
        /// handler.
        ///
        /// Commands and URLs are excluded: their subscriptions carry payloads ([`CommandSpec`],
        /// [`UrlScope`]) instead of a plain flag.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        #[repr(usize)]
        pub enum EventKind {
            $(
                #[doc = $kind_doc]
                $kind,
            )*
        }

        impl EventKind {
            /// Every kind, in declaration and discriminator order.
            pub const ALL: &'static [Self] = &[$(Self::$kind,)*];

            /// Returns the kind's low-cardinality name, for telemetry.
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$kind => $name,)*
                }
            }

            /// Returns the kind's position in [`EventKind::ALL`].
            ///
            /// The discriminators are the declaration order, so an index into the dispatcher's
            /// per-kind arrays follows [`EventKind::ALL`] without a hand-written mapping.
            #[must_use]
            pub const fn index(self) -> usize {
                self as usize
            }

            /// Builds the event of this kind from `message`, or [`None`] when the message does
            /// not carry one (a `NOTICE` that is not a CTCP message).
            ///
            /// The caller is responsible for passing a message of the matching IRC command;
            /// building a `Message` event from a `JOIN` message, for example, produces an event
            /// whose accessors panic.
            #[must_use]
            pub fn event(self, message: Arc<Message>) -> Option<Event> {
                match self {
                    $(
                        Self::$kind => {
                            <$event as FromMessage>::from_message(message).map(Event::$kind)
                        }
                    )*
                }
            }
        }

        impl Subscriptions {
            $(
                #[doc = concat!("Registers interest in ", $receive_doc, ".")]
                pub fn $receive(&mut self) -> &mut Self {
                    self.receive(EventKind::$kind)
                }
            )*
        }
    };
}

event_kind_flags! {
    Message(MessageEvent, receive_message, "Channel messages ([`MessageEvent`]).",
        "every channel message, whether or not it matches a registered command", "message"),
    Join(JoinEvent, receive_join, "Users joining channels ([`JoinEvent`]).",
        "users joining channels", "join"),
    Part(PartEvent, receive_part, "Users leaving channels ([`PartEvent`]).",
        "users leaving channels", "part"),
    Quit(QuitEvent, receive_quit, "Users quitting the network ([`QuitEvent`]).",
        "users quitting the network", "quit"),
    Nick(NickEvent, receive_nick, "Nickname changes ([`NickEvent`]).",
        "nickname changes", "nick"),
    Kick(KickEvent, receive_kick, "Users kicked from channels ([`KickEvent`]).",
        "users kicked from channels", "kick"),
    Ctcp(CtcpEvent, receive_ctcp, "CTCP messages ([`CtcpEvent`]).",
        "CTCP messages", "ctcp"),
    Raw(RawEvent, receive_raw, "Raw, unmodeled IRC commands ([`RawEvent`]).",
        "raw, unmodeled IRC commands", "raw"),
}

impl Event {
    /// Returns the IRCv3 `msgid` of the message the event was derived from, when it has one.
    ///
    /// The identifier is the same on every event built from one message, so a handler can
    /// correlate its activity with the client's view of the message. See [`message_id`] for when
    /// a server attaches one.
    #[must_use]
    pub fn message_id(&self) -> Option<&str> {
        message_id(self.message())
    }

    /// Returns the IRC command the event was derived from, as an `OTel` messaging operation.
    ///
    /// `CommandEvent`, `UrlEvent` and `MessageEvent` always come from a `PRIVMSG`; CTCP events
    /// distinguish the `PRIVMSG` request from the `NOTICE` reply, and an unmodeled command keeps
    /// its received name, rendered lowercased by [`MessagingOperation`].
    #[must_use]
    pub fn messaging_operation(&self) -> MessagingOperation<'_> {
        match self {
            Self::Command(_) | Self::Url(_) | Self::Message(_) => {
                MessagingOperation::Modeled("privmsg")
            }
            Self::Join(_) => MessagingOperation::Modeled("join"),
            Self::Part(_) => MessagingOperation::Modeled("part"),
            Self::Quit(_) => MessagingOperation::Modeled("quit"),
            Self::Nick(_) => MessagingOperation::Modeled("nick"),
            Self::Kick(_) => MessagingOperation::Modeled("kick"),
            Self::Ctcp(event) => {
                if matches!(&event.message.command, Command::NOTICE(..)) {
                    MessagingOperation::Modeled("notice")
                } else {
                    MessagingOperation::Modeled("privmsg")
                }
            }
            Self::Raw(event) => MessagingOperation::Unmodeled(event.command()),
        }
    }

    /// Returns the message the event was derived from.
    fn message(&self) -> &Message {
        match self {
            Self::Command(event) => &event.message,
            Self::Url(event) => &event.message,
            Self::Message(event) => &event.message,
            Self::Join(event) => &event.message,
            Self::Part(event) => &event.message,
            Self::Quit(event) => &event.message,
            Self::Nick(event) => &event.message,
            Self::Kick(event) => &event.message,
            Self::Ctcp(event) => &event.message,
            Self::Raw(event) => &event.message,
        }
    }
}

/// The events a plugin registers interest in during initialization.
///
/// The host passes a fresh, empty `Subscriptions` to
/// [`Registered::new`](crate::Registered::new), where the plugin registers everything it wants to
/// receive:
///
/// ```
/// use zeta_plugin::{CommandSpec, Subscriptions, UrlScope};
///
/// const DIG: CommandSpec = CommandSpec::new(".dig", "Look up DNS records for a domain");
///
/// fn register(subscriptions: &mut Subscriptions) {
///     subscriptions
///         .command(DIG)
///         .urls(UrlScope::Hosts(&["example.com"]))
///         .receive_join();
/// }
///
/// let mut subscriptions = Subscriptions::new();
/// register(&mut subscriptions);
///
/// assert_eq!(subscriptions.commands(), &[DIG]);
/// assert_eq!(
///     subscriptions.url_scope(),
///     zeta_plugin::UrlScope::Hosts(&["example.com"])
/// );
/// ```
///
/// Events of kinds the plugin did not register are never delivered to it, even though every
/// event kind is routable through [`Plugin::handle`](crate::Plugin::handle).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Subscriptions {
    /// The commands the plugin handles.
    commands: Vec<CommandSpec>,
    /// The scope of URLs the plugin receives.
    urls: UrlScope,
    /// The event kinds the plugin receives, as a set.
    events: BTreeSet<EventKind>,
}

impl Subscriptions {
    /// Constructs an empty subscription set.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            commands: Vec::new(),
            urls: UrlScope::None,
            events: BTreeSet::new(),
        }
    }

    /// Registers interest in a command.
    ///
    /// Every channel message whose first word matches the command's trigger is delivered to
    /// [`Plugin::handle`](crate::Plugin::handle) as a
    /// [`CommandEvent`].
    pub fn command(&mut self, command: CommandSpec) -> &mut Self {
        self.commands.push(command);
        self
    }

    /// Registers the scope of URLs the plugin receives.
    ///
    /// Calling it again replaces the previous scope.
    pub const fn urls(&mut self, scope: UrlScope) -> &mut Self {
        self.urls = scope;
        self
    }

    // The public `receive_*` registration methods are generated together with the
    // `EventKind` enum by `event_kind_flags!`.
    /// Adds `kind` to the set of event kinds the plugin receives.
    fn receive(&mut self, kind: EventKind) -> &mut Self {
        self.events.insert(kind);
        self
    }

    /// Returns the registered commands.
    #[must_use]
    pub fn commands(&self) -> &[CommandSpec] {
        &self.commands
    }

    /// Returns the scope of URLs the plugin registered interest in.
    #[must_use]
    pub const fn url_scope(&self) -> UrlScope {
        self.urls
    }

    /// Returns the event kinds the plugin registered interest in.
    #[must_use]
    pub const fn events(&self) -> &BTreeSet<EventKind> {
        &self.events
    }
}
