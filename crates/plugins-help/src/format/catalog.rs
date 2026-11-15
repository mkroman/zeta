//! Matching help queries against the plugin catalog and rendering their replies.

use zeta_plugin::{CatalogEntry, CommandSpec, PluginCatalog};

use super::details::command_details;
use super::layout::with_list;
use super::usage::usage;
use super::{gray, heading};

/// Returns the messages showing a usage hint and indexing the catalog's plugins.
pub(super) fn index_messages(catalog: &PluginCatalog) -> Vec<String> {
    let header = format!(
        "{}{} <plugin|command>{}",
        heading(""),
        gray(": Usage: .help"),
        gray(" Plugins:"),
    );

    let entries: Vec<&str> = catalog
        .entries
        .iter()
        .filter(|plugin| !plugin.commands.is_empty())
        .map(|plugin| plugin.name.as_str())
        .collect();

    with_list(&header, &entries)
}

/// Returns the messages listing the commands handled by `plugin`.
pub(super) fn plugin_messages(plugin: &CatalogEntry) -> Vec<String> {
    let header = format!(
        "{}{}",
        heading(&format!(" ({})", plugin.name)),
        gray(": Commands:"),
    );

    let entries: Vec<&str> = plugin.commands.iter().map(CommandSpec::trigger).collect();

    with_list(&header, &entries)
}

/// Returns the messages describing `command`.
///
/// The reply carries the command's description, its usage synopsis and, when it accepts
/// arguments or subcommands, their descriptions in a parenthesized list.
pub(super) fn command_messages(command: &CommandSpec) -> Vec<String> {
    let prefix = command.trigger();
    let mut message = heading(&format!(" ({prefix})"));

    let description = command.description();

    if !description.is_empty() {
        message.push_str(&gray(&format!(": {description}")));
    }

    let Some(info) = command.args_info() else {
        return vec![message];
    };

    message.push_str(&gray(" Usage:"));
    message.push(' ');
    message.push_str(&usage(prefix, &info));

    let details = command_details(&info);

    if !details.is_empty() {
        message.push_str(&gray(" ("));
        message.push_str(&details);
        message.push(')');
    }

    vec![message]
}

/// Strips the leading command sigil (`.` or `!`) from `name`.
fn normalize(name: &str) -> &str {
    name.trim_start_matches(['.', '!'])
}

/// Finds the plugin and command handling the given command name.
pub(super) fn find_command<'a>(
    catalog: &'a PluginCatalog,
    query: &str,
) -> Option<(&'a CatalogEntry, &'a CommandSpec)> {
    let query = normalize(query);

    catalog.entries.iter().find_map(|plugin| {
        plugin
            .commands
            .iter()
            .find(|command| normalize(command.trigger()) == query)
            .map(|command| (plugin, command))
    })
}

/// Finds the plugin with the given name.
pub(super) fn find_plugin<'a>(catalog: &'a PluginCatalog, query: &str) -> Option<&'a CatalogEntry> {
    let query = normalize(query);

    catalog.entries.iter().find(|plugin| plugin.name == query)
}
