//! Offline string utilities.
//!
//! `.b <string>` prints the string's UTF-8 bytes as `\x..` hex escapes, `.len <string>` counts
//! its characters, `.ord <chars>` lists each character's Unicode codepoint, and `.rev <string>`
//! reverses it. An empty argument replies with the command's usage. Results are sent as a
//! plain reply.
//!
//! The registered `.uni` command ("Show a character's Unicode properties") is not yet
//! implemented and is silently ignored.

use std::fmt::Write;

use crate::prelude::*;

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

/// The string utilities plugin: offline text transformations for its commands.
pub struct StringUtils;

impl Default for StringUtils {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin<Context> for StringUtils {
    type Settings = NoSettings;

    fn new(
        _ctx: &Context,
        _settings: &NoSettings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, PluginError> {
        subscriptions
            .subscribe_command(BYTES)
            .subscribe_command(LENGTH)
            .subscribe_command(ORD)
            .subscribe_command(REVERSE)
            .subscribe_command(UNICODE);
        Ok(Self::new())
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &IrcClient,
        command: &CommandEvent,
    ) -> Result<(), PluginError> {
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

        client.send_privmsg(command.channel(), plain_reply(&reply))?;

        Ok(())
    }
}

impl StringUtils {
    /// Replies with usage information for the invoked command.
    fn usage(client: &IrcClient, channel: &str, command: CommandSpec) -> Result<(), PluginError> {
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

        client.send_privmsg(channel, plain_reply(usage))?;

        Ok(())
    }
}

fn str_to_hex_string(s: &str) -> String {
    let mut buf = String::with_capacity(s.len() * 4);

    for b in s.bytes() {
        let _ = write!(buf, "\\x{b:x}");
    }

    buf
}

impl StringUtils {
    /// Creates a new string utils plugin instance.
    #[must_use]
    pub const fn new() -> Self {
        Self
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

    #[tokio::test]
    async fn commands_are_registered() {
        let mut subscriptions = Subscriptions::new();

        <StringUtils as Plugin<Context>>::new(
            &zeta_test_support::test_context(),
            &NoSettings,
            &mut subscriptions,
        )
        .unwrap();

        assert!(subscriptions.commands().contains(&BYTES));
        assert!(subscriptions.commands().contains(&LENGTH));
        assert!(subscriptions.commands().contains(&ORD));
        assert!(subscriptions.commands().contains(&REVERSE));
        assert!(subscriptions.commands().contains(&UNICODE));
    }
}
