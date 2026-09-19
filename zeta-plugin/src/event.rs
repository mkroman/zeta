//! Zeta-specific events delivered to plugins.
//!
//! Instead of raw IRC protocol messages, plugins receive events — typed values derived from the
//! incoming IRC traffic they registered an interest in during initialization. A plugin declares
//! its interests in [`Plugin::new`](crate::Plugin::new) through a [`Subscriptions`] set;
//! the host only routes messages that match it, so a plugin's handlers are called only for
//! the events it asked for.
//!
//! Every event borrows its data from the IRC message it was derived from: the message is shared
//! behind an [`Arc`], and the event accessors (`channel()`, `text()`, `sender()`, …) return
//! borrowed views. Derived values such as command arguments are computed lazily from the shared
//! message, so delivering an event only clones an [`Arc`], never a string.
//!
//! Events are constructed by the host's dispatcher; the public constructors exist so tests (and
//! plugin authors) can build events to exercise handlers directly.

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::Arc;

use argh::FromArgs;
use irc::proto::{Command, Message};
use url::Url;

use crate::command::{ArgsError, CommandSpec};

pub use irc::proto::message::Tag;

/// Destructures a message into its `PRIVMSG` target and text.
///
/// The host only constructs `PRIVMSG`-derived events for matching messages; reaching another
/// command here is a host bug.
fn privmsg(message: &Message) -> (&str, &str) {
    match &message.command {
        Command::PRIVMSG(target, text) => (target, text),
        _ => unreachable!("events are only delivered for PRIVMSG messages"),
    }
}

/// Destructures a message into its target and text, whether it is a `PRIVMSG` or a `NOTICE`.
fn message_text(message: &Message) -> Option<(&str, &str)> {
    match &message.command {
        Command::PRIVMSG(target, text) | Command::NOTICE(target, text) => Some((target, text)),
        _ => None,
    }
}

/// Returns the IRCv3 message tags attached to `message`, if any.
fn tags(message: &Message) -> &[Tag] {
    message.tags.as_deref().unwrap_or(&[])
}

/// The sender of a message, as identified by its IRC prefix.
///
/// Events without a nickname prefix (e.g. server notices) have no sender; sender-scoped
/// filters never match without one.
#[derive(Clone, Copy, Debug)]
pub struct Sender<'a> {
    /// The nickname of the sender.
    pub nick: &'a str,
    /// The username (ident) of the sender.
    pub username: &'a str,
    /// The hostname of the sender.
    pub hostname: &'a str,
}

impl<'a> Sender<'a> {
    /// Constructs a sender from its parts.
    #[must_use]
    pub const fn new(nick: &'a str, username: &'a str, hostname: &'a str) -> Self {
        Self {
            nick,
            username,
            hostname,
        }
    }

    /// Returns the sender of `message`, or [`None`] for messages without a nickname prefix.
    #[must_use]
    pub fn from_message(message: &'a Message) -> Option<Sender<'a>> {
        let Some(irc::proto::Prefix::Nickname(nick, username, hostname)) = &message.prefix else {
            return None;
        };

        Some(Sender {
            nick,
            username,
            hostname,
        })
    }
}

/// Generates the sender and tags accessors shared by every event type.
///
/// Declared before its first use so every event struct can invoke it.
macro_rules! sender_and_tags {
    () => {
        /// Returns the sender of the message, if it has a nickname prefix.
        #[must_use]
        pub fn sender(&self) -> Option<Sender<'_>> {
            Sender::from_message(&self.message)
        }

        /// Returns the IRCv3 message tags attached to the message, if any.
        #[must_use]
        pub fn tags(&self) -> &[Tag] {
            tags(&self.message)
        }
    };
}

