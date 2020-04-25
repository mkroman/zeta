use zeta_irc::{Error, IrcParser, Strictness};

/// Creates and returns a strict parser
fn strict_parser() -> IrcParser {
    IrcParser::new(Strictness::Strict)
}

/// Creates and returns a lenient parser
fn lenient_parser() -> IrcParser {
    IrcParser::new(Strictness::Lenient)
}

#[test]
fn should_return_length_error() {
    let result = strict_parser().parse(&[0; 16 * 1024]);

    assert_eq!(result.err(), Some(Error::LengthError));
}

#[test]
fn should_raise_error_when_strict_and_whitespace_prefix() {
    let result = strict_parser().parse(b"   :hi!hi@hi PRIVMSG #test hello");

    assert_eq!(result.err().map(|e| e.is_parse_error()), Some(true));
}

#[test]
fn should_not_raise_error_when_lenient_and_whitespace_prefix() {
    let result = lenient_parser().parse(b"   :hi!hi@hi PRIVMSG #test hello");

    assert_eq!(result.err(), None);
}

#[test]
fn it_should_fail_with_iso_8559_1_tags() {
    let res = strict_parser()
        .parse(b"@tag=\xE6\xF8\xE5 :nick!user@example.com PRIVMSG #channel :hello, world!");

    assert_eq!(res.err().map(|e| e.is_encoding_error()), Some(true));
}

#[test]
fn it_should_extract_message_tags() {
    let res = strict_parser()
        .parse(b"@tag1=hello @tag2=hello2 @tag3=hello3 :nick!user@example.com PRIVMSG #channel :hello, world!")
        .unwrap();

    assert_eq!(res.tags(), Some("@tag1=hello @tag2=hello2 @tag3=hello3"));
}

#[test]
fn it_should_extract_prefix() {
    let res = strict_parser()
        .parse(b"@tag1=hello @tag2=hello2 @tag3=hello3 :nick!user@example.com PRIVMSG #channel :hello, world!")
        .unwrap();

    assert_eq!(res.prefix(), Some(&b"nick!user@example.com"[..]));
}

#[test]
fn it_should_extract_command() {
    let parser = strict_parser();
    let res = parser
        .parse(b"@tag1=hello @tag2=hello2 @tag3=hello3 :nick!user@example.com PRIVMSG #channel :hello, world!")
        .unwrap();

    assert_eq!(res.command(), &b"PRIVMSG"[..]);

    let res = parser
        .parse(b"@tag1=hello @tag2=hello2 @tag3=hello3 :nick!user@example.com PRIVMSG")
        .unwrap();

    assert_eq!(res.command(), &b"PRIVMSG"[..]);
}

#[test]
fn should_parse_privmsg() {
    let res = strict_parser().parse(b":nick!user@example.com PRIVMSG #channel :hello, world!\r\n");

    assert!(res.is_ok());
}
