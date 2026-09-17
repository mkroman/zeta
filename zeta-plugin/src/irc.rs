//! IRC formatting shared by bundled plugins.
//!
//! Plugin replies follow a common visual convention: a teal `>` marker (mIRC color 10), an
//! optional bold plugin name, and the message body with values emphasized by switching between
//! the reply color and the default color. The helpers here produce those byte sequences so the
//! exact escape codes are written down in one place.

use std::fmt;

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
}
