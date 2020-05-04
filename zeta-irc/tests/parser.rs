use zeta_irc::{Error, IrcParser, Mode, Prefix};

/// Creates and returns a strict parser
fn strict_parser() -> IrcParser {
    IrcParser::new(Mode::Strict)
}

/// Creates and returns a lenient parser
fn lenient_parser() -> IrcParser {
    IrcParser::new(Mode::Lenient)
}

#[test]
fn it_should_return_length_error() {
    let result = strict_parser().parse(&[0; 16 * 1024]);

    assert_eq!(result.err(), Some(Error::LengthError));
}

#[test]
fn it_should_raise_error_when_strict_and_whitespace_prefix() {
    let result = strict_parser().parse(b"   :hi!hi@hi PRIVMSG #test hello");

    assert_eq!(result.err().map(|e| e.is_parse_error()), Some(true));
}

#[test]
fn it_should_not_raise_error_when_lenient_and_whitespace_prefix() {
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
fn it_should_parse_user_mask() {
    let res = strict_parser()
        .parse(b"@tag1=hello;tag2=hello2;tag3=hello3 :nick!user@example.com PRIVMSG #channel :hello, world!")
        .unwrap();

    assert_eq!(
        res.prefix(),
        Some(Prefix::UserMask {
            nick: &b"nick"[..],
            user: &b"user"[..],
            host: &b"example.com"[..]
        })
    );
}

#[test]
fn it_should_parse_params() {
    let res = strict_parser()
        .parse(b"@tag1=hello;tag2=hello2;tag3=hello3 :nick!user@example.com PRIVMSG #channel :hello, world!")
        .unwrap();

    assert_eq!(
        res.params(),
        Some(&vec![
            &b"PRIVMSG"[..],
            &b"#channel"[..],
            &b"hello, world!"[..]
        ])
    );
}

#[test]
fn it_should_parse_hostname() {
    let res = strict_parser()
        .parse(b"@tag1=hello;tag2=hello2;tag3=hello3 :server.example.com PRIVMSG #channel :hello, world!")
        .unwrap();

    assert_eq!(
        res.prefix(),
        Some(Prefix::HostName(&b"server.example.com"[..]))
    );
}

#[test]
fn it_should_extract_command() {
    let parser = strict_parser();
    let res = parser
        .parse(b"@tag1=hello;tag2=hello2 :nick!user@example.com PRIVMSG #channel :hello, world!")
        .unwrap();

    assert_eq!(res.command(), &b"PRIVMSG"[..]);

    let res = parser
        .parse(b"@tag1=hello;tag2=hello2;tag3=hello3 :nick!user@example.com PRIVMSG")
        .unwrap();

    assert_eq!(res.command(), &b"PRIVMSG"[..]);
}

#[test]
fn it_should_parse_freenode_log() {
    use std::collections::BTreeMap;
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use std::path::Path;

    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    struct Prefix {
        host: Option<String>,
        nick: Option<String>,
        user: Option<String>,
    }

    #[derive(Deserialize, Debug)]
    struct ParsedMessage {
        command: String,
        parameters: Vec<String>,
        tags: Option<BTreeMap<String, String>>,
        prefix: Prefix,
    }

    #[derive(Deserialize, Debug)]
    struct JSONLine {
        direction: String,
        data: String,
        parsed: Option<ParsedMessage>,
    }

    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("irc.freenode.net:6697.log");

    let parser = strict_parser();

    let file = BufReader::new(File::open(path).unwrap());

    for line in file.lines() {
        let json_line: JSONLine =
            serde_json::from_str(&line.unwrap()).expect("could not deserialize line");

        let result = parser.parse(json_line.data.as_ref()).unwrap();

        if let Some(command) = json_line.parsed.as_ref().map(|x| &x.command) {
            let bytes: &[u8] = command.as_ref();

            assert_eq!(&result.command(), &bytes);
        }

        if let Some(params) = json_line.parsed.as_ref().map(|x| &x.parameters) {
            let params_u8: Vec<&[u8]> = params.iter().map(|x| x.as_ref()).collect();
            let parsed_params = result.params().unwrap();

            assert_eq!(parsed_params[1..], params_u8[..]);
        }
    }
}

#[test]
fn it_should_parse_privmsg() {
    let res = strict_parser().parse(b":nick!user@example.com PRIVMSG #channel :hello, world!\r\n");

    assert!(res.is_ok());
}