/// An event delivered to a plugin.
///
/// A plugin receives only the event kinds it registered during initialization; each kind is
/// handled through its own [`Plugin`](super::Plugin) method (`handle_command`, `handle_url`,
/// `handle_join`, ...), which is only called when the plugin registered that kind.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Event {
    /// A registered command was invoked.
    Command(CommandEvent),
    /// A URL on a registered host (or any URL, for generic handlers) was posted.
    Url(UrlEvent),
    /// A channel message was posted.
    Message(MessageEvent),
    /// A user joined a channel.
    Join(JoinEvent),
    /// A user left a channel.
    Part(PartEvent),
    /// A user quit the network.
    Quit(QuitEvent),
    /// A user changed their nickname.
    Nick(NickEvent),
    /// A user was kicked from a channel.
    Kick(KickEvent),
    /// A CTCP request or reply arrived.
    Ctcp(CtcpEvent),
    /// An unmodeled IRC command arrived.
    Raw(RawEvent),
}

/// A registered command was invoked in a channel.
///
/// The host matches the message against the [`CommandSpec`]s the plugin registered; the event
/// carries the specification that matched and the trailing, whitespace-normalized arguments.
#[derive(Clone, Debug)]
pub struct CommandEvent {
    message: Arc<Message>,
    /// The command specification that matched.
    pub spec: CommandSpec,
    /// The byte range of the arguments within the message text.
    args: Range<usize>,
}

impl CommandEvent {
    /// Creates a command event if `spec` matches the message's `PRIVMSG` text.
    #[must_use]
    pub fn new(message: Arc<Message>, spec: CommandSpec) -> Option<Self> {
        let text = privmsg(&message).1;
        let args = spec.parse(text)?;
        // The parsed arguments are a suffix of the input, so their range is derivable from
        // their length.
        let start = text.len() - args.len();
        let end = text.len();

        Some(Self {
            message,
            spec,
            args: start..end,
        })
    }

    /// Returns the channel the command was invoked in.
    #[must_use]
    pub fn channel(&self) -> &str {
        privmsg(&self.message).0
    }

    /// Returns the trailing arguments of the invocation, with leading whitespace stripped
    /// (empty when the command was invoked without arguments).
    #[must_use]
    pub fn args(&self) -> &str {
        &privmsg(&self.message).1[self.args.clone()]
    }

    /// Parses the trailing arguments into a [`FromArgs`]-derived struct.
    ///
    /// See [`CommandSpec::parse_args`] for the tokenization and error behavior.
    ///
    /// # Errors
    ///
    /// Returns [`ArgsError::Quoting`] if the arguments could not be tokenized, or
    /// [`ArgsError::Usage`] if `argh` rejected the arguments.
    pub fn parse_args<T: FromArgs>(&self) -> Result<T, ArgsError> {
        self.spec.parse_args(self.args())
    }

    /// Parses the trailing arguments into a [`FromArgs`]-derived struct, splitting them on
    /// whitespace.
    ///
    /// See [`CommandSpec::parse_words`] for the tokenization and error behavior.
    ///
    /// # Errors
    ///
    /// Returns [`ArgsError::Usage`] if `argh` rejected the arguments.
    pub fn parse_words<T: FromArgs>(&self) -> Result<T, ArgsError> {
        self.spec.parse_words(self.args())
    }

    sender_and_tags!();
}

/// A URL was posted in a channel.
///
/// The host extracts URLs once per message, applies the shared URL filters, and delivers one
/// event per URL to every plugin that subscribed to its host (or to any host, for generic
/// handlers).
#[derive(Clone, Debug)]
pub struct UrlEvent {
    message: Arc<Message>,
    url: Arc<Url>,
    /// The broken scheme prefix as posted, when extraction repaired it (e.g. `ttps`).
    pub repaired_from: Option<&'static str>,
}

impl UrlEvent {
    /// Creates a URL event.
    #[must_use]
    pub fn new(message: Arc<Message>, url: Arc<Url>, repaired_from: Option<&'static str>) -> Self {
        Self {
            message,
            url,
            repaired_from,
        }
    }

    /// Returns the URL.
    #[must_use]
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Returns the channel the URL was posted in.
    #[must_use]
    pub fn channel(&self) -> &str {
        privmsg(&self.message).0
    }

    /// Returns the full text of the message the URL was posted in.
    #[must_use]
    pub fn text(&self) -> &str {
        privmsg(&self.message).1
    }

    sender_and_tags!();
}

/// A channel message was posted.
///
/// Delivered to plugins that subscribed with [`Subscriptions::receive_message`]; every channel
/// `PRIVMSG` that is not a CTCP message produces one event, whether or not it matches any
/// registered command.
#[derive(Clone, Debug)]
pub struct MessageEvent {
    message: Arc<Message>,
}

