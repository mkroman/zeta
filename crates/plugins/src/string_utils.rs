//! Offline string utilities.
//!
//! `.b <string>` prints the string's UTF-8 bytes as `\x..` hex escapes, `.len <string>` counts
//! its characters, `.ord <chars>` lists each character's Unicode codepoint, `.rev <string>`
//! reverses it, and `.uni <chars>` prints every character's Unicode name and general category —
//! or, when the argument is one emoji sequence, that sequence's name. An empty argument replies
//! with the command's usage. Results are sent as a notice.

use std::fmt::Write;

use zeta_unicode::{emoji_name, general_category, name_or_label};
use zeta_utils::utils::{MAX_LISTING_LENGTH, append_entries_within_budget};

use crate::prelude::*;

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
    ) -> Result<Self, ZetaError> {
        subscriptions
            .command(BYTES)
            .command(LENGTH)
            .command(ORD)
            .command(REVERSE)
            .command(UNICODE);
        Ok(Self::new())
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
    fn usage_line(command: CommandSpec) -> Option<&'static str> {
        let usage = match command {
            BYTES => "Usage: .b\x0f <byte..>",
            LENGTH => "Usage: .len\x0f <string>",
            ORD => "Usage: .ord\x0f <chars..>",
            REVERSE => "Usage: .rev\x0f <string>",
            UNICODE => "Usage: .uni\x0f <chars>",
            _ => return None,
        };

        Some(usage)
    }
}

/// Formats the reply to `.uni`.
///
/// When `args` is exactly one emoji sequence — `🏳️‍🌈`, `🫶🏼`, `2️⃣` — the reply names the
/// sequence as a whole: `{U+1F3F3, U+FE0F, U+200D, U+1F308}: RAINBOW FLAG`. Otherwise every
/// character gets its own entry, `U+0064: LATIN SMALL LETTER D (Lowercase Letter)`, and the
/// listing stops at [`MAX_LISTING_LENGTH`] with an ellipsis.
fn unicode_details(args: &str) -> String {
    const SUFFIX: &str = ", …";

    if let Some(sequence) = emoji_name(args) {
        let points = args
            .chars()
            .map(|c| format!("U+{:04X}", u32::from(c)))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("{{{points}}}:\x0f {}\x0310", sequence.to_uppercase());
    }

    let total = args.chars().count();
    let mut message = String::new();
    let appended = append_entries_within_budget(
        &mut message,
        args.chars().map(format_char_entry),
        ", ",
        MAX_LISTING_LENGTH,
        SUFFIX.len(),
    );

    if appended < total {
        message.push_str(SUFFIX);
    }

    message
}

/// Formats one `.uni` entry: `U+0064: LATIN SMALL LETTER D (Lowercase Letter)`.
fn format_char_entry(c: char) -> String {
    format!(
        "U+{:04X}:\x0f {}\x0310 ({})",
        u32::from(c),
        name_or_label(c),
        general_category(c).long_name()
    )
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

    #[test]
    fn uni_names_a_single_character() {
        assert_eq!(
            unicode_details("d"),
            "U+0064:\x0f LATIN SMALL LETTER D\x0310 (Lowercase Letter)"
        );
        assert_eq!(
            unicode_details("ˇ"),
            "U+02C7:\x0f CARON\x0310 (Modifier Letter)"
        );
    }

    #[test]
    fn uni_names_an_emoji_sequence_as_a_whole() {
        assert_eq!(unicode_details("🐗"), "{U+1F417}:\x0f BOAR\x0310");
        assert_eq!(
            unicode_details("🏳️‍🌈"),
            "{U+1F3F3, U+FE0F, U+200D, U+1F308}:\x0f RAINBOW FLAG\x0310"
        );
        assert_eq!(
            unicode_details("2️⃣"),
            "{U+0032, U+FE0F, U+20E3}:\x0f KEYCAP DIGIT TWO\x0310"
        );
    }

    #[test]
    fn uni_lists_every_characters_name_and_category() {
        assert_eq!(
            unicode_details("d'être"),
            "U+0064:\x0f LATIN SMALL LETTER D\x0310 (Lowercase Letter), \
             U+0027:\x0f APOSTROPHE\x0310 (Other Punctuation), \
             U+00EA:\x0f LATIN SMALL LETTER E WITH CIRCUMFLEX\x0310 (Lowercase Letter), \
             U+0074:\x0f LATIN SMALL LETTER T\x0310 (Lowercase Letter), \
             U+0072:\x0f LATIN SMALL LETTER R\x0310 (Lowercase Letter), \
             U+0065:\x0f LATIN SMALL LETTER E\x0310 (Lowercase Letter)"
        );
    }

    #[test]
    fn uni_stops_within_the_listing_budget() {
        assert!(!unicode_details("é").contains('…'));

        let repeated = "é".repeat(60);
        let reply = unicode_details(&repeated);
        assert!(reply.contains('…'), "reply was not truncated: {reply}");
        assert!(reply.ends_with(", …"));
        assert!(reply.len() <= MAX_LISTING_LENGTH + ", …".len());
    }

    #[test]
    fn usage_lines_cover_every_command() {
        assert_eq!(
            StringUtils::usage_line(BYTES),
            Some("Usage: .b\x0f <byte..>")
        );
        assert_eq!(
            StringUtils::usage_line(LENGTH),
            Some("Usage: .len\x0f <string>")
        );
        assert_eq!(
            StringUtils::usage_line(ORD),
            Some("Usage: .ord\x0f <chars..>")
        );
        assert_eq!(
            StringUtils::usage_line(REVERSE),
            Some("Usage: .rev\x0f <string>")
        );
        assert_eq!(
            StringUtils::usage_line(UNICODE),
            Some("Usage: .uni\x0f <chars>")
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

        assert!(subscriptions.commands().contains(&BYTES));
        assert!(subscriptions.commands().contains(&LENGTH));
        assert!(subscriptions.commands().contains(&ORD));
        assert!(subscriptions.commands().contains(&REVERSE));
        assert!(subscriptions.commands().contains(&UNICODE));
    }
}
