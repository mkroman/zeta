//! IRC formatting shared by bundled plugins.
//!
//! Plugin replies follow a common visual convention: a teal `>` marker (mIRC color 10), an
//! optional bold plugin name, and the message body with values emphasized by switching between
//! the reply color and the default color. The helpers here produce those byte sequences so the
//! exact escape codes are written down in one place.

use std::fmt;
use std::future::Future;

use argh::FromArgs;
use irc::client::Client;
use tracing::warn;

use crate::command::ArgsError;
use crate::event::CommandEvent;

/// The teal color (mIRC color 10) used for plugin replies.
pub const COLOR: &str = "\x0310";

/// Resets bold, color, and any other text formatting back to the default.
pub const RESET: &str = "\x0f";

/// Starts bold text.
pub const BOLD: &str = "\x02";

/// The prefix shared by every reply format, bold name or not.
pub const REPLY_PREFIX: &str = "\x0310>";

/// Formats `message` as a reply from the plugin named `name`.
///
/// The reply starts with [`REPLY_PREFIX`], followed by the bold plugin name, e.g.
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
    format!("{REPLY_PREFIX}{RESET}{BOLD} {name}:{BOLD}{COLOR} ")
}

/// Formats `message` as a plain reply without a plugin name.
#[must_use]
pub fn notice(message: impl fmt::Display) -> String {
    format!("{REPLY_PREFIX} {message}")
}

/// Formats one `` Label:\x0f value\x0310 `` field: the label in the default color, `value` in
/// the reply color.
///
/// No leading space is included, so a message's first field is written as-is while continuation
/// fields prefix the space themselves, e.g. `write!(fmt, " {}", field("Plot", plot))`.
#[must_use]
pub fn field(label: &str, value: impl fmt::Display) -> String {
    format!("{label}:{} {value}{COLOR}", RESET)
}

/// Emphasizes `value` by switching from the surrounding color to the reply color.
#[must_use]
pub fn em(value: impl fmt::Display) -> String {
    format!("{RESET}{value}{COLOR}")
}

/// Formats `value` as `` “value” `` — curly quotes around the reply color.
#[must_use]
pub fn quoted(value: impl fmt::Display) -> String {
    format!("“{}”", em(value))
}

/// Runs a lookup command that yields a single result, replying to `channel`.
///
/// Empty `args` are answered with `usage`; otherwise `lookup` runs with the trimmed arguments and
/// a successful result is formatted by `format` (which produces the complete reply, banner
/// included). An error is answered with a plain notice and logged.
///
/// # Errors
///
/// Returns any error produced while sending the reply.
pub async fn reply_lookup<T, E, F, Fut>(
    client: &Client,
    channel: &str,
    args: &str,
    usage: &str,
    lookup: F,
    format: impl FnOnce(&T) -> String,
) -> Result<(), irc::error::Error>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: fmt::Display,
{
    let message = if args.trim().is_empty() {
        notice(usage)
    } else {
        match lookup(args.trim().to_string()).await {
            Ok(result) => format(&result),
            Err(error) => {
                warn!(%error, "lookup failed");
                notice(error)
            }
        }
    };

    client.send_privmsg(channel, message)
}

/// Runs a lookup command that yields a list of results, replying to `channel` with its first
/// entry.
///
/// Empty `args` are answered with `usage`; otherwise `lookup` runs with the trimmed arguments and
/// its first result is formatted by `format` (which produces the complete reply, banner
/// included). An empty list is answered with a no-results notice, an error with a plain notice
/// and a log line.
///
/// # Errors
///
/// Returns any error produced while sending the reply.
pub async fn reply_first_lookup<T, E, F, Fut>(
    client: &Client,
    channel: &str,
    args: &str,
    usage: &str,
    lookup: F,
    format: impl FnOnce(&T) -> String,
) -> Result<(), irc::error::Error>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<Vec<T>, E>>,
    E: fmt::Display,
{
    let message = if args.trim().is_empty() {
        notice(usage)
    } else {
        match lookup(args.trim().to_string()).await {
            Ok(results) => results.first().map_or_else(|| notice("No results"), format),
            Err(error) => {
                warn!(%error, "lookup failed");
                notice(error)
            }
        }
    };

    client.send_privmsg(channel, message)
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
    client: &Client,
    channel: &str,
    error: &ArgsError,
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
    client: &Client,
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
    client: &Client,
    command: &CommandEvent,
    format: impl Fn(&str) -> String,
) -> Result<Option<T>, irc::error::Error> {
    parse_or_usage(command.parse_words(), client, command.channel(), format)
}

/// Replies `error`'s usage lines through `format` and yields `None` for the caller to abandon
/// the command with, or passes a successful parse result through.
fn parse_or_usage<T>(
    result: Result<T, ArgsError>,
    client: &Client,
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
    fn notice_prefixes_the_message_with_the_marker() {
        assert_eq!(notice("No results found"), "\x0310> No results found");
    }

    #[test]
    fn field_emphasizes_the_value_in_the_reply_color() {
        assert_eq!(field("Plot", "steep"), "Plot:\x0f steep\x0310");
    }

    #[test]
    fn quoted_wraps_the_value_in_curly_quotes() {
        assert_eq!(quoted("Pilot"), "“\x0fPilot\x0310”");
    }

    #[test]
    fn em_switches_to_the_reply_color_around_the_value() {
        assert_eq!(em("2008"), "\x0f2008\x0310");
    }
}