impl MessageEvent {
    /// Creates a message event.
    #[must_use]
    pub fn new(message: Arc<Message>) -> Self {
        Self { message }
    }

    /// Returns the channel the message was posted in.
    #[must_use]
    pub fn channel(&self) -> &str {
        privmsg(&self.message).0
    }

    /// Returns the text of the message.
    #[must_use]
    pub fn text(&self) -> &str {
        privmsg(&self.message).1
    }

    sender_and_tags!();
}

/// A user joined a channel.
#[derive(Clone, Debug)]
pub struct JoinEvent {
    message: Arc<Message>,
}

impl JoinEvent {
    /// Creates a join event.
    #[must_use]
    pub fn new(message: Arc<Message>) -> Self {
        Self { message }
    }

    /// Returns the channel that was joined.
    #[must_use]
    pub fn channel(&self) -> &str {
        match &self.message.command {
            Command::JOIN(channel, ..) => channel,
            _ => unreachable!("join events are only delivered for JOIN messages"),
        }
    }

    /// Returns the nickname of the user that joined.
    #[must_use]
    pub fn nick(&self) -> &str {
        Sender::from_message(&self.message).map_or("", |sender| sender.nick)
    }

    sender_and_tags!();
}

/// A user left a channel.
#[derive(Clone, Debug)]
pub struct PartEvent {
    message: Arc<Message>,
}

impl PartEvent {
    /// Creates a part event.
    #[must_use]
    pub fn new(message: Arc<Message>) -> Self {
        Self { message }
    }

    /// Returns the channel that was left.
    #[must_use]
    pub fn channel(&self) -> &str {
        match &self.message.command {
            Command::PART(channel, ..) => channel,
            _ => unreachable!("part events are only delivered for PART messages"),
        }
    }

    /// Returns the parting comment, if any.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match &self.message.command {
            Command::PART(_, reason) => reason.as_deref(),
            _ => unreachable!("part events are only delivered for PART messages"),
        }
    }

    sender_and_tags!();
}

/// A user quit the network.
#[derive(Clone, Debug)]
pub struct QuitEvent {
    message: Arc<Message>,
}

impl QuitEvent {
    /// Creates a quit event.
    #[must_use]
    pub fn new(message: Arc<Message>) -> Self {
        Self { message }
    }

    /// Returns the quit message, if any.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match &self.message.command {
            Command::QUIT(reason) => reason.as_deref(),
            _ => unreachable!("quit events are only delivered for QUIT messages"),
        }
    }

    sender_and_tags!();
}

/// A user changed their nickname; the previous identity is available through
/// [`sender`](Self::sender).
#[derive(Clone, Debug)]
pub struct NickEvent {
    message: Arc<Message>,
}

impl NickEvent {
    /// Creates a nick change event.
    #[must_use]
    pub fn new(message: Arc<Message>) -> Self {
        Self { message }
    }

    /// Returns the new nickname.
    #[must_use]
    pub fn new_nick(&self) -> &str {
        match &self.message.command {
            Command::NICK(nick) => nick,
            _ => unreachable!("nick events are only delivered for NICK messages"),
        }
    }

    sender_and_tags!();
}

/// A user was kicked from a channel.
#[derive(Clone, Debug)]
pub struct KickEvent {
    message: Arc<Message>,
}

impl KickEvent {
    /// Creates a kick event.
    #[must_use]
    pub fn new(message: Arc<Message>) -> Self {
        Self { message }
    }

    /// Returns the channel the user was kicked from.
    #[must_use]
    pub fn channel(&self) -> &str {
        match &self.message.command {
            Command::KICK(channel, ..) => channel,
            _ => unreachable!("kick events are only delivered for KICK messages"),
        }
    }

    /// Returns the nickname of the user that was kicked.
    #[must_use]
    pub fn target(&self) -> &str {
        match &self.message.command {
            Command::KICK(_, target, _) => target,
            _ => unreachable!("kick events are only delivered for KICK messages"),
        }
    }

