//! IRC formatting shared by bundled plugins.
//!
//! Plugin replies follow a common visual convention: a teal `>` marker (mIRC color 10), an
//! optional bold plugin name, and the message body with values emphasized by switching between
//! the reply color and the default color. The helpers here produce those byte sequences so the
//! exact escape codes are written down in one place.

use std::fmt;

use argh::FromArgs;
use irc::client::Client as IrcClient;

use crate::command::ArgumentError;
use crate::event::CommandEvent;

/// The teal color (mIRC color 10) used for plugin replies.
pub const REPLY_COLOR: &str = "\x0310";

/// Resets bold, color, and any other text formatting back to the default.
pub const RESET: &str = "\x0f";

/// Starts bold text.
pub const BOLD: &str = "\x02";

/// The prefix shared by every reply format, bold name or not.
pub const REPLY_MARKER: &str = "\x0310>";

/// Formats `message` as a reply from the plugin named `name`.
///
/// The reply starts with [`REPLY_MARKER`], followed by the bold plugin name, e.g.
/// `> Twitch: <message>`.
#[must_use]
pub fn reply(name: &str, message: impl fmt::Display) -> String {
    format!("{}{}", reply_prefix(name), message)
}

/// Returns the bold-name prefix that [`reply`] starts its replies with.
///
/// Unlike [`reply`], the prefix can be written to an incrementally built message, e.g. from a
/// `Display` implementation.
#[must_use]
pub fn reply_prefix(name: &str) -> String {
    format!("{REPLY_MARKER}{RESET}{BOLD} {name}:{BOLD}{REPLY_COLOR} ")
}

/// Formats `message` as a plain reply without a plugin name.
#[must_use]
pub fn plain_reply(message: impl fmt::Display) -> String {
    format!("{REPLY_MARKER} {message}")
}

/// Sends the output of a failed argument parse to `channel`, one message per non-empty line.
///
/// Parse failures produce the command's usage or help output, which spans multiple lines; IRC
/// messages cannot contain line breaks, so each line is sent as its own `PRIVMSG`, formatted by
/// `format` (e.g. by wrapping it in [`reply`] with the plugin's name).
///
/// # Errors
///
/// Returns any error produced while sending the messages.
pub fn reply_usage_lines(
    client: &IrcClient,
    channel: &str,
    error: &ArgumentError,
    format: impl Fn(&str) -> String,
) -> Result<(), irc::error::Error> {
    for line in error.to_string().lines().filter(|line| !line.is_empty()) {
        client.send_privmsg(channel, format(line))?;
    }

    Ok(())
}

/// Parses the trailing arguments of `command` with [`CommandEvent::parse_args`], replying with
/// the usage lines to the command's channel when the parse fails.
///
/// Unlike [`parse_words_or_usage`], the arguments are tokenized like a POSIX shell (via
/// [`CommandEvent::parse_args`]), so quoted arguments and escapes are supported.
///
/// Returns `Ok(None)` when the usage was replied to and the command should be abandoned.
///
/// # Errors
///
/// Returns any error produced while sending the usage lines.
pub fn parse_args_or_usage<T: FromArgs>(
    client: &IrcClient,
    command: &CommandEvent,
    format: impl Fn(&str) -> String,
) -> Result<Option<T>, irc::error::Error> {
    parse_or_usage(command.parse_args(), client, command.channel(), format)
}

/// Parses the trailing arguments of `command` with [`CommandEvent::parse_words`], replying with
/// the usage lines to the command's channel when the parse fails.
///
/// Returns `Ok(None)` when the usage was replied to and the command should be abandoned.
///
/// # Errors
///
/// Returns any error produced while sending the usage lines.
pub fn parse_words_or_usage<T: FromArgs>(
    client: &IrcClient,
    command: &CommandEvent,
    format: impl Fn(&str) -> String,
) -> Result<Option<T>, irc::error::Error> {
    parse_or_usage(command.parse_words(), client, command.channel(), format)
}

/// Replies `error`'s usage lines through `format` and yields `None` for the caller to abandon
/// the command with, or passes a successful parse result through.
fn parse_or_usage<T>(
    result: Result<T, ArgumentError>,
    client: &IrcClient,
    channel: &str,
    format: impl Fn(&str) -> String,
) -> Result<Option<T>, irc::error::Error> {
    match result {
        Ok(args) => Ok(Some(args)),
        Err(error) => {
            reply_usage_lines(client, channel, &error, format)?;

            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_wraps_the_message_in_the_plugin_banner() {
        assert_eq!(
            reply("Twitch", "hello"),
            "\x0310>\x0f\x02 Twitch:\x02\x0310 hello"
        );
    }

    #[test]
    fn plain_reply_prefixes_the_message_with_the_marker() {
        assert_eq!(plain_reply("No results found"), "\x0310> No results found");
    }
}
