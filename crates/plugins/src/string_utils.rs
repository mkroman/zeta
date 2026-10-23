//! Offline string utilities.
//!
//! `.b <string>` prints the string's UTF-8 bytes as `\x..` hex escapes, `.len <string>` counts
//! its characters, `.ord <chars>` lists each character's Unicode codepoint, `.rev <string>`
//! reverses it, and `.uni <chars>` lists every character's name — the emoji name where one
//! exists — and general category, naming each run of characters that forms an emoji sequence
//! as a whole. An empty argument replies with the command's usage. Results are sent as a
//! notice, which supplies each reply's `\x0310> ` prefix.

use std::fmt::Write;

use crate::prelude::*;

mod unicode;

use unicode::unicode_details;

/// The `.b` string to bytes command.
const BYTES: CommandSpec = CommandSpec::new(".b", "Show a string's UTF-8 bytes as hex escapes");
/// The `.len` string length command.
const LENGTH: CommandSpec = CommandSpec::new(".len", "Count the characters in a string");
/// The `.ord` character codepoint command.
const ORD: CommandSpec = CommandSpec::new(".ord", "Show the Unicode codepoint of each character");
/// The `.rev` string reverse command.
const REVERSE: CommandSpec = CommandSpec::new(".rev", "Reverse a string");
/// The `.uni` Unicode properties command.
const UNICODE: CommandSpec = CommandSpec::new(".uni", "Show a character's Unicode properties");

/// The string utilities plugin: offline text transformations for its commands.
#[derive(Default)]
pub struct StringUtils;

#[plugin(name = "string_utils")]
impl StringUtils {
    fn new(subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions
            .command(BYTES)
            .command(LENGTH)
            .command(ORD)
            .command(REVERSE)
            .command(UNICODE);
        Ok(Self)
    }

    #[handler(command)]
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
            UNICODE => unicode_details(command.args()),
            // Unhandled commands are ignored.
            _ => return Ok(()),
        };

        client.send_privmsg(command.channel(), notice(&reply))?;

        Ok(())
    }
}

impl StringUtils {
    /// Replies with usage information for the invoked command.
    fn usage(client: &Client, channel: &str, command: CommandSpec) -> Result<(), ZetaError> {
        let Some(usage) = Self::usage_line(command) else {
            return Ok(());
        };

        client.send_privmsg(channel, notice(usage))?;

        Ok(())
    }

    /// The usage line of `command`, or `None` when it has none.
    fn usage_line(command: CommandSpec) -> Option<String> {
        let args = match command {
            BYTES => " <byte..>",
            LENGTH | REVERSE => " <string>",
            ORD => " <chars..>",
            UNICODE => " <chars>",
            _ => return None,
        };

        Some(format!("Usage: {}{RESET}{args}", command.trigger()))
    }
}

fn str_to_hex_string(s: &str) -> String {
    let mut buf = String::with_capacity(s.len() * 4);

    for b in s.bytes() {
        let _ = write!(buf, "\\x{b:x}");
    }

    buf
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
    fn usage_lines_cover_every_command() {
        assert_eq!(
            StringUtils::usage_line(BYTES).as_deref(),
            Some("Usage: .b\x0f <byte..>")
        );
        assert_eq!(
            StringUtils::usage_line(LENGTH).as_deref(),
            Some("Usage: .len\x0f <string>")
        );
        assert_eq!(
            StringUtils::usage_line(ORD).as_deref(),
            Some("Usage: .ord\x0f <chars..>")
        );
        assert_eq!(
            StringUtils::usage_line(REVERSE).as_deref(),
            Some("Usage: .rev\x0f <string>")
        );
        assert_eq!(
            StringUtils::usage_line(UNICODE).as_deref(),
            Some("Usage: .uni\x0f <chars>")
        );
        assert_eq!(
            StringUtils::usage_line(CommandSpec::new(".nope", "not a command")),
            None
        );
    }

    #[tokio::test]
    async fn commands_are_registered() {
        let mut subscriptions = Subscriptions::new();

        <StringUtils as Plugin<Context>>::new(
            &zeta_test_support::test_context(),
            &NoSettings {},
            &mut subscriptions,
        )
        .unwrap();

        for command in [BYTES, LENGTH, ORD, REVERSE, UNICODE] {
            assert!(subscriptions.commands().contains(&command));
            assert!(
                StringUtils::usage_line(command).is_some(),
                "{} has no usage line",
                command.trigger()
            );
        }
    }
}
