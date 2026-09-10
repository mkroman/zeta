//! List the plugins the bot has loaded and the commands they handle.
//!
//! The `.help` command reads the plugin catalog published by the registry, so it always reflects
//! the plugins that were successfully registered. Each command's short description is shown in
//! the per-plugin view, and commands that associate an `argh` argument type also expose usage and
//! argument details derived from it.
//!
//! Entries are packed into as few messages as possible, so `.help` stays a short index while
//! `.help <command>` provides the details.

use argh::{CommandInfoWithArgs, FlagInfo, FlagInfoKind, Optionality, PositionalInfo};

use crate::plugin::prelude::*;

/// The `.help` command.
const HELP: PluginCommand = PluginCommand::new(
    Prefix::new(".help"),
    "List plugins and commands, or show usage for one",
);

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[HELP];

/// The maximum length of a help message body, leaving room for the IRC line overhead.
const MAX_MESSAGE_LENGTH: usize = 360;

/// Separator between entries in a help message.
const SEPARATOR: &str = "  ";

/// Help plugin.
///
/// Responds to `.help` with an index of every plugin and its commands, `.help <plugin>` with the
/// plugin's commands and their descriptions, or `.help <command>` with the usage and arguments
/// derived from the command's `argh` type.
pub struct Help;

#[async_trait]
impl Plugin<Context> for Help {
    fn new(_ctx: &Context) -> Result<Self, ZetaError> {
        Ok(Help)
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "help".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn commands(&self) -> &'static [PluginCommand] {
        COMMANDS
    }

    async fn handle_command(
        &self,
        ctx: &Context,
        client: &Client,
        channel: &str,
        _command: &Prefix,
        args: &str,
    ) -> Result<(), ZetaError> {
        let Some(catalog) = ctx.shared.get::<PluginCatalog>() else {
            client.send_privmsg(channel, formatted("the plugin catalog is unavailable"))?;

            return Ok(());
        };

        let query = args.trim();

        let messages = if query.is_empty() {
            pack(
                catalog
                    .plugins
                    .iter()
                    .filter(|plugin| !plugin.commands.is_empty())
                    .map(plugin_entry),
            )
        } else if let Some((plugin, command)) = find_command(&catalog, query) {
            pack(command_lines(plugin, command))
        } else if let Some(plugin) = find_plugin(&catalog, query) {
            pack(plugin_entries(plugin))
        } else {
            vec![format!("no plugin or command matches `{query}`")]
        };

        for message in messages {
            client.send_privmsg(channel, formatted(&message))?;
        }

        Ok(())
    }
}

/// Formats a plugin and its commands as a single help entry.
fn plugin_entry(plugin: &PluginInfo) -> String {
    format!("\x02{}\x02: {}", plugin.name, commands(plugin))
}

/// Formats a plugin's heading and each of its commands as help entries.
fn plugin_entries(plugin: &PluginInfo) -> Vec<String> {
    let authors = authors(plugin);

    let heading = if authors.is_empty() {
        format!("\x02{}\x02", plugin.name)
    } else {
        format!("\x02{}\x02 by {authors}", plugin.name)
    };

    std::iter::once(heading)
        .chain(plugin.commands.iter().map(command_entry))
        .collect()
}

/// Formats a command and its description as a help entry.
fn command_entry(command: &PluginCommand) -> String {
    let prefix = command.prefix().as_str();
    let description = command.description();

    if description.is_empty() {
        format!("\x02{prefix}\x02")
    } else {
        format!("\x02{prefix}\x02 - {description}")
    }
}

