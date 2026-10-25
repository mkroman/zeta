//! Offline string utilities.
//!
//! `.b <string>` prints the string's UTF-8 bytes as `\x..` hex escapes, `.len <string>` counts
//! its characters, `.ord <chars>` lists each character's Unicode codepoint, `.rev <string>`
//! reverses it, and `.uni <chars>` lists every character's name — the emoji name where one
//! exists — and general category, naming each run of characters that forms an emoji sequence
//! as a whole. An empty argument replies with the command's usage. Results are sent as a
//! notice, which supplies each reply's `\x0310> ` prefix.

use std::fmt::Write;
use std::sync::Once;

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

/// One command this plugin serves: its specification, usage arguments and renderer.
struct Command {
    /// The registered command.
    spec: CommandSpec,
    /// The arguments shown after the trigger in the usage line.
    usage: &'static str,
    /// Renders one reply body from the argument text.
    render: fn(&str) -> String,
}

/// Every command this plugin registers, in usage order.
const COMMANDS: [Command; 5] = [
    Command {
        spec: BYTES,
        usage: " <byte..>",
        render: str_to_hex,
    },
    Command {
        spec: LENGTH,
        usage: " <string>",
        render: character_count,
    },
    Command {
        spec: ORD,
        usage: " <chars..>",
        render: code_points,
    },
    Command {
        spec: REVERSE,
        usage: " <string>",
        render: reverse,
    },
    Command {
        spec: UNICODE,
        usage: " <chars>",
        render: unicode_details,
    },
];

/// Guards the one-time background warm-up of the Unicode tables.
static UNICODE_INIT: Once = Once::new();

/// The string utilities plugin: offline text transformations for its commands.
#[derive(Default)]
pub struct StringUtils;

#[plugin(name = "string_utils")]
impl StringUtils {
    fn new(subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        for command in &COMMANDS {
            subscriptions.command(command.spec);
        }
        Ok(Self)
    }

    #[handler(loaded)]
    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        warm_unicode();
        Ok(())
    }

    #[handler(command)]
    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let Some(definition) = Self::find(command.spec) else {
            return Ok(());
        };

        if command.args().is_empty() {
            return Self::usage(client, command.channel(), command.spec);
        }

        client.send_privmsg(
            command.channel(),
            notice((definition.render)(command.args())),
        )?;

        Ok(())
    }
}

impl StringUtils {
    /// Finds the command definition for `spec`.
    fn find(spec: CommandSpec) -> Option<&'static Command> {
        COMMANDS.iter().find(|command| command.spec == spec)
    }

    /// Replies with usage information for the invoked command.
    fn usage(client: &Client, channel: &str, command: CommandSpec) -> Result<(), ZetaError> {
        let Some(usage) = Self::usage_line(command) else {
            return Ok(());
        };

        client.send_privmsg(channel, notice(usage))?;

        Ok(())
    }

    /// The usage line of `command`, or `None` when it is not a command of this plugin.
    fn usage_line(command: CommandSpec) -> Option<String> {
        let definition = Self::find(command)?;
        Some(format!(
            "Usage: {}{RESET}{}",
            definition.spec.trigger(),
            definition.usage
        ))
    }
}

/// Renders `.len`: the number of characters in `text`.
fn character_count(text: &str) -> String {
    text.chars().count().to_string()
}

/// Renders `.ord`: each character's code point, comma separated.
fn code_points(text: &str) -> String {
    text.chars()
        .map(|c| u32::from(c).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders `.rev`: the characters in reverse order.
fn reverse(text: &str) -> String {
    text.chars().rev().collect()
}

/// Renders `.b`: the UTF-8 bytes as `\x..` escapes.
fn str_to_hex(s: &str) -> String {
    let mut buf = String::with_capacity(s.len() * 4);

    for b in s.bytes() {
        let _ = write!(buf, "\\x{b:x}");
    }

    buf
}

/// Warms the process-wide emoji and general-category tables in the background.
///
/// `zeta_unicode::init` decodes those shared sections; the per-thread name decoder stays lazy
/// and builds on the handler thread's first name lookup. A failed spawn is harmless: the first
/// command then pays the lazy cost itself.
fn warm_unicode() {
    UNICODE_INIT.call_once(|| {
        let spawn = std::thread::Builder::new()
            .name("unicode-init".into())
            .spawn(zeta_unicode::init);
        if let Err(error) = spawn {
            tracing::warn!(%error, "failed to spawn the Unicode warm-up thread");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_to_hex_test() {
        assert_eq!(
            str_to_hex("🏳️‍🌈"),
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

        for command in &COMMANDS {
            assert!(subscriptions.commands().contains(&command.spec));
            assert!(
                StringUtils::usage_line(command.spec).is_some(),
                "{} has no usage line",
                command.spec.trigger()
            );
        }
    }
}
