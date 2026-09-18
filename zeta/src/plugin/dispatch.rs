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
use tracing::{debug, warn};
use zeta_plugin::event::{
    CommandEvent, CtcpEvent, JoinEvent, KickEvent, MessageEvent, NickEvent, PartEvent, QuitEvent,
    RawEvent, Sender, Subscriptions, UrlEvent,
};
use zeta_plugin::{CommandSpec, Event, UrlInterest};

use crate::plugin::filtering::Filters;
use crate::url::{ExtractUrls, ExtractedUrl, HTTP_SCHEMES};

/// A delivery endpoint for events: the mailbox of one plugin's task.
#[derive(Clone, Debug)]
struct Subscriber {
    /// The plugin's name, used for logging.
    name: Arc<str>,
    /// The mailbox events are queued on.
    mailbox: mpsc::UnboundedSender<Event>,
}

impl Subscriber {
    /// Queues `event` for the plugin, recording the plugin in `stopped` when its mailbox is
    /// closed (i.e. the plugin's task has stopped).
    fn send(&self, event: Event, stopped: &mut Vec<String>) {
        if let Err(error) = self.mailbox.send(event) {
            warn!(plugin = %self.name, %error, "plugin task has stopped");
            stopped.push(self.name.to_string());
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
    /// Plugins subscribed to every channel message.
    message: Vec<Subscriber>,
    /// Plugins subscribed to users joining channels.
    join: Vec<Subscriber>,
    /// Plugins subscribed to users leaving channels.
    part: Vec<Subscriber>,
    /// Plugins subscribed to users quitting the network.
    quit: Vec<Subscriber>,
    /// Plugins subscribed to nickname changes.
    nick: Vec<Subscriber>,
    /// Plugins subscribed to kicks.
    kick: Vec<Subscriber>,
    /// Plugins subscribed to CTCP messages.
    ctcp: Vec<Subscriber>,
    /// Plugins subscribed to raw, unmodeled IRC commands.
    raw: Vec<Subscriber>,
}

impl EventIndex {
    /// Adds a plugin to the index with the `subscriptions` it declared and its `mailbox`.
    ///
    /// Command triggers containing whitespace cannot be matched by the first-word index and
    /// are skipped with a warning.
    pub fn add(
        &mut self,
        name: &str,
        subscriptions: &Subscriptions,
        mailbox: mpsc::UnboundedSender<Event>,
    ) {
        let subscriber = Subscriber {
            name: name.into(),
            mailbox,
        };

        for command in subscriptions.commands() {
            if command.trigger().contains(char::is_whitespace) {
                warn!(
                    plugin = %name,
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

        match subscriptions.url_interest() {
            UrlInterest::None => {}
            UrlInterest::Hosts(hosts) => {
                for host in hosts {
                    self.url_hosts
                        .entry((*host).to_ascii_lowercase())
                        .or_default()
                        .push(subscriber.clone());
                }
            }
            UrlInterest::Any => self.url_any.push(subscriber.clone()),
        }

        if subscriptions.wants_messages() {
            self.message.push(subscriber.clone());
        }

        let presence = subscriptions.presence();

        if presence.join {
            self.join.push(subscriber.clone());
        }
        if presence.part {
            self.part.push(subscriber.clone());
        }
        if presence.quit {
            self.quit.push(subscriber.clone());
        }
        if presence.nick {
            self.nick.push(subscriber.clone());
        }
        if presence.kick {
            self.kick.push(subscriber.clone());
        }

        if subscriptions.wants_ctcp() {
            self.ctcp.push(subscriber.clone());
        }
        if subscriptions.wants_raw() {
            self.raw.push(subscriber);
        }
    }

    /// Routes `message` to the plugins that subscribed to events it matches.
    ///
    /// Returns the names of the plugins whose mailboxes are closed — their tasks have stopped,
    /// so the caller should evict them with [`EventIndex::evict`].
    pub fn dispatch(&self, filters: &Filters, message: Message) -> Vec<String> {
        let message = Arc::new(message);
        let mut stopped = Vec::new();

        match &message.command {
            Command::PRIVMSG(..) => self.dispatch_privmsg(filters, &message, &mut stopped),
            // CTCP replies arrive as NOTICE-wrapped messages; only CTCP subscribers see them —
            // ordinary notices are not an event kind.
            Command::NOTICE(..) if !self.ctcp.is_empty() => {
                if let Some(event) = CtcpEvent::new(Arc::clone(&message)) {
                    let event = Event::Ctcp(event);
                    Self::deliver(&event, &self.ctcp, &mut stopped);
                }
            }
            Command::JOIN(..) if !self.join.is_empty() => {
                let event = JoinEvent::new(Arc::clone(&message));
                Self::deliver(&Event::Join(event), &self.join, &mut stopped);
            }
            Command::PART(..) if !self.part.is_empty() => {
                let event = PartEvent::new(Arc::clone(&message));
                Self::deliver(&Event::Part(event), &self.part, &mut stopped);
            }
            Command::QUIT(..) if !self.quit.is_empty() => {
                let event = QuitEvent::new(Arc::clone(&message));
                Self::deliver(&Event::Quit(event), &self.quit, &mut stopped);
            }
            Command::NICK(..) if !self.nick.is_empty() => {
                let event = NickEvent::new(Arc::clone(&message));
                Self::deliver(&Event::Nick(event), &self.nick, &mut stopped);
            }
            Command::KICK(..) if !self.kick.is_empty() => {
                let event = KickEvent::new(Arc::clone(&message));
                Self::deliver(&Event::Kick(event), &self.kick, &mut stopped);
            }
            Command::Raw(..) if !self.raw.is_empty() => {
                let event = RawEvent::new(Arc::clone(&message));
                Self::deliver(&Event::Raw(event), &self.raw, &mut stopped);
            }
            // Everything else is connection protocol the plugins have no events for.
            _ => {}
        }

        stopped
    }

    /// Removes every trace of the named plugin from the index.
    ///
    /// Called when one of its deliveries failed because the plugin's task stopped.
    pub fn evict(&mut self, name: &str) {
        let other = |subscriber: &Subscriber| subscriber.name.as_ref() != name;

        self.commands.retain(|_, targets| {
            targets.retain(|target| other(&target.subscriber));
            !targets.is_empty()
        });

        self.url_hosts.retain(|_, subscribers| {
            subscribers.retain(|subscriber| other(subscriber));
            !subscribers.is_empty()
        });
        self.url_any.retain(|subscriber| other(subscriber));
        self.message.retain(|subscriber| other(subscriber));
        self.join.retain(|subscriber| other(subscriber));
        self.part.retain(|subscriber| other(subscriber));
        self.quit.retain(|subscriber| other(subscriber));
        self.nick.retain(|subscriber| other(subscriber));
        self.kick.retain(|subscriber| other(subscriber));
        self.ctcp.retain(|subscriber| other(subscriber));
        self.raw.retain(|subscriber| other(subscriber));
    }

    /// Delivers one event to every subscriber of a list.
    fn deliver(event: &Event, subscribers: &[Subscriber], stopped: &mut Vec<String>) {
        for subscriber in subscribers {
            subscriber.send(event.clone(), stopped);
        }
    }

    /// Routes a `PRIVMSG` to the plugins whose subscriptions match.
    fn dispatch_privmsg(&self, filters: &Filters, message: &Arc<Message>, stopped: &mut Vec<String>) {
        let Command::PRIVMSG(target, text) = &message.command else {
            return;
        };

        // CTCP messages are their own event kind and are routed to nothing else.
        if text.starts_with('\x01') {
            if let Some(event) = CtcpEvent::new(Arc::clone(message)) {
                let event = Event::Ctcp(event);
                Self::deliver(&event, &self.ctcp, stopped);
            }

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

        // Message subscribers observe every non-CTCP channel message.
        if !self.message.is_empty() {
            let event = Event::Message(MessageEvent::new(Arc::clone(message)));
            Self::deliver(&event, &self.message, stopped);
        }

        // URLs — extracted once per message, deduplicated and filtered before routing by host.
        if self.url_hosts.is_empty() && self.url_any.is_empty() {
            return;
        }

        let sender = Sender::from_message(message);
        let mut seen = HashSet::new();

        for ExtractedUrl { url, repaired_from } in ExtractUrls::with_schemes(text, HTTP_SCHEMES) {
            if !seen.insert(url.clone()) {
                continue;
            }

            if filters.is_filtered(target, sender, &url) {
                debug!(%url, "skipping filtered url");

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
    use zeta_plugin::CtcpKind;

    use super::*;

    /// Collects every event a test subscriber received from its mailbox.
    fn drain(mailbox: &mut mpsc::UnboundedReceiver<Event>) -> Vec<Event> {
        let mut events = Vec::new();

        while let Ok(event) = mailbox.try_recv() {
            events.push(event);
        }

        events
    }

    fn message(prefix: &str, command: &str, args: &[&str]) -> Message {
        let prefix = (!prefix.is_empty()).then_some(prefix);

        Message::new(prefix, command, args.to_vec()).expect("message should parse")
    }

    fn privmsg(text: &str) -> Message {
        message("nick!user@host.example", "PRIVMSG", &["#test", text])
    }

    /// Registers a test subscriber and returns the mailbox its events arrive on.
    fn subscribe(
        index: &mut EventIndex,
        name: &str,
        register: impl FnOnce(&mut Subscriptions),
    ) -> mpsc::UnboundedReceiver<Event> {
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

        assert!(stopped.is_empty());
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
            index.dispatch(&Filters::default(), privmsg(text));

            assert!(drain(&mut mailbox).is_empty(), "`{text}` must not dispatch");
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

        index.dispatch(&Filters::default(), privmsg(".gis black cats"));

        assert!(matches!(
            drain(&mut kagi)[..],
            [Event::Command(ref command)] if command.spec.trigger() == ".gis"
        ));
    }

    #[test]
    fn urls_route_to_registered_hosts_and_generic_handlers() {
        let mut index = EventIndex::default();
        let mut youtube = subscribe(&mut index, "youtube", |subscriptions| {
            subscriptions.url_hosts(&["youtube.com", "www.youtube.com"]);
        });
        let mut titles = subscribe(&mut index, "titles", |s| { s.url_any(); });

        // A claimed host is delivered to its plugin and to generic handlers.
        index.dispatch(&Filters::default(), privmsg("https://youtube.com/watch?v=1"));

        assert!(matches!(
            drain(&mut youtube)[..],
            [Event::Url(..)]
        ));
        assert!(matches!(
            drain(&mut titles)[..],
            [Event::Url(..)]
        ));

        // Host matching is case-insensitive.
        index.dispatch(&Filters::default(), privmsg("https://WWW.YOUTUBE.COM/x"));

        assert_eq!(drain(&mut youtube).len(), 1);
        assert_eq!(drain(&mut titles).len(), 1);

        // An unclaimed host only reaches generic handlers.
        index.dispatch(&Filters::default(), privmsg("https://example.com/x"));

        assert!(drain(&mut youtube).is_empty());
        assert_eq!(drain(&mut titles).len(), 1);
    }

    #[test]
    fn multiple_urls_yield_one_event_each() {
        let mut index = EventIndex::default();
        let mut titles = subscribe(&mut index, "titles", |s| { s.url_any(); });

        index.dispatch(
            &Filters::default(),
            privmsg("https://a.example/x https://a.example/x https://b.example/y"),
        );

        assert_eq!(drain(&mut titles).len(), 2, "duplicate URLs are deduplicated");
    }

    #[test]
    fn ctcp_messages_bypass_everything_else() {
        let mut index = EventIndex::default();
        let mut ctcp = subscribe(&mut index, "ctcp", |s| { s.ctcp(); });
        let mut messages = subscribe(&mut index, "messages", |s| { s.messages(); });
        let mut dig = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig"));
        });

        index.dispatch(&Filters::default(), privmsg("\x01ACTION slaps .dig\x01"));

        assert!(matches!(
            drain(&mut ctcp)[..],
            [Event::Ctcp(ref event)] if event.kind == CtcpKind::Action
        ));
        assert!(drain(&mut messages).is_empty());
        assert!(drain(&mut dig).is_empty());
    }

    #[test]
    fn ctcp_replies_arrive_through_notices() {
        let mut index = EventIndex::default();
        let mut ctcp = subscribe(&mut index, "ctcp", |s| { s.ctcp(); });
        let mut watcher = subscribe(&mut index, "watcher", |s| { s.messages(); });

        index.dispatch(
            &Filters::default(),
            message("service.example", "NOTICE", &["zeta", "\x01VERSION 1.0\x01"]),
        );

        assert!(matches!(
            drain(&mut ctcp)[..],
            [Event::Ctcp(ref event)] if event.kind == CtcpKind::Version
        ));
        assert!(drain(&mut watcher).is_empty());
    }

    #[test]
    fn plain_notices_are_not_delivered() {
        let mut index = EventIndex::default();
        let mut ctcp = subscribe(&mut index, "ctcp", |s| { s.ctcp(); });

        index.dispatch(
            &Filters::default(),
            message("server.example", "NOTICE", &["zeta", "server maintenance"]),
        );

        assert!(drain(&mut ctcp).is_empty());
    }

    #[test]
    fn messages_reach_subscribers_regardless_of_matches() {
        let mut index = EventIndex::default();
        let mut watcher = subscribe(&mut index, "watcher", |s| { s.messages(); });
        let mut dig = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig"));
        });

        index.dispatch(&Filters::default(), privmsg(".dig example.com"));

        assert!(matches!(
            drain(&mut dig)[..],
            [Event::Command(..)]
        ));
        assert!(matches!(
            drain(&mut watcher)[..],
            [Event::Message(..)]
        ));

        index.dispatch(&Filters::default(), privmsg("no events at all?"));

        assert!(drain(&mut dig).is_empty());
        assert_eq!(drain(&mut watcher).len(), 1);
    }

    #[test]
    fn presence_events_route_by_kind() {
        let mut index = EventIndex::default();
        let mut joins = subscribe(&mut index, "joins", |s| { s.join(); });
        let mut quits = subscribe(&mut index, "quits", |s| { s.quit(); });
        let mut parts = subscribe(&mut index, "parts", |s| { s.part(); });

        index.dispatch(&Filters::default(), message("nick!u@h", "JOIN", &["#test"]));
        index.dispatch(&Filters::default(), message("nick!u@h", "QUIT", &["gone"]));

        assert!(matches!(
            drain(&mut joins)[..],
            [Event::Join(ref join)] if join.channel() == "#test"
        ));
        assert!(matches!(
            drain(&mut quits)[..],
            [Event::Quit(ref quit)] if quit.reason() == Some("gone")
        ));
        assert!(drain(&mut parts).is_empty());
    }

    #[test]
    fn unmodeled_commands_reach_raw_subscribers() {
        let mut index = EventIndex::default();
        let mut raw = subscribe(&mut index, "raw", |s| { s.raw(); });

        index.dispatch(
            &Filters::default(),
            message("server.example", "SOMECMD", &["#test", "data"]),
        );

        assert!(matches!(
            drain(&mut raw)[..],
            [Event::Raw(ref event)] if event.command() == "SOMECMD"
        ));
    }

    #[test]
    fn stopped_plugins_are_evicted() {
        let mut index = EventIndex::default();
        let dig = subscribe(&mut index, "dig", |subscriptions| {
            subscriptions.command(CommandSpec::new(".dig", "dig")).join();
        });

        drop(dig);

        let stopped = index.dispatch(&Filters::default(), privmsg(".dig example.com"));
        assert_eq!(stopped, ["dig"]);

        index.evict("dig");

        // The evicted plugin receives nothing further.
        let stopped = index.dispatch(&Filters::default(), message("n!u@h", "JOIN", &["#test"]));

        assert!(stopped.is_empty());
    }
}
