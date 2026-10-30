//! IRC reply helpers shared by bundled plugins.
//!
//! Plugin replies follow a common visual convention: a teal `>` marker (mIRC color 10), an
//! optional bold plugin name, and the message body with values emphasized by switching between
//! the reply color and the default color. The formatting helpers that produce those byte
//! sequences live in [`zeta_utils::irc`], next to the other IRC formatting helpers, and are
//! re-exported here so plugin code can keep importing them from this module. What remains here
//! are the helpers that need the plugin vocabulary: replying with a failed argument parse's
//! usage lines.

use std::future::Future;

use clap::Parser;
use irc::client::Client;

use crate::command::{ArgsError, ArgsMode};
use crate::event::CommandEvent;

pub use zeta_utils::irc::{
    BOLD, COLOR, PLAIN_REPLY_PREFIX, REPLY_MARKER, RESET, labeled_reply, labeled_reply_prefix,
    plain_reply, unstyle_text,
};

/// Sends the output of a failed argument parse to `channel`, one message per non-empty line.
///
/// Parse failures produce the command's usage or help output, which spans multiple lines; IRC
/// messages cannot contain line breaks, so each line is sent as its own `PRIVMSG`, formatted by
/// `format` (e.g. by wrapping it in [`labeled_reply`] with the plugin's label).
///
/// # Errors
///
/// Returns any error produced while sending the messages.
fn reply_usage_lines(
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

/// Parses the trailing arguments of `command` with the tokenization selected by `mode`,
/// replying with the usage lines to the command's channel when the parse fails.
///
/// Returns `Ok(None)` when the usage was replied to and the command should be abandoned.
///
/// # Errors
///
/// Returns any error produced while sending the usage lines.
pub fn parse_or_usage<T: Parser + 'static>(
    client: &Client,
    command: &CommandEvent,
    mode: ArgsMode,
    format: impl Fn(&str) -> String,
) -> Result<Option<T>, irc::error::Error> {
    reply_parse_result(command.parse_with(mode), client, command.channel(), format)
}

/// Awaits a lookup and replies with its outcome to `channel`.
///
/// `render` formats a successful value and `on_error` a failed lookup, so a handler that only
/// differs in how it presents the two does not repeat the `match` and `send_privmsg` scaffold.
///
/// # Errors
///
/// Returns any error produced while sending the reply.
pub async fn reply_lookup<T, E>(
    client: &Client,
    channel: &str,
    lookup: impl Future<Output = Result<T, E>>,
    render: impl FnOnce(T) -> String,
    on_error: impl FnOnce(E) -> String,
) -> Result<(), irc::error::Error> {
    let message = match lookup.await {
        Ok(value) => render(value),
        Err(error) => on_error(error),
    };

    client.send_privmsg(channel, message)
}

/// Replies `error`'s usage lines through `format` and yields `None` for the caller to abandon
/// the command with, or passes a successful parse result through.
fn reply_parse_result<T>(
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
