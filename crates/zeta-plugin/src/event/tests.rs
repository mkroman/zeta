//! Unit tests for the event vocabulary: per-kind accessors, subscriptions, message ids and
//! messaging operations.

use std::sync::Arc;

use ::url::Url;
use irc::proto::Message;
use irc::proto::message::Tag;
use zeta_test_support::assert_empty;

use crate::command::CommandSpec;

use super::*;

fn message(prefix: &str, command: &str, args: &[&str]) -> Arc<Message> {
    let prefix = (!prefix.is_empty()).then_some(prefix);

    Arc::new(Message::new(prefix, command, args.to_vec()).expect("message should parse"))
}

#[test]
fn command_event_extracts_channel_and_args() {
    let message = message(
        "nick!user@example.com",
        "PRIVMSG",
        &["#test", ".dig example.com"],
    );

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

    let event =
        CommandEvent::new(message, CommandSpec::new(".dig", "dig")).expect("command should match");

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
fn command_event_rejects_messages_that_are_not_privmsgs() {
    let message = message("nick!user@host", "JOIN", &["#test"]);

    assert!(
        CommandEvent::new(message, CommandSpec::new(".dig", "dig")).is_none(),
        "a JOIN cannot carry a command invocation"
    );
}

#[test]
fn message_event_exposes_the_message() {
    let message = message("nick!~user@host.example", "PRIVMSG", &["#test", "hello"]);

    let event = MessageEvent::new(message);

    assert_eq!(event.channel(), "#test");
    assert_eq!(event.text(), "hello");

    let sender = event.sender().expect("prefix should be a nickname");
    assert_eq!(sender.nick, "nick");
    assert_eq!(sender.hostname, "host.example");
}

#[test]
fn url_event_exposes_the_url() {
    let message = message(
        "nick!user@host",
        "PRIVMSG",
        &["#test", "see ttps://x.example"],
    );
    let url: Url = "https://x.example".parse().expect("url");

    let event = UrlEvent::new(message, Arc::new(url), Some("ttps"));

    assert_eq!(event.url().as_str(), "https://x.example/");
    assert_eq!(event.repaired_from, Some("ttps"));
    assert_eq!(event.text(), "see ttps://x.example");
}

#[test]
fn url_event_shares_its_url() {
    let message = message(
        "nick!user@host",
        "PRIVMSG",
        &["#test", "see https://x.example"],
    );
    let event = UrlEvent::new(
        message,
        Arc::new("https://x.example".parse().expect("url")),
        None,
    );

    let first = event.url_arc();
    let cloned = event.clone();
    let second = cloned.url_arc();

    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(event.url(), first.as_ref());
    assert_eq!(first.as_str(), "https://x.example/");
}

#[test]
fn ctcp_action_is_classified() {
    let message = message(
        "nick!user@host",
        "PRIVMSG",
        &["#test", "\x01ACTION slaps\x01"],
    );

    let event = CtcpEvent::new(message).expect("ctcp should parse");

    assert_eq!(event.kind, CtcpKind::Action);
    assert_eq!(event.command(), "ACTION");
    assert_eq!(event.args(), "slaps");
    assert_eq!(event.target(), "#test");
    assert_eq!(event.text(), "\x01ACTION slaps\x01");
}

#[test]
fn ctcp_kinds_are_classified_case_insensitively() {
    for (command, kind) in [
        ("ACTION", CtcpKind::Action),
        ("dcc", CtcpKind::Dcc),
        ("pInG", CtcpKind::Ping),
        ("Version", CtcpKind::Version),
        ("time", CtcpKind::Time),
        ("CLIENTINFO", CtcpKind::Clientinfo),
        ("UserInfo", CtcpKind::Userinfo),
        ("NOTAKIND", CtcpKind::Other),
    ] {
        assert_eq!(CtcpKind::classify(command), kind, "`{command}`");
    }
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

    assert_empty!(subscriptions.commands());
    assert_eq!(subscriptions.url_scope(), UrlScope::None);
    assert_empty!(subscriptions.events());
}

/// The IRCv3 `msgid` is read from the message tags of any command, and only when the tag has
/// a value.
#[test]
fn message_id_is_read_from_message_tags() {
    let tagged = |command: &str, args: Vec<&str>, value: Option<&str>| {
        Arc::new(
            Message::with_tags(
                Some(vec![Tag("msgid".to_owned(), value.map(ToOwned::to_owned))]),
                Some("nick!user@host"),
                command,
                args,
            )
            .expect("message should parse"),
        )
    };

    let privmsg = tagged("PRIVMSG", vec!["#test", "hello"], Some("m1"));
    assert_eq!(message_id(&privmsg), Some("m1"));

    let notice = tagged("NOTICE", vec!["#test", "hello"], Some("n1"));
    assert_eq!(message_id(&notice), Some("n1"));

    let join = tagged("JOIN", vec!["#test"], Some("j1"));
    assert_eq!(
        message_id(&join),
        Some("j1"),
        "servers may attach msgid to any event"
    );

    let tagmsg = tagged("TAGMSG", vec!["#test"], Some("t1"));
    assert_eq!(message_id(&tagmsg), Some("t1"));

    let valueless = tagged("PRIVMSG", vec!["#test", "hello"], None);
    assert_eq!(message_id(&valueless), None);

    let untagged = message("nick!user@host", "PRIVMSG", &["#test", "hello"]);
    assert_eq!(message_id(&untagged), None);
}

/// `Event::message_id` exposes the tag for every event kind, not just text messages.
#[test]
fn events_expose_the_message_id_of_any_kind() {
    let join = Event::Join(JoinEvent::new(Arc::new(
        Message::with_tags(
            Some(vec![Tag("msgid".to_owned(), Some("j1".to_owned()))]),
            Some("nick!user@host"),
            "JOIN",
            vec!["#test"],
        )
        .expect("message should parse"),
    )));

    assert_eq!(join.message_id(), Some("j1"));
}

/// Every event reports the IRC command it was derived from as its `OTel` messaging operation;
/// a CTCP reply reports `notice`, and an unmodeled command stays bounded by the command name.
#[test]
fn events_report_their_messaging_operation() {
    let privmsg = message("nick!user@host", "PRIVMSG", &["#test", ".dig example.com"]);

    let command = Event::Command(
        CommandEvent::new(Arc::clone(&privmsg), CommandSpec::new(".dig", "dig"))
            .expect("command should match"),
    );
    assert_eq!(
        command.messaging_operation(),
        MessagingOperation::Modeled("privmsg")
    );
    assert_eq!(command.messaging_operation().to_string(), "privmsg");

    let join = Event::Join(JoinEvent::new(message(
        "nick!user@host",
        "JOIN",
        &["#test"],
    )));
    assert_eq!(
        join.messaging_operation(),
        MessagingOperation::Modeled("join")
    );

    let quit = Event::Quit(QuitEvent::new(message("nick!user@host", "QUIT", &["gone"])));
    assert_eq!(
        quit.messaging_operation(),
        MessagingOperation::Modeled("quit")
    );

    let request = Event::Ctcp(
        CtcpEvent::new(message(
            "nick!user@host",
            "PRIVMSG",
            &["#test", "\x01VERSION\x01"],
        ))
        .expect("ctcp should parse"),
    );
    assert_eq!(
        request.messaging_operation(),
        MessagingOperation::Modeled("privmsg")
    );

    let reply = Event::Ctcp(
        CtcpEvent::new(message(
            "service.example",
            "NOTICE",
            &["zeta", "\x01VERSION 1.0\x01"],
        ))
        .expect("ctcp should parse"),
    );
    assert_eq!(
        reply.messaging_operation(),
        MessagingOperation::Modeled("notice")
    );

    let raw = Event::Raw(RawEvent::new(message(
        "server.example",
        "TAGMSG",
        &["#test"],
    )));
    assert_eq!(
        raw.messaging_operation(),
        MessagingOperation::Unmodeled("TAGMSG")
    );
    assert_eq!(
        raw.messaging_operation().to_string(),
        "tagmsg",
        "an unmodeled command is rendered lowercased"
    );
}

/// Lowercasing for the span field must happen once per event, not per subscriber: a known
/// or already-lowercase operation stays borrowed, and only an uppercase unmodeled command
/// allocates.
#[test]
fn messaging_operation_lowercases_once() {
    use std::borrow::Cow;

    assert!(matches!(
        MessagingOperation::Modeled("privmsg").into_cow(),
        Cow::Borrowed("privmsg")
    ));
    assert!(matches!(
        MessagingOperation::Unmodeled("tagmsg").into_cow(),
        Cow::Borrowed("tagmsg")
    ));
    assert!(matches!(
        MessagingOperation::Unmodeled("TAGMSG").into_cow(),
        Cow::Owned(name) if name == "tagmsg"
    ));
}

#[test]
fn event_kind_indices_follow_the_all_list() {
    for (index, kind) in EventKind::ALL.iter().enumerate() {
        assert_eq!(kind.index(), index, "{kind:?}");
    }
}