    /// Returns the kick reason, if any.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match &self.message.command {
            Command::KICK(.., reason) => reason.as_deref(),
            _ => unreachable!("kick events are only delivered for KICK messages"),
        }
    }

    sender_and_tags!();
}

/// A CTCP request or reply arrived.
///
/// Any `PRIVMSG` or `NOTICE` whose text is wrapped in CTCP `\x01` markers produces one event —
/// so such messages are never delivered as [`MessageEvent`]s or [`UrlEvent`]s.
#[derive(Clone, Debug)]
pub struct CtcpEvent {
    message: Arc<Message>,
    /// The kind of CTCP message.
    pub kind: CtcpKind,
}

impl CtcpEvent {
    /// Creates a CTCP event if `message` carries a CTCP payload.
    #[must_use]
    pub fn new(message: Arc<Message>) -> Option<Self> {
        let (_, text) = message_text(&message)?;
        let (command, _) = parse_ctcp(text)?;

        Some(Self {
            kind: CtcpKind::classify(command),
            message,
        })
    }

    /// Returns the target the CTCP message was sent to: the channel, or the recipient's
    /// nickname for direct queries.
    #[must_use]
    pub fn target(&self) -> &str {
        message_text(&self.message).expect("ctcp events are only delivered for PRIVMSG or NOTICE")
            .0
    }

    /// Returns the full, `\x01`-wrapped text of the CTCP message.
    #[must_use]
    pub fn text(&self) -> &str {
        message_text(&self.message).expect("ctcp events are only delivered for PRIVMSG or NOTICE")
            .1
    }

    /// Returns the CTCP command, e.g. `ACTION`.
    #[must_use]
    pub fn command(&self) -> &str {
        parse_ctcp(self.text()).map_or("", |(command, _)| command)
    }

    /// Returns the CTCP payload following the command (empty when there is none).
    #[must_use]
    pub fn args(&self) -> &str {
        parse_ctcp(self.text()).map_or("", |(_, args)| args)
    }

    sender_and_tags!();
}

/// The kind of a [`CtcpEvent`], classified from its CTCP command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CtcpKind {
    /// An action (`/me`) message.
    Action,
    /// A direct client-to-client connection request.
    Dcc,
    /// A latency probe.
    Ping,
    /// A client version query.
    Version,
    /// A local time query.
    Time,
    /// A query for the supported CTCP commands.
    Clientinfo,
    /// A client information query.
    Userinfo,
    /// Any other CTCP command.
    Other,
}

impl CtcpKind {
    /// Classifies a CTCP command, matching case-insensitively.
    #[must_use]
    pub fn classify(command: &str) -> Self {
        if command.eq_ignore_ascii_case("ACTION") {
            Self::Action
        } else if command.eq_ignore_ascii_case("DCC") {
            Self::Dcc
        } else if command.eq_ignore_ascii_case("PING") {
            Self::Ping
        } else if command.eq_ignore_ascii_case("VERSION") {
            Self::Version
        } else if command.eq_ignore_ascii_case("TIME") {
            Self::Time
        } else if command.eq_ignore_ascii_case("CLIENTINFO") {
            Self::Clientinfo
        } else if command.eq_ignore_ascii_case("USERINFO") {
            Self::Userinfo
        } else {
            Self::Other
        }
    }
}

/// An unmodeled IRC command arrived.
///
/// Delivered only to plugins that subscribed with [`Subscriptions::receive_raw`]; numeric
/// replies and
/// the connection's own protocol traffic are never delivered to plugins.
#[derive(Clone, Debug)]
pub struct RawEvent {
    message: Arc<Message>,
}

impl RawEvent {
    /// Creates a raw event.
    #[must_use]
    pub fn new(message: Arc<Message>) -> Self {
        Self { message }
    }

    /// Returns the raw command as received, e.g. `SOMECMD`.
    #[must_use]
    pub fn command(&self) -> &str {
        match &self.message.command {
            Command::Raw(command, _) => command,
            _ => unreachable!("raw events are only delivered for Raw commands"),
        }
    }

    /// Returns the arguments of the raw command.
    #[must_use]
    pub fn args(&self) -> &[String] {
        match &self.message.command {
            Command::Raw(_, args) => args,
            _ => unreachable!("raw events are only delivered for Raw commands"),
        }
    }