/// Returns the lines describing a command, derived from its argument information.
fn command_lines(plugin: &PluginInfo, command: &PluginCommand) -> Vec<String> {
    let prefix = command.prefix().as_str();
    let name = format!("\x02{prefix}\x02");
    let attribution = format!("\x0310({})\x0f", plugin.name);
    let description = command.description();

    let heading = if description.is_empty() {
        format!("{name} {attribution}")
    } else {
        format!("{name} - {description} {attribution}")
    };

    let Some(info) = command.args_info() else {
        return vec![heading];
    };

    let mut lines = vec![heading];

    lines.push(format!("\x02Usage\x02: {}", usage(prefix, &info)));

    for positional in info
        .positionals
        .iter()
        .filter(|positional| !positional.hidden)
    {
        lines.push(entry(
            &positional_label(positional),
            positional.description,
        ));
    }

    for flag in info
        .flags
        .iter()
        .filter(|flag| !flag.hidden && flag.long != "--help")
    {
        lines.push(entry(&flag_label(flag), flag.description));
    }

    for subcommand in &info.commands {
        let name = format!("{prefix} {}", subcommand.command.name);

        lines.push(entry(&name, subcommand.command.description));
    }

    lines.extend(info.notes.iter().map(ToString::to_string));

    lines
}

/// Formats a bold `label`, appending `description` when it is not empty.
fn entry(label: &str, description: &str) -> String {
    let description = description.trim();

    if description.is_empty() {
        format!("\x02{label}\x02")
    } else {
        format!("\x02{label}\x02: {description}")
    }
}

/// Packs `entries` into as few messages as possible without splitting an entry.
fn pack(entries: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut messages = Vec::new();
    let mut message = String::new();

    for entry in entries {
        if !message.is_empty() && message.len() + SEPARATOR.len() + entry.len() > MAX_MESSAGE_LENGTH
        {
            messages.push(std::mem::take(&mut message));
        }

        if !message.is_empty() {
            message.push_str(SEPARATOR);
        }

        message.push_str(&entry);
    }

    if !message.is_empty() {
        messages.push(message);
    }

    messages
}

