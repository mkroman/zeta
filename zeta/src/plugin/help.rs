//! List the plugins the bot has loaded and the commands they handle.
//!
//! The `.help` command reads the plugin catalog published by the registry, so it always reflects
//! the plugins that were successfully registered. Commands that associate an `argh` argument type
//! also expose usage and option descriptions derived from it.

use argh::{CommandInfoWithArgs, FlagInfo, FlagInfoKind, Optionality, PositionalInfo};

use crate::plugin::prelude::*;

/// The `.help` command.
const HELP: Prefix = Prefix::new(".help");

/// Help plugin.
///
/// Responds to `.help` with one line per plugin and its commands, `.help <plugin>` with the
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
        const { &[PluginCommand::new(HELP)] }
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

        if query.is_empty() {
            for plugin in catalog
                .plugins
                .iter()
                .filter(|plugin| !plugin.commands.is_empty())
            {
                client.send_privmsg(channel, formatted(&plugin_line(plugin)))?;
            }
        } else if let Some((plugin, command)) = find_command(&catalog, query) {
            for line in command_lines(plugin, command) {
                client.send_privmsg(channel, formatted(&line))?;
            }
        } else if let Some(plugin) = find_plugin(&catalog, query) {
            for line in plugin_lines(plugin) {
                client.send_privmsg(channel, formatted(&line))?;
            }
        } else {
            client.send_privmsg(
                channel,
                formatted(&format!("no plugin or command matches `{query}`")),
            )?;
        }

        Ok(())
    }
}

/// Formats a plugin and its commands.
fn plugin_line(plugin: &PluginInfo) -> String {
    format!("{}: {}", plugin.name, commands(plugin))
}

/// Returns the lines describing a plugin and each of its commands.
fn plugin_lines(plugin: &PluginInfo) -> Vec<String> {
    let authors = authors(plugin);

    let heading = if authors.is_empty() {
        plugin.name.clone()
    } else {
        format!("{} (by {authors})", plugin.name)
    };

    std::iter::once(heading)
        .chain(plugin.commands.iter().map(command_line))
        .collect()
}

/// Returns the lines describing a command, derived from its argument information.
fn command_lines(plugin: &PluginInfo, command: &PluginCommand) -> Vec<String> {
    let prefix = command.prefix().as_str();
    let Some(info) = command.args_info() else {
        return vec![format!("{prefix} ({})", plugin.name)];
    };

    let mut lines = vec![format!("{} ({})", command_line(command), plugin.name)];

    lines.push(format!("Usage: {}", usage(prefix, &info)));

    for positional in info
        .positionals
        .iter()
        .filter(|positional| !positional.hidden)
    {
        lines.push(described(
            &positional_label(positional),
            positional.description,
        ));
    }

    for flag in info
        .flags
        .iter()
        .filter(|flag| !flag.hidden && flag.long != "--help")
    {
        lines.push(described(&flag_label(flag), flag.description));
    }

    for subcommand in &info.commands {
        let name = subcommand.command.name;
        let description = subcommand.command.description.trim();

        lines.push(if description.is_empty() {
            format!("{prefix} {name}")
        } else {
            format!("{prefix} {name}: {description}")
        });
    }

    lines.extend(info.notes.iter().map(ToString::to_string));

    lines
}

/// Formats a command and, when known, its description.
fn command_line(command: &PluginCommand) -> String {
    let prefix = command.prefix().as_str();

    match command.args_info() {
        Some(info) if !info.description.is_empty() => format!("{prefix} - {}", info.description),
        _ => prefix.to_string(),
    }
}

/// Returns the plugin's commands as a comma-separated list.
fn commands(plugin: &PluginInfo) -> String {
    plugin
        .commands
        .iter()
        .map(|command| command.prefix().as_str())
        .collect::<Vec<_>>()
        .join(", ")
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

/// Formats `label`, appending `description` when it is not empty.
fn described(label: &str, description: &str) -> String {
    let description = description.trim();

    if description.is_empty() {
        label.to_string()
    } else {
        format!("{label}: {description}")
    }
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
    format!("\x0310>\x0f\x02 Help\x02\x0310: {s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALERT: Prefix = Prefix::new(".alert");
    const ALERT_COMMANDS: &[PluginCommand] = &[PluginCommand::new(ALERT)];

    const DIG: Prefix = Prefix::new(".dig");
    const DIG_COMMANDS: &[PluginCommand] = &[PluginCommand::with_args::<DigOpts>(DIG)];

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
    fn describes_commands_from_arguments() {
        let catalog = catalog();
        let (plugin, command) = find_command(&catalog, "dig").unwrap();

        let lines = command_lines(plugin, command);

        assert_eq!(lines[0], ".dig - Look up a domain name. (dig)");
        assert_eq!(lines[1], "Usage: .dig [-s] <name>");
        assert_eq!(lines[2], "<name>: the domain to look up");
        assert_eq!(lines[3], "-s, --short: only display the answer section");
    }

    #[test]
    fn lists_plugins_one_line_each() {
        let catalog = catalog();

        let alert = plugin_lines(&catalog.plugins[0]);
        let dig = plugin_lines(&catalog.plugins[1]);

        assert_eq!(alert[0], "alert (by John Doe <john.doe@example.com>)");
        assert_eq!(alert[1], ".alert");
        assert_eq!(dig, vec!["dig", ".dig - Look up a domain name."]);
    }
}