    /// Returns the full IRC protocol message.
    #[must_use]
    pub fn message(&self) -> &Message {
        &self.message
    }

    sender_and_tags!();
}

/// Splits the payload of a CTCP message into its command and arguments.
///
/// The text is `\x01`-wrapped (e.g. `"\x01ACTION slaps\x01"`); a missing trailing `\x01` is
/// tolerated, as some clients omit it.
fn parse_ctcp(text: &str) -> Option<(&str, &str)> {
    let body = text.strip_prefix('\x01')?;
    let body = body.strip_suffix('\x01').unwrap_or(body);

    Some(match body.split_once(' ') {
        Some((command, args)) => (command, args),
        None => (body, ""),
    })
}

/// The scope of URLs a plugin registered interest in: which posted URLs it wants to receive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UrlScope {
    /// The plugin does not handle URLs.
    #[default]
    None,
    /// The plugin handles URLs posted on the given hosts.
    ///
    /// Hosts are matched exactly after ASCII-lowercasing both sides, so list every variant that
    /// can appear in a posted URL (e.g. `imdb.com` as well as `www.imdb.com`).
    Hosts(&'static [&'static str]),
    /// The plugin handles every URL, including hosts other plugins handle.
    ///
    /// Generic handlers receive all URLs and apply their own exclusions (e.g. the hosts
    /// advertised by other plugins through the plugin catalog).
    Any,
}

/// The kind of an event a plugin subscribes to with a plain flag — one per per-kind handler.
///
/// Commands and URLs are excluded: their subscriptions carry payloads ([`CommandSpec`],
/// [`UrlScope`]) instead of a plain flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventKind {
    /// Channel messages ([`MessageEvent`]).
    Message,
    /// Users joining channels ([`JoinEvent`]).
    Join,
    /// Users leaving channels ([`PartEvent`]).
    Part,
    /// Users quitting the network ([`QuitEvent`]).
    Quit,
    /// Nickname changes ([`NickEvent`]).
    Nick,
    /// Users kicked from channels ([`KickEvent`]).
    Kick,
    /// CTCP messages ([`CtcpEvent`]).
    Ctcp,
    /// Raw, unmodeled IRC commands ([`RawEvent`]).
    Raw,
}

impl EventKind {
    /// Every kind, in declaration order.
    pub const ALL: [Self; 8] = [
        Self::Message,
        Self::Join,
        Self::Part,
        Self::Quit,
        Self::Nick,
        Self::Kick,
        Self::Ctcp,
        Self::Raw,
    ];
}

