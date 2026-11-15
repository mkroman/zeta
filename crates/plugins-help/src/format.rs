//! Formatting and catalog introspection for help replies.
//!
//! Renders the plugin index, a plugin's commands, and a command's description, usage synopsis,
//! and argument details, splitting long lists across continuation messages.
//!
//! The module is split by concern: `catalog` matches a query against the plugin catalog and
//! renders the matching reply, `details` formats a command's arguments and subcommands, `usage`
//! derives usage synopses and argument labels from the command's `clap` metadata, and `layout`
//! packs lists into the message budget.

mod catalog;
mod details;
mod layout;
#[cfg(test)]
mod tests;
mod usage;

use zeta_plugin::PluginCatalog;
use zeta_plugin::irc::{BOLD, COLOR, REPLY_MARKER, RESET};

use self::catalog::{command_messages, find_command, find_plugin, index_messages, plugin_messages};

/// The maximum length of a help message, assuming the IRCv3 extended line length of 8191 bytes
/// and leaving room for the sender prefix, `PRIVMSG` framing and line ending overhead.
const MAX_MESSAGE_LENGTH: usize = 7679;

/// The separator between entries in a help list: a comma in the scaffolding color and a space.
const SEPARATOR: &str = "\x0310,\x0f ";

/// Returns the messages describing the plugins or commands matching `query`.
///
/// An empty query shows a usage hint and an index of every plugin that handles commands, a
/// plugin name lists its commands, and a command name shows the command's description, usage and
/// arguments.
pub fn messages(catalog: &PluginCatalog, query: &str) -> Vec<String> {
    if query.is_empty() {
        return index_messages(catalog);
    }

    if let Some((_, command)) = find_command(catalog, query) {
        return command_messages(command);
    }

    if let Some(plugin) = find_plugin(catalog, query) {
        return plugin_messages(plugin);
    }

    vec![format!(
        "{}{}",
        heading(""),
        gray(&format!(" no plugin or command matches `{query}`"))
    )]
}

/// Formats the header of a help reply, with `subject` appended inside the bold heading.
pub fn heading(subject: &str) -> String {
    format!("{REPLY_MARKER}{RESET}{BOLD} Help{subject}{BOLD}")
}

/// Formats `s` in the muted scaffolding color.
pub fn gray(s: &str) -> String {
    format!("{COLOR}{s}{RESET}")
}