/// Returns the plugin's commands as a space-separated list.
fn commands(plugin: &PluginInfo) -> String {
    plugin
        .commands
        .iter()
        .map(|command| command.prefix().as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Returns the plugin's authors as a comma-separated list.
fn authors(plugin: &PluginInfo) -> String {
    plugin
        .authors
        .iter()
        .map(Author::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Builds the usage synopsis of a command from its argument information.
fn usage(prefix: &str, info: &CommandInfoWithArgs) -> String {
    let mut usage = prefix.to_string();

    if !info.commands.is_empty() {
        usage.push_str(" <command>");

        return usage;
    }

    for flag in info
        .flags
        .iter()
        .filter(|flag| !flag.hidden && flag.long != "--help")
    {
        usage.push(' ');
        usage.push_str(&flag_usage(flag));
    }

    for positional in info
        .positionals
        .iter()
        .filter(|positional| !positional.hidden)
    {
        usage.push(' ');
        usage.push_str(&positional_label(positional));
    }

    usage
}

/// Formats a flag for the usage synopsis, using its short form when it has one.
fn flag_usage(flag: &FlagInfo<'_>) -> String {
    let mut label = flag
        .short
        .map_or_else(|| flag.long.to_string(), |short| format!("-{short}"));

    if let FlagInfoKind::Option { arg_name } = flag.kind {
        label.push_str(" <");
        label.push_str(arg_name);
        label.push('>');
    }

    if flag.optionality == Optionality::Optional {
        format!("[{label}]")
    } else {
        label
    }
}

/// Formats a flag for the argument list, showing both its short and long forms.
fn flag_label(flag: &FlagInfo<'_>) -> String {
    let mut label = String::new();

    if let Some(short) = flag.short {
        label.push('-');
        label.push(short);
        label.push_str(", ");
    }

    label.push_str(flag.long);

    if let FlagInfoKind::Option { arg_name } = flag.kind {
        label.push_str(" <");
        label.push_str(arg_name);
        label.push('>');
    }

    label
}

/// Formats a positional argument, marking its optionality.
fn positional_label(positional: &PositionalInfo<'_>) -> String {
    match positional.optionality {
        Optionality::Required => format!("<{}>", positional.name),
        Optionality::Optional => format!("[<{}>]", positional.name),
        Optionality::Repeating | Optionality::Greedy => format!("<{}>...", positional.name),
    }
}

/// Finds the plugin and command handling the given command name.
fn find_command<'a>(
    catalog: &'a PluginCatalog,
    query: &str,
) -> Option<(&'a PluginInfo, &'a PluginCommand)> {
    let query = query.trim_start_matches('.');

    catalog.plugins.iter().find_map(|plugin| {
        plugin
            .commands
            .iter()
            .find(|command| command.prefix().as_str().trim_start_matches('.') == query)
            .map(|command| (plugin, command))
    })
}

/// Finds the plugin with the given name.
fn find_plugin<'a>(catalog: &'a PluginCatalog, query: &str) -> Option<&'a PluginInfo> {
    let query = query.trim_start_matches('.');

    catalog.plugins.iter().find(|plugin| plugin.name == query)
}

/// Formats `s` as a help response.
fn formatted(s: &str) -> String {
    format!("\x0310>\x0f\x02 Help\x02\x0310:\x0f {s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALERT: PluginCommand = PluginCommand::new(
        Prefix::new(".alert"),
        "Schedule an alert to be posted later",
    );
    const ALERT_COMMANDS: &[PluginCommand] = &[ALERT];

    const DIG: PluginCommand = PluginCommand::with_args::<DigOpts>(
        Prefix::new(".dig"),
        "Look up DNS records for a domain",
    );
    const DIG_COMMANDS: &[PluginCommand] = &[DIG];

    /// Look up a domain name.
    #[derive(argh::ArgsInfo)]
    #[allow(dead_code)]
    struct DigOpts {
        /// the domain to look up
        #[argh(positional)]
        name: String,
        /// only display the answer section
        #[argh(switch, short = 's')]
        short: bool,
    }

    fn catalog() -> PluginCatalog {
        PluginCatalog {
            plugins: vec![
                PluginInfo {
                    name: "alert".into(),
                    authors: vec!["John Doe <john.doe@example.com>".into()],
                    commands: ALERT_COMMANDS,
                },
                PluginInfo {
                    name: "dig".into(),
                    authors: vec![],
                    commands: DIG_COMMANDS,
                },
            ],
        }
    }

    #[test]
    fn finds_commands_by_name() {
        let catalog = catalog();

        assert!(find_command(&catalog, "alert").is_some());
        assert!(find_command(&catalog, ".alert").is_some());
        assert!(find_command(&catalog, ".dig").is_some());
        assert!(find_command(&catalog, "missing").is_none());
    }

    #[test]
    fn finds_plugins_by_name() {
        let catalog = catalog();

        assert_eq!(
            find_plugin(&catalog, "dig").map(|plugin| plugin.name.as_str()),
            Some("dig")
        );
        assert!(find_plugin(&catalog, "missing").is_none());
    }

    #[test]
    fn derives_usage_from_arguments() {
        let info = DIG_COMMANDS[0].args_info().unwrap();

        assert_eq!(usage(".dig", &info), ".dig [-s] <name>");
    }

    #[test]
    fn describes_commands_from_descriptions() {
        let catalog = catalog();
        let (plugin, command) = find_command(&catalog, "dig").unwrap();

        let lines = command_lines(plugin, command);

        assert_eq!(
            lines[0],
            "\x02.dig\x02 - Look up DNS records for a domain \x0310(dig)\x0f"
        );
        assert_eq!(lines[1], "\x02Usage\x02: .dig [-s] <name>");
        assert_eq!(lines[2], "\x02<name>\x02: the domain to look up");
        assert_eq!(
            lines[3],
            "\x02-s, --short\x02: only display the answer section"
        );
    }

    #[test]
    fn formats_styled_plugin_entries() {
        let catalog = catalog();

        assert_eq!(plugin_entry(&catalog.plugins[0]), "\x02alert\x02: .alert");
        assert_eq!(
            plugin_entries(&catalog.plugins[1]),
            vec![
                "\x02dig\x02".to_string(),
                "\x02.dig\x02 - Look up DNS records for a domain".to_string(),
            ]
        );
    }

    #[test]
    fn packs_entries_without_splitting_them() {
        let entries = (0..100).map(|index| format!("\x02entry-{index:02}\x02"));

        let messages = pack(entries);

        assert!(messages.iter().all(|message| message.len() <= MAX_MESSAGE_LENGTH));
        assert!(messages.len() > 1);
        assert!(messages.iter().all(|message| message.contains("entry-")));
    }
}
