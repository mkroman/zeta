//! Formatting a command's arguments and subcommands as a labeled detail list.

use std::fmt::Write as _;

use clap::{Arg, Command};

use zeta_plugin::irc::{BOLD, COLOR, RESET};

use super::usage::{
    flag_label, positional_label, visible_flags, visible_positionals, visible_subcommands,
};

/// A labeled detail of a command: a bold label with a gray description.
pub(super) struct Detail {
    /// The label of the detail (e.g. the usage form of an argument).
    pub(super) label: String,
    /// The description of the detail.
    pub(super) description: String,
}

/// Formats the visible arguments and subcommands of a command as a styled detail list.
pub(super) fn command_details(info: &Command) -> String {
    let mut details = Vec::new();

    for positional in visible_positionals(info) {
        details.push(Detail {
            label: positional_label(positional),
            description: help_text(positional),
        });
    }

    for flag in visible_flags(info) {
        details.push(Detail {
            label: flag_label(flag),
            description: help_text(flag),
        });
    }

    for subcommand in visible_subcommands(info) {
        details.push(Detail {
            label: subcommand.get_name_and_visible_aliases().join(", "),
            description: subcommand
                .get_about()
                .map_or_else(String::new, ToString::to_string),
        });
    }

    details_list(&details)
}

/// Returns an argument's help text, or the empty string when it has none.
fn help_text(argument: &Arg) -> String {
    argument
        .get_help()
        .map_or_else(String::new, ToString::to_string)
}

/// Formats `details` as a comma-separated list.
///
/// Each label is bold and each description gray; all but the last detail are comma-terminated
/// (with the comma inside the color when the detail has a description), and details after the
/// first carry a leading space inside their label.
pub(super) fn details_list(details: &[Detail]) -> String {
    // The list is one string: every piece is a fixed escape sequence plus the detail's own
    // text, so the size is known before the first write.
    let mut list = String::with_capacity(
        details
            .iter()
            .map(|detail| detail.label.len() + detail.description.len() + 24)
            .sum(),
    );

    for (index, detail) in details.iter().enumerate() {
        let comma = if index + 1 == details.len() { "" } else { "," };

        list.push_str(BOLD);

        if index > 0 {
            list.push(' ');
        }

        list.push_str(&detail.label);
        list.push_str(BOLD);

        if detail.description.is_empty() {
            list.push_str(comma);
        } else {
            list.push_str(COLOR);
            // Writing to a `String` cannot fail.
            let _ = write!(list, ": {}{comma}", detail.description);
            list.push_str(RESET);
        }
    }

    list
}