/// The events a plugin registers interest in during initialization.
///
/// The host passes a fresh, empty `Subscriptions` to
/// [`Plugin::new`](super::Plugin::new), where the plugin registers everything it wants to
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
/// Events of kinds the plugin did not register are never delivered to it, even though the
/// corresponding handler methods exist as no-op defaults.
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
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers interest in a command.
    ///
    /// Every channel message whose first word matches the command's trigger is delivered to
    /// [`Plugin::handle_command`](super::Plugin::handle_command).
    pub fn command(&mut self, command: CommandSpec) -> &mut Self {
        self.commands.push(command);
        self
    }

    /// Registers the scope of URLs the plugin receives.
    ///
    /// Calling it again replaces the previous scope.
    pub fn urls(&mut self, scope: UrlScope) -> &mut Self {
        self.urls = scope;
        self
    }

    /// Registers interest in every channel message, whether or not it matches a registered
    /// command.
    pub fn receive_message(&mut self) -> &mut Self {
        self.receive(EventKind::Message)
    }

    /// Registers interest in users joining channels.
    pub fn receive_join(&mut self) -> &mut Self {
        self.receive(EventKind::Join)
    }

    /// Registers interest in users leaving channels.
    pub fn receive_part(&mut self) -> &mut Self {
        self.receive(EventKind::Part)
    }

    /// Registers interest in users quitting the network.
    pub fn receive_quit(&mut self) -> &mut Self {
        self.receive(EventKind::Quit)
    }

    /// Registers interest in nickname changes.
    pub fn receive_nick(&mut self) -> &mut Self {
        self.receive(EventKind::Nick)
    }

    /// Registers interest in kicks.
    pub fn receive_kick(&mut self) -> &mut Self {
        self.receive(EventKind::Kick)
    }

    /// Registers interest in CTCP messages.
    pub fn receive_ctcp(&mut self) -> &mut Self {
        self.receive(EventKind::Ctcp)
    }

    /// Registers interest in raw, unmodeled IRC commands.
    pub fn receive_raw(&mut self) -> &mut Self {
        self.receive(EventKind::Raw)
    }

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
    pub fn events(&self) -> &BTreeSet<EventKind> {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(prefix: &str, command: &str, args: &[&str]) -> Arc<Message> {
        let prefix = (!prefix.is_empty()).then_some(prefix);

        Arc::new(Message::new(prefix, command, args.to_vec()).expect("message should parse"))
    }

    #[test]
    fn command_event_extracts_channel_and_args() {
        let message = message("nick!user@example.com", "PRIVMSG", &["#test", ".dig example.com"]);

        let event = CommandEvent::new(
            Arc::clone(&message),
            CommandSpec::new(".dig", "Look up DNS records for a domain"),
        )
        .expect("command should match");

        assert_eq!(event.channel(), "#test");
        assert_eq!(event.args(), "example.com");
        assert_eq!(event.sender().map(|sender| sender.nick), Some("nick"));
    }

    #[test]
    fn command_event_without_args_is_empty() {
        let message = message("nick!user@host", "PRIVMSG", &["#test", ".dig"]);

        let event = CommandEvent::new(message, CommandSpec::new(".dig", "dig"))
            .expect("command should match");

        assert_eq!(event.args(), "");
    }

    #[test]
    fn command_event_rejects_non_matching_text() {
        let message = message("nick!user@host", "PRIVMSG", &["#test", ".digg"]);

        assert!(
            CommandEvent::new(message, CommandSpec::new(".dig", "dig")).is_none(),
            "`.digg` must not match `.dig`"
        );
    }

    #[test]
    fn message_event_exposes_the_message() {
        let message = message("nick!~user@host.example", "PRIVMSG", &["#test", "hello"]);

        let event = MessageEvent::new(message);

        assert_eq!(event.channel(), "#test");
        assert_eq!(event.text(), "hello");
        assert_eq!(event.tags(), &[]);

        let sender = event.sender().expect("prefix should be a nickname");
        assert_eq!(sender.nick, "nick");
        assert_eq!(sender.hostname, "host.example");
    }

    #[test]
    fn url_event_exposes_the_url() {
        let message = message("nick!user@host", "PRIVMSG", &["#test", "see ttps://x.example"]);
        let url: Url = "https://x.example".parse().expect("url");

        let event = UrlEvent::new(message, Arc::new(url), Some("ttps"));

        assert_eq!(event.url().as_str(), "https://x.example/");
        assert_eq!(event.repaired_from, Some("ttps"));
        assert_eq!(event.text(), "see ttps://x.example");
    }

    #[test]
    fn ctcp_action_is_classified() {
        let message = message("nick!user@host", "PRIVMSG", &["#test", "\x01ACTION slaps\x01"]);

        let event = CtcpEvent::new(message).expect("ctcp should parse");

        assert_eq!(event.kind, CtcpKind::Action);
        assert_eq!(event.command(), "ACTION");
        assert_eq!(event.args(), "slaps");
        assert_eq!(event.target(), "#test");
        assert_eq!(event.text(), "\x01ACTION slaps\x01");
    }

    #[test]
    fn ctcp_without_payload_has_empty_args() {
        let message = message("nick!user@host", "PRIVMSG", &["#test", "\x01VERSION\x01"]);

        let event = CtcpEvent::new(message).expect("ctcp should parse");

        assert_eq!(event.kind, CtcpKind::Version);
        assert_eq!(event.command(), "VERSION");
        assert_eq!(event.args(), "");
    }

    #[test]
    fn ctcp_without_trailing_marker_is_tolerated() {
        let message = message("nick!user@host", "PRIVMSG", &["#test", "\x01PING 123"]);

        let event = CtcpEvent::new(message).expect("ctcp should parse");

        assert_eq!(event.command(), "PING");
        assert_eq!(event.args(), "123");
    }

    #[test]
    fn ctcp_replies_arrive_through_notices() {
        let message = message("nick!user@host", "NOTICE", &["zeta", "\x01VERSION 1.0\x01"]);

        let event = CtcpEvent::new(message).expect("ctcp should parse");

        assert_eq!(event.target(), "zeta");
        assert_eq!(event.command(), "VERSION");
    }

    #[test]
    fn plain_messages_are_not_ctcp() {
        let message = message("nick!user@host", "PRIVMSG", &["#test", "hello"]);

        assert!(CtcpEvent::new(message).is_none());
    }

    #[test]
    fn join_event_exposes_channel_and_nick() {
        let message = message("nick!user@host", "JOIN", &["#test"]);

        let event = JoinEvent::new(message);

        assert_eq!(event.channel(), "#test");
        assert_eq!(event.nick(), "nick");
    }

    #[test]
    fn part_event_exposes_reason() {
        let message = message("nick!user@host", "PART", &["#test", "bye"]);

        let event = PartEvent::new(message);

        assert_eq!(event.channel(), "#test");
        assert_eq!(event.reason(), Some("bye"));
    }

    #[test]
    fn quit_event_exposes_reason() {
        let message = message("nick!user@host", "QUIT", &["gone"]);

        let event = QuitEvent::new(message);

        assert_eq!(event.reason(), Some("gone"));
        assert_eq!(event.sender().map(|sender| sender.nick), Some("nick"));
    }

    #[test]
    fn nick_event_exposes_new_nick() {
        let message = message("old!user@host", "NICK", &["new"]);

        let event = NickEvent::new(message);

        assert_eq!(event.new_nick(), "new");
        assert_eq!(event.sender().map(|sender| sender.nick), Some("old"));
    }

    #[test]
    fn kick_event_exposes_target_and_reason() {
        let message = message("oper!user@host", "KICK", &["#test", "victim", "flood"]);

        let event = KickEvent::new(message);

        assert_eq!(event.channel(), "#test");
        assert_eq!(event.target(), "victim");
        assert_eq!(event.reason(), Some("flood"));
    }

    #[test]
    fn raw_event_exposes_the_command() {
        let message = message("server.example", "SOMECMD", &["#test", "data"]);

        let event = RawEvent::new(message);

        assert_eq!(event.command(), "SOMECMD");
        assert_eq!(event.args(), &["#test".to_owned(), "data".to_owned()]);
    }

    #[test]
    fn subscriptions_track_interests() {
        let mut subscriptions = Subscriptions::new();

        subscriptions
            .command(CommandSpec::new(".dig", "dig"))
            .command(CommandSpec::new(".ddo", "ddo"))
            .urls(UrlScope::Hosts(&["x.example", "www.x.example"]))
            .receive_message()
            .receive_join()
            .receive_kick()
            .receive_ctcp();

        assert_eq!(subscriptions.commands().len(), 2);
        assert_eq!(
            subscriptions.url_scope(),
            UrlScope::Hosts(&["x.example", "www.x.example"])
        );
        assert!(subscriptions.events().contains(&EventKind::Message));
        assert!(subscriptions.events().contains(&EventKind::Join));
        assert!(subscriptions.events().contains(&EventKind::Kick));
        assert!(!subscriptions.events().contains(&EventKind::Part));
        assert!(subscriptions.events().contains(&EventKind::Ctcp));
        assert!(!subscriptions.events().contains(&EventKind::Raw));
    }

    #[test]
    fn urls_any_overrides_hosts() {
        let mut subscriptions = Subscriptions::new();

        subscriptions
            .urls(UrlScope::Hosts(&["x.example"]))
            .urls(UrlScope::Any);

        assert_eq!(subscriptions.url_scope(), UrlScope::Any);
    }

    #[test]
    fn default_subscriptions_are_empty() {
        let subscriptions = Subscriptions::new();

        assert!(subscriptions.commands().is_empty());
        assert_eq!(subscriptions.url_scope(), UrlScope::None);
        assert!(subscriptions.events().is_empty());
    }
}
