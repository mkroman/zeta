use rstest::*;
use zeta_irc::{IrcParser, Mode};

#[fixture]
fn parser() -> IrcParser {
    IrcParser::new(Mode::Strict)
}

#[rstest]
fn it_should_parse_message_tags_as_none_if_none_are_provided(parser: IrcParser) {
    let message = parser
        .parse(b":nick!ident@host.com PRIVMSG me :Hello")
        .expect("parsing failed");

    assert_eq!(message.tags(), None);
}

#[rstest]
fn it_should_parse_message_tags(parser: IrcParser) {
    let message = parser
        .parse(b"@a=a :nick!ident@host.com PRIVMSG me :Hello")
        .expect("parsing failed");

    assert!(message.tags().is_some());
}

#[rstest]
fn it_should_parse_opaque_tags(parser: IrcParser) {
    let message = parser
        .parse(b"@aaa=bbb;ccc;example.com/ddd=eee :nick!ident@host.com PRIVMSG me :Hello")
        .expect("parsing failed");

    let tags = message.tags().unwrap();

    assert_eq!(tags.get("ccc"), Some(&None));
}

#[rstest]
fn it_should_parse_message_tags_with_values(parser: IrcParser) {
    let message = parser
        .parse(b"@aaa=bbb;ccc;example.com/ddd=eee :nick!ident@host.com PRIVMSG me :Hello")
        .expect("parsing failed");

    let tags = message.tags().unwrap();

    assert_eq!(tags.get("aaa"), Some(&Some("bbb")));
    assert_eq!(tags.get("example.com/ddd"), Some(&Some("eee")));
}
