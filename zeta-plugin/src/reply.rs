//! Reply formatting shared by bundled plugins.
//!
//! The reply conventions live in the [`zeta-fmt`] crate: a plugin's
//! [`Banner`] is defined once and produces the teal `>` marker, the optional
//! bold plugin name, and the scaffolding color; the [`Reply`] builder tracks
//! the formatting state so control codes are emitted only on transitions,
//! and interpolated values are stripped of control characters.
//!
//! The helpers here are the byte-compatible bridge to the old free-function
//! API (`reply`, `reply_prefix`, `notice`) plus the usage-line senders used
//! by command handlers.
//!
//! [`zeta-fmt`]: zeta_fmt

use std::fmt;

use argh::FromArgs;
use irc::client::Client;

use crate::command::ArgsError;
use crate::event::CommandEvent;
use zeta_fmt::Banner;

pub use zeta_fmt::{BOLD, COLOR, REPLY_PREFIX, RESET, plain};

/// Formats `message` as a reply from the plugin named `name`.
///
/// The reply starts with the reply marker, followed by the bold plugin name,
/// e.g. `> Twitch: <message>`.
#[must_use]
pub fn reply(name: &str, message: impl fmt::Display) -> String {
    Banner::named(name.to_owned()).message(message)
}

/// Returns the bold-name prefix that [`reply`] starts its replies with.
///
/// Unlike [`reply`], the prefix can be written to an incrementally built
/// message, e.g. from a `Display` implementation.
#[must_use]
pub fn reply_prefix(name: &str) -> String {
    Banner::named(name.to_owned()).prefix()
}

/// Formats `message` as a plain reply without a plugin name.
#[must_use]
pub fn notice(message: impl fmt::Display) -> String {
    Banner::BARE.message(message)
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
    fn reply_prefix_matches_the_banner_prefix() {
        assert_eq!(reply_prefix("Dig"), "\x0310>\x0f\x02 Dig:\x02\x0310 ");
    }
}
