//! Offline string utilities.
//!
//! `.b <string>` prints the string's UTF-8 bytes as `\x..` hex escapes, `.len <string>` counts
//! its characters, `.ord <chars>` lists each character's Unicode codepoint, `.rev <string>`
//! reverses it, and `.uni <chars>` lists every character's name — the emoji name where one
//! exists — and general category, naming each run of characters that forms an emoji sequence
//! as a whole. An empty argument replies with the command's usage. Results are sent as a
//! notice, which supplies each reply's `\x0310> ` prefix.

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
    fn usage_line(command: CommandSpec) -> Option<String> {
        let usage = match command {
            BYTES => format!("Usage: .b{RESET} <byte..>"),
            LENGTH => format!("Usage: .len{RESET} <string>"),
            ORD => format!("Usage: .ord{RESET} <chars..>"),
            REVERSE => format!("Usage: .rev{RESET} <string>"),
            UNICODE => format!("Usage: .uni{RESET} <chars>"),
            _ => return None,
        };

        Some(usage)
    }
}

/// The `\x0310> ` marker `notice()` prefixes every reply with, budgeted out of the listings.
const NOTICE_PREFIX_LEN: usize = REPLY_PREFIX.len() + 1;
/// Appended when a listing does not fit the budget.
const SUFFIX: &str = ", …";

/// Formats the reply to `.uni`.
///
/// `notice()` supplies the teal `> ` marker the reply opens with; the message itself lists one
/// entry per character, each entry naming its codepoint bold inside braces and its general
/// category in parentheses: `d` answers
/// `{\x0f\x02U+0064\x02\x0310}:\x0f LATIN SMALL LETTER D\x0310 (\x0fLowercase Letter\x0310)`.
/// An emoji code point is named by its emoji name, which is the more informative spelling for
/// the button and squared sets; a run of characters that names an emoji sequence of more than
/// one code point — `🏳️‍🌈` — is grouped into one entry that names the sequence as a whole and
/// carries the category of its base character. A listing that does not fit
/// [`MAX_LISTING_LENGTH`] once the notice prefix is added stops with [`SUFFIX`], and a grouped
/// sequence's codepoint list is budgeted the same way.
fn unicode_details(args: &str) -> String {
    let entries = grouped_entries(args);
    let total = entries.len();
    let mut message = String::new();
    let appended = append_entries_within_budget(
        &mut message,
        entries,
        ", ",
        MAX_LISTING_LENGTH - NOTICE_PREFIX_LEN,
        SUFFIX.len(),
    );

    if appended < total {
        message.push_str(SUFFIX);
    }

    message
}

/// Renders one entry per character of `args`, grouping each run of characters that names an
/// emoji sequence of at least two code points into a single entry named as a whole.
fn grouped_entries(args: &str) -> Vec<String> {
    let bounds: Vec<usize> = args
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(args.len()))
        .collect();
    let last = bounds.len() - 1;
    let mut entries = Vec::new();
    let mut at = 0usize;

    while at < last {
        // The longest run from here that names a sequence; a single code point is not a
        // grouping but a character entry of its own.
        let grouped = (at + 2..=last)
            .filter_map(|end| Some((end, emoji_name(&args[bounds[at]..bounds[end]])?)))
            .max_by_key(|&(end, _)| end);

        if let Some((end, name)) = grouped {
            let name = name.to_uppercase();
            entries.push(sequence_entry(&args[bounds[at]..bounds[end]], &name));
            at = end;
        } else {
            let c = args[bounds[at]..]
                .chars()
                .next()
                .expect("a boundary starts a character");
            entries.push(char_entry(c));
            at += 1;
        }
    }

    entries
}

/// Renders one grouped emoji sequence: its codepoints bold inside the braces, then the
/// sequence's name and the category of its base character.
///
/// The codepoint list is a listing too: canonicalization strips every `U+FE0F`, so the input
/// that matched can be much longer than the stored key, and the list stops with [`SUFFIX`]
/// inside the braces.
fn sequence_entry(text: &str, name: &str) -> String {
    let base = text.chars().next().expect("a sequence has a base character");
    let category = general_category(base).long_name();
    // Everything that may follow the list: the closing brace with the name and the base
    // character's category in parentheses.
    let framing = BOLD.len()
        + COLOR.len()
        + "}: ".len()
        + RESET.len()
        + name.len()
        + COLOR.len()
        + " (".len()
        + category.len()
        + COLOR.len()
        + ")".len();

    // The first codepoint is appended unconditionally, so the entry only fits while the name
    // leaves room for the prefix and `U+10FFFF` on top of the reserved bytes.
    debug_assert!(
        NOTICE_PREFIX_LEN + "{".len() + RESET.len() + BOLD.len() + "U+10FFFF".len()
            + SUFFIX.len() + framing
            <= MAX_LISTING_LENGTH,
        "the sequence name {name:?} does not leave room for the codepoint list"
    );

    let total = text.chars().count();
    let mut entry = format!("{{{RESET}{BOLD}");
    let appended = append_entries_within_budget(
        &mut entry,
        text.chars().map(|c| format!("U+{:04X}", u32::from(c))),
        ", ",
        MAX_LISTING_LENGTH - NOTICE_PREFIX_LEN,
        SUFFIX.len() + framing,
    );
    if appended < total {
        entry.push_str(SUFFIX);
    }
    let _ = write!(entry, "{BOLD}{COLOR}}}:{RESET} {name}{COLOR} ({RESET}{category}{COLOR})");

    entry
}

