use std::fmt::Write;

use crate::plugin::prelude::*;

/// The `.b` string to bytes command.
const BYTES: PluginCommand = PluginCommand::new(
    Prefix::new(".b"),
    "Show a string's UTF-8 bytes as hex escapes",
);
/// The `.len` string length command.
const LENGTH: PluginCommand = PluginCommand::new(
    Prefix::new(".len"),
    "Count the characters in a string",
);
/// The `.ord` character codepoint command.
const ORD: PluginCommand = PluginCommand::new(
    Prefix::new(".ord"),
    "Show the Unicode codepoint of each character",
);
/// The `.rev` string reverse command.
const REVERSE: PluginCommand = PluginCommand::new(
    Prefix::new(".rev"),
    "Reverse a string",
);
/// The `.uni` command (not implemented yet).
const UNICODE: PluginCommand = PluginCommand::new(
    Prefix::new(".uni"),
    "Show a character's Unicode properties (not implemented)",
);

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[BYTES, LENGTH, ORD, REVERSE, UNICODE];

pub struct StringUtils;

#[async_trait]
impl Plugin<Context> for StringUtils {
    fn new(_ctx: &Context) -> Result<StringUtils, ZetaError> {
        Ok(StringUtils::new())
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "string_utils".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn commands(&self) -> &'static [PluginCommand] {
        COMMANDS
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        channel: &str,
        command: &Prefix,
        args: &str,
    ) -> Result<(), ZetaError> {
        if args.is_empty() {
            return Self::usage(client, channel, command);
        }

        let reply = if *command == BYTES.prefix() {
            str_to_hex_string(args)
        } else if *command == LENGTH.prefix() {
            args.chars().count().to_string()
        } else if *command == ORD.prefix() {
            args.chars()
                .map(|x| (x as u32).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        } else if *command == REVERSE.prefix() {
            args.chars().rev().collect()
        } else {
            // Unhandled commands (including the not-yet-implemented `.uni`) are ignored.
            return Ok(());
        };

        client.send_privmsg(channel, formatted(&reply))?;

        Ok(())
    }
}

impl StringUtils {
    /// Replies with usage information for the invoked command.
    fn usage(client: &Client, channel: &str, command: &Prefix) -> Result<(), ZetaError> {
        let usage = if *command == BYTES.prefix() {
            "Usage: .b\x0f <byte..>"
        } else if *command == LENGTH.prefix() {
            "Usage: .len\x0f <string>"
        } else if *command == ORD.prefix() {
            "Usage: .ord\x0f <chars..>"
        } else if *command == REVERSE.prefix() {
            "Usage: .rev\x0f <string>"
        } else {
            return Ok(());
        };

        client.send_privmsg(channel, formatted(usage))?;

        Ok(())
    }
}

fn formatted(s: &str) -> String {
    format!("\x0310> {s}")
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
