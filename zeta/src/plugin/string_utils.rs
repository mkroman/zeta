use std::fmt::Write;

use crate::plugin::prelude::*;

/// The `.b` string to bytes command.
const BYTES: CommandSpec = CommandSpec::new(".b", "Show a string's UTF-8 bytes as hex escapes");
/// The `.len` string length command.
const LENGTH: CommandSpec = CommandSpec::new(".len", "Count the characters in a string");
/// The `.ord` character codepoint command.
const ORD: CommandSpec = CommandSpec::new(".ord", "Show the Unicode codepoint of each character");
/// The `.rev` string reverse command.
const REVERSE: CommandSpec = CommandSpec::new(".rev", "Reverse a string");
/// The `.uni` command (not implemented yet).
const UNICODE: CommandSpec = CommandSpec::new(
    ".uni",
    "Show a character's Unicode properties (not implemented)",
);

pub struct StringUtils;

#[async_trait]
impl Plugin<Context> for StringUtils {
    type Settings = NoSettings;

    fn new(_ctx: &Context, _settings: &NoSettings, subscriptions: &mut Subscriptions) -> Result<StringUtils, ZetaError> {
        subscriptions
            .command(BYTES)
            .command(LENGTH)
            .command(ORD)
            .command(REVERSE)
            .command(UNICODE);
        Ok(StringUtils::new())
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        if command.args().is_empty() {
            return Self::usage(client, command.channel(), command.spec);
        }

        let reply = match command.spec {
            BYTES => str_to_hex_string(command.args()),
            LENGTH => command.args().chars().count().to_string(),
            ORD => command
                .args()
                .chars()
                .map(|x| (x as u32).to_string())
                .collect::<Vec<_>>()
                .join(", "),
            REVERSE => command.args().chars().rev().collect(),
            // Unhandled commands (including the not-yet-implemented `.uni`) are ignored.
            _ => return Ok(()),
        };

        client.send_privmsg(command.channel(), notice(&reply))?;

        Ok(())
    }
}

impl StringUtils {
    /// Replies with usage information for the invoked command.
    fn usage(client: &Client, channel: &str, command: CommandSpec) -> Result<(), ZetaError> {
        let usage = if command == BYTES {
            "Usage: .b\x0f <byte..>"
        } else if command == LENGTH {
            "Usage: .len\x0f <string>"
        } else if command == ORD {
            "Usage: .ord\x0f <chars..>"
        } else if command == REVERSE {
            "Usage: .rev\x0f <string>"
        } else {
            return Ok(());
        };

        client.send_privmsg(channel, notice(usage))?;

        Ok(())
    }
}

fn str_to_hex_string(s: &str) -> String {
    let mut buf = String::with_capacity(s.len() * 4);

    for b in s.bytes() {
        write!(buf, "\\x{b:x}").unwrap();
    }

    buf
}

impl StringUtils {
    pub const fn new() -> StringUtils {
        StringUtils
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_to_hex_string_test() {
        assert_eq!(
            str_to_hex_string("🏳️‍🌈"),
            r"\xf0\x9f\x8f\xb3\xef\xb8\x8f\xe2\x80\x8d\xf0\x9f\x8c\x88"
        );
    }

    #[test]
    fn commands_are_registered() {
        let plugin = StringUtils::new();

        assert!(plugin.commands().contains(&BYTES));
        assert!(plugin.commands().contains(&LENGTH));
        assert!(plugin.commands().contains(&ORD));
        assert!(plugin.commands().contains(&REVERSE));
        assert!(plugin.commands().contains(&UNICODE));
    }
}