/// Renders one character entry: its codepoint bold inside the braces, then name and category.
///
/// An emoji code point is named by its emoji name — the CLDR spelling says what a character is
/// for (`AB button (blood type)`) where the character name spells its glyph (`NEGATIVE SQUARED
/// AB`) — falling back to [`name_or_label`] for everything else.
fn char_entry(c: char) -> String {
    let mut encoded = [0u8; 4];
    let name = emoji_name(c.encode_utf8(&mut encoded)).map_or_else(
        || name_or_label(c).into_owned(),
        |name| name.to_uppercase(),
    );
    let category = general_category(c).long_name();
    format!(
        "{{{RESET}{BOLD}U+{:04X}{BOLD}{COLOR}}}:{RESET} {name}{COLOR} ({RESET}{category}{COLOR})",
        u32::from(c),
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
            "{\x0f\x02U+0064\x02\x0310}:\x0f LATIN SMALL LETTER D\x0310 \
             (\x0fLowercase Letter\x0310)"
        );
        assert_eq!(
            unicode_details("ˇ"),
            "{\x0f\x02U+02C7\x02\x0310}:\x0f CARON\x0310 \
             (\x0fModifier Letter\x0310)"
        );
    }

    #[test]
    fn uni_names_an_emoji_sequence_as_a_whole() {
        assert_eq!(
            unicode_details("🏳️‍🌈"),
            "{\x0f\x02U+1F3F3, U+FE0F, U+200D, U+1F308\x02\x0310}:\x0f RAINBOW FLAG\x0310 \
             (\x0fOther Symbol\x0310)"
        );
        assert_eq!(
            unicode_details("2️⃣"),
            "{\x0f\x02U+0032, U+FE0F, U+20E3\x02\x0310}:\x0f KEYCAP DIGIT TWO\x0310 \
             (\x0fDecimal Number\x0310)"
        );
        // A single code point is a character entry of its own.
        assert_eq!(
            unicode_details("🐗"),
            "{\x0f\x02U+1F417\x02\x0310}:\x0f BOAR\x0310 (\x0fOther Symbol\x0310)"
        );
    }

    #[test]
    fn uni_names_an_emoji_code_point_by_its_emoji_name() {
        // `NEGATIVE SQUARED AB` is the character name; the emoji name says what it is for.
        assert_eq!(
            unicode_details("🆎"),
            "{\x0f\x02U+1F18E\x02\x0310}:\x0f AB BUTTON (BLOOD TYPE)\x0310 \
             (\x0fOther Symbol\x0310)"
        );
    }

    #[test]
    fn uni_groups_a_sequence_beside_a_character() {
        assert_eq!(
            unicode_details("🏳️‍🌈🦔"),
            "{\x0f\x02U+1F3F3, U+FE0F, U+200D, U+1F308\x02\x0310}:\x0f RAINBOW FLAG\x0310 \
             (\x0fOther Symbol\x0310), \
             {\x0f\x02U+1F994\x02\x0310}:\x0f HEDGEHOG\x0310 (\x0fOther Symbol\x0310)"
        );
    }

    #[test]
    fn uni_lists_single_emoji_as_characters() {
        assert_eq!(
            unicode_details("🦔🫠"),
            "{\x0f\x02U+1F994\x02\x0310}:\x0f HEDGEHOG\x0310 (\x0fOther Symbol\x0310), \
             {\x0f\x02U+1FAE0\x02\x0310}:\x0f MELTING FACE\x0310 (\x0fOther Symbol\x0310)"
        );
    }

    #[test]
    fn uni_lists_every_characters_name_and_category() {
        assert_eq!(
            unicode_details("d'êtr"),
            "{\x0f\x02U+0064\x02\x0310}:\x0f LATIN SMALL LETTER D\x0310 \
             (\x0fLowercase Letter\x0310), \
             {\x0f\x02U+0027\x02\x0310}:\x0f APOSTROPHE\x0310 (\x0fOther Punctuation\x0310), \
             {\x0f\x02U+00EA\x02\x0310}:\x0f LATIN SMALL LETTER E WITH CIRCUMFLEX\x0310 \
             (\x0fLowercase Letter\x0310), \
             {\x0f\x02U+0074\x02\x0310}:\x0f LATIN SMALL LETTER T\x0310 \
             (\x0fLowercase Letter\x0310), \
             {\x0f\x02U+0072\x02\x0310}:\x0f LATIN SMALL LETTER R\x0310 \
             (\x0fLowercase Letter\x0310)"
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
    fn uni_budgets_a_padded_emoji_sequence() {
        // `U+FE0F` is stripped by canonicalization, so this still matches the rainbow
        // flag; without a budget the points list would be much longer than the input.
        let padded = format!("🏳️‍🌈{}", "\u{FE0F}".repeat(60));
        let reply = unicode_details(&padded);

        assert!(reply.contains('…'), "reply was not truncated: {reply}");
        assert!(reply.len() <= MAX_LISTING_LENGTH, "{reply}");
        assert!(reply.ends_with("Other Symbol\x0310)"), "{reply}");
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
