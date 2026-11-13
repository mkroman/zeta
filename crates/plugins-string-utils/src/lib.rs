//! Offline string utilities.
//!
//! `.b <string>` prints the string's UTF-8 bytes as `\x..` hex escapes, `.len <string>` counts
//! its characters, `.ord <chars>` lists each character's Unicode codepoint, `.rev <string>`
//! reverses it, and `.uni <chars>` lists every character's name — the emoji name where one
//! exists — and general category, naming each run of characters that forms an emoji sequence
//! as a whole. An empty argument replies with the command's usage. Results are sent as a
//! plain reply, which supplies each reply's `\x0310> ` prefix.

use std::fmt::Write;
use std::sync::Once;

use zeta_plugin::irc::{RESET, plain_reply, send_reply};
use zeta_plugin::{Client, CommandEvent, CommandSpec, Context, Subscriptions, ZetaError, plugin};

mod unicode;

/// One command this plugin serves: its specification, usage arguments and renderer.
struct Command {
    /// The registered command.
    spec: CommandSpec,
    /// The arguments shown after the trigger in the usage line.
    usage: &'static str,
    /// Renders one reply body from the argument text.
    render: fn(&str) -> String,
}

/// Defines the fixed string commands and the [`COMMANDS`] table from one list of entries.
///
/// Keeping both in one place means a command cannot be registered without its renderer, or
/// rendered without being registered.
macro_rules! string_commands {
    ($($spec:ident => $trigger:literal, $description:literal, $usage:literal, $render:path;)*) => {
        $(
            #[doc = concat!("The `", $trigger, "` command: ", $description, ".")]
            const $spec: CommandSpec = CommandSpec::new($trigger, $description);
        )*

        /// Every command this plugin registers, in usage order.
        const COMMANDS: &[Command] = &[
            $(Command {
                spec: $spec,
                usage: $usage,
                render: $render,
            },)*
        ];
    };
}

string_commands! {
    BYTES => ".b", "Show a string's UTF-8 bytes as hex escapes", " <byte..>", str_to_hex;
    LENGTH => ".len", "Count the characters in a string", " <string>", character_count;
    ORD => ".ord", "Show the Unicode codepoint of each character", " <chars..>", code_points;
    REVERSE => ".rev", "Reverse a string", " <string>", reverse;
    UNICODE => ".uni", "Show a character's Unicode properties", " <chars>", unicode::unicode_details;
}

/// Guards the one-time background warm-up of the Unicode tables.
static UNICODE_INIT: Once = Once::new();

/// The string utilities plugin: offline text transformations for its commands.
#[derive(Default)]
pub struct StringUtils;

#[plugin(name = "string_utils")]
impl StringUtils {
    fn new(subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        for command in COMMANDS {
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

        send_reply(
            client,
            command.channel(),
            plain_reply((definition.render)(command.args())),
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

        send_reply(client, channel, plain_reply(usage))?;

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
    // A code point is at most seven decimal digits plus its separator.
    let mut out = String::with_capacity(text.chars().count().saturating_mul(9));

    for (index, c) in text.char_indices() {
        if index > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "{}", u32::from(c));
    }

    out
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
            tracing::warn!(exception.message = %error, "failed to spawn the Unicode warm-up thread");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_plugin::{NoSettings, Registered};

    #[test]
    fn str_to_hex_test() {
        assert_eq!(
            str_to_hex("🏳️‍🌈"),
            r"\xf0\x9f\x8f\xb3\xef\xb8\x8f\xe2\x80\x8d\xf0\x9f\x8c\x88"
        );
    }

    #[test]
    fn code_points_lists_every_codepoint() {
        assert_eq!(code_points("a🦔\u{0}"), "97, 129428, 0");
    }

    #[test]
    fn usage_lines_cover_every_command() {
        for command in COMMANDS {
            let expected = format!("Usage: {}{RESET}{}", command.spec.trigger(), command.usage);

            assert_eq!(
                StringUtils::usage_line(command.spec).as_deref(),
                Some(expected.as_str()),
                "{}",
                command.spec.trigger()
            );
        }

        // Pin the exact spacing of the reset code and the usage arguments.
        assert_eq!(
            StringUtils::usage_line(BYTES).as_deref(),
            Some("Usage: .b\x0f <byte..>")
        );
        assert_eq!(
            StringUtils::usage_line(CommandSpec::new(".nope", "not a command")),
            None
        );
    }

    #[tokio::test]
    async fn commands_are_registered() {
        let mut subscriptions = Subscriptions::new();

        <StringUtils as Registered>::new(
            &zeta_test_support::test_context(),
            &NoSettings {},
            &mut subscriptions,
        )
        .unwrap();

        for command in COMMANDS {
            assert!(subscriptions.commands().contains(&command.spec));
            assert!(
                StringUtils::usage_line(command.spec).is_some(),
                "{} has no usage line",
                command.spec.trigger()
            );
        }
    }
}
