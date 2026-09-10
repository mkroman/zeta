//! List the plugins the bot has loaded and the commands they handle.
//!
//! The `.help` command reads the plugin catalog published by the registry, so it always reflects
//! the plugins that were successfully registered. Each command's short description is shown in
//! the per-plugin view, along with the usage synopsis derived from its `argh` argument type.
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

/// The prefix every help reply starts with.
const PREFIX: &str = "\x0310>\x0f\x02 Help\x02\x0310:\x0f";

/// The maximum length of a help message body, leaving room for the reply prefix and the IRC line
/// overhead.
const MAX_MESSAGE_LENGTH: usize = 360;

/// Separator between entries in a help message.
const SEPARATOR: &str = "  ";

/// Help plugin.
///
/// Responds to `.help` with an index of every plugin and its commands, `.help <plugin>` with the
/// plugin's commands, descriptions and usage synopses, or `.help <command>` with the command's
/// usage and arguments derived from its `argh` type.
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
            client.send_privmsg(channel, format!("{PREFIX} the plugin catalog is unavailable"))?;

            return Ok(());
        };

        for message in messages(&catalog, args.trim()) {
            client.send_privmsg(channel, message)?;
        }

        Ok(())
    }
}

/// Returns the messages describing the plugins or commands matching `query`.
///
/// An empty query indexes every plugin, a plugin name lists its commands, and a command name
/// shows the command's usage and arguments.
fn messages(catalog: &PluginCatalog, query: &str) -> Vec<String> {
    let bodies = if query.is_empty() {
        pack_entries(
            catalog
                .plugins
                .iter()
                .filter(|plugin| !plugin.commands.is_empty())
                .map(plugin_summary),
        )
    } else if let Some((plugin, command)) = find_command(catalog, query) {
        pack_entries(command_details(plugin, command))
    } else if let Some(plugin) = find_plugin(catalog, query) {
        pack_entries(plugin_details(plugin))
    } else {
        vec![format!("no plugin or command matches `{query}`")]
    };

    bodies
        .into_iter()
        .map(|body| format!("{PREFIX} {body}"))
        .collect()
}

/// Formats a plugin and its commands as an index entry.
fn plugin_summary(plugin: &PluginInfo) -> String {
    format!("{}: {}", bold(&plugin.name), command_list(plugin))
}

/// Formats a plugin's heading and each of its commands as detail entries.
fn plugin_details(plugin: &PluginInfo) -> Vec<String> {
    let authors = author_list(plugin);

    let heading = if authors.is_empty() {
        bold(&plugin.name)
    } else {
        format!("{} by {authors}", bold(&plugin.name))
    };

    std::iter::once(heading)
        .chain(plugin.commands.iter().map(command_summary))
        .collect()
}

/// Formats a command, its description and its usage synopsis as a single entry.
fn command_summary(command: &PluginCommand) -> String {
    let prefix = command.prefix().as_str();
    let description = command.description();

    let summary = if description.is_empty() {
        bold(prefix)
    } else {
        format!("{} - {description}", bold(prefix))
    };

    let Some(info) = command.args_info() else {
        return summary;
    };

    let synopsis = usage(prefix, &info);

    if synopsis == prefix {
        summary
    } else {
        format!("{summary} {}", gray(&synopsis))
    }
}

/// Formats a command's usage, arguments, examples and description as detail entries.
fn command_details(plugin: &PluginInfo, command: &PluginCommand) -> Vec<String> {
    let prefix = command.prefix().as_str();
    let description = command.description();
    let attribution = gray(&format!("({})", plugin.name));

    let heading = if description.is_empty() {
        format!("{} {attribution}", bold(prefix))
    } else {
        format!("{} - {description} {attribution}", bold(prefix))
    };

    let Some(info) = command.args_info() else {
        return vec![heading];
    };

    let mut lines = vec![heading];

    lines.push(labeled("Usage", &usage(prefix, &info)));

    for positional in visible_positionals(&info) {
        lines.push(labeled(&positional_label(positional), positional.description));
    }

    for flag in visible_flags(&info) {
        lines.push(labeled(&flag_label(flag), flag.description));
    }

    for subcommand in &info.commands {
        let name = format!("{prefix} {}", subcommand.command.name);

        lines.push(labeled(
            &usage(&name, &subcommand.command),
            subcommand.command.description,
        ));
    }

    for example in info.examples {
        lines.push(labeled("Example", example));
    }

    lines.extend(info.notes.iter().map(ToString::to_string));

    lines
}

/// Packs `entries` into as few messages as possible without splitting an entry.
fn pack_entries(entries: impl IntoIterator<Item = String>) -> Vec<String> {
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

/// Returns the plugin's command prefixes as a space-separated list.
fn command_list(plugin: &PluginInfo) -> String {
    plugin
        .commands
        .iter()
        .map(|command| command.prefix().as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Returns the plugin's authors as a comma-separated list.
fn author_list(plugin: &PluginInfo) -> String {
    plugin
        .authors
        .iter()
        .map(Author::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Builds the usage synopsis of a command from its argument information.
///
/// The synopsis follows `argh`'s conventions: optional flags and arguments are bracketed,
/// repeating ones are suffixed with `...`, and greedy positionals are unbracketed names.
fn usage(prefix: &str, info: &CommandInfoWithArgs) -> String {
    let mut usage = prefix.to_string();

    if !info.commands.is_empty() {
        usage.push_str(" <command> [<args>]");

        return usage;
    }

    for flag in visible_flags(info) {
        usage.push(' ');
        usage.push_str(&flag_usage(flag));
    }

    for positional in visible_positionals(info) {
        usage.push(' ');
        usage.push_str(&positional_label(positional));
    }

    usage
}

/// Returns the flags that should appear in help output.
fn visible_flags(info: &CommandInfoWithArgs) -> impl Iterator<Item = &FlagInfo<'_>> {
    info.flags
        .iter()
        .filter(|flag| !flag.hidden && flag.long != "--help")
}

/// Returns the positional arguments that should appear in help output.
fn visible_positionals(info: &CommandInfoWithArgs) -> impl Iterator<Item = &PositionalInfo<'_>> {
    info.positionals
        .iter()
        .filter(|positional| !positional.hidden)
}

/// Formats a flag for the usage synopsis, using its short form when it has one.
fn flag_usage(flag: &FlagInfo<'_>) -> String {
    let mut label = flag
        .short
        .map_or_else(|| flag.long.to_string(), |short| format!("-{short}"));

    if let FlagInfoKind::Option { arg_name } = flag.kind {
        label.push_str(" <");
        label.push_str(arg_name);

        if flag.optionality == Optionality::Repeating {
            label.push_str("...");
        }

        label.push('>');
    }

    if flag.optionality == Optionality::Required {
        label
    } else {
        format!("[{label}]")
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

/// Formats a positional argument, marking its optionality and repetition.
fn positional_label(positional: &PositionalInfo<'_>) -> String {
    let repeating = matches!(
        positional.optionality,
        Optionality::Repeating | Optionality::Greedy
    );

    let mut label = String::new();

    if positional.optionality == Optionality::Greedy {
        label.push_str(positional.name);

        if repeating {
            label.push_str("...");
        }
    } else {
        label.push('<');
        label.push_str(positional.name);

        if repeating {
            label.push_str("...");
        }

        label.push('>');
    }

    if positional.optionality == Optionality::Required {
        label
    } else {
        format!("[{label}]")
    }
}

/// Formats a bold `label`, appending `description` when it is not empty.
fn labeled(label: &str, description: &str) -> String {
    let description = description.trim();

    if description.is_empty() {
        bold(label)
    } else {
        format!("{}: {description}", bold(label))
    }
}

/// Formats `s` in bold.
fn bold(s: &str) -> String {
    format!("\x02{s}\x02")
}

/// Formats `s` in the muted scaffolding color.
fn gray(s: &str) -> String {
    format!("\x0310{s}\x0f")
}

/// Strips the leading command sigil (`.` or `!`) from `name`.
fn normalize(name: &str) -> &str {
    name.trim_start_matches(['.', '!'])
}

/// Finds the plugin and command handling the given command name.
fn find_command<'a>(
    catalog: &'a PluginCatalog,
    query: &str,
) -> Option<(&'a PluginInfo, &'a PluginCommand)> {
    let query = normalize(query);

    catalog.plugins.iter().find_map(|plugin| {
        plugin
            .commands
            .iter()
            .find(|command| normalize(command.prefix().as_str()) == query)
            .map(|command| (plugin, command))
    })
}

/// Finds the plugin with the given name.
fn find_plugin<'a>(catalog: &'a PluginCatalog, query: &str) -> Option<&'a PluginInfo> {
    let query = normalize(query);

    catalog.plugins.iter().find(|plugin| plugin.name == query)
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

    const IMDB: PluginCommand =
        PluginCommand::new(Prefix::new("!imdb"), "Search IMDb and post the top match");
    const IMDB_COMMANDS: &[PluginCommand] = &[IMDB];

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

    /// Repeat values.
    #[derive(argh::ArgsInfo)]
    #[allow(dead_code)]
    #[argh(example = ".repeat --values one two")]
    struct RepeatOpts {
        /// values to repeat
        #[argh(option)]
        values: Vec<String>,
        /// files to process
        #[argh(positional)]
        files: Vec<String>,
        /// not shown in help
        #[argh(switch, hidden_help)]
        hidden: bool,
    }

    const REPEAT: PluginCommand = PluginCommand::with_args::<RepeatOpts>(
        Prefix::new(".repeat"),
        "Repeat the given values",
    );
    const REPEAT_COMMANDS: &[PluginCommand] = &[REPEAT];

    /// Subcommand fixture.
    #[derive(argh::ArgsInfo)]
    #[allow(dead_code)]
    struct SubcommandOpts {
        #[argh(subcommand)]
        command: Subcommands,
    }

    #[derive(argh::ArgsInfo)]
    #[allow(dead_code)]
    #[argh(subcommand)]
    enum Subcommands {
        Stats(Stats),
    }

    /// Display statistics.
    #[derive(argh::ArgsInfo)]
    #[allow(dead_code)]
    #[argh(subcommand, name = "stats")]
    struct Stats {}

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
                PluginInfo {
                    name: "imdb".into(),
                    authors: vec![],
                    commands: IMDB_COMMANDS,
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
        assert!(find_command(&catalog, "imdb").is_some());
        assert!(find_command(&catalog, "!imdb").is_some());
        assert!(find_command(&catalog, "missing").is_none());
    }

    #[test]
    fn finds_plugins_by_name() {
        let catalog = catalog();

        assert_eq!(
            find_plugin(&catalog, "dig").map(|plugin| plugin.name.as_str()),
            Some("dig")
        );
        assert_eq!(
            find_plugin(&catalog, "!imdb").map(|plugin| plugin.name.as_str()),
            Some("imdb")
        );
        assert!(find_plugin(&catalog, "missing").is_none());
    }

    #[test]
    fn derives_usage_from_arguments() {
        let info = DIG_COMMANDS[0].args_info().unwrap();

        assert_eq!(usage(".dig", &info), ".dig [-s] <name>");
    }

    #[test]
    fn derives_usage_for_repeating_arguments() {
        let info = REPEAT_COMMANDS[0].args_info().unwrap();

        assert_eq!(
            usage(".repeat", &info),
            ".repeat [--values <values...>] [<files...>]"
        );
    }

    #[test]
    fn derives_usage_for_subcommands() {
        let command = PluginCommand::with_args::<SubcommandOpts>(
            Prefix::new(".stats"),
            "Show statistics",
        );
        let info = command.args_info().unwrap();

        assert_eq!(usage(".stats", &info), ".stats <command> [<args>]");
    }

    #[test]
    fn describes_commands_from_descriptions() {
        let catalog = catalog();
        let (plugin, command) = find_command(&catalog, "dig").unwrap();

        let lines = command_details(plugin, command);

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
    fn renders_examples_and_skips_hidden_arguments() {
        let plugin = PluginInfo {
            name: "repeat".into(),
            authors: vec![],
            commands: REPEAT_COMMANDS,
        };

        let lines = command_details(&plugin, &REPEAT);

        assert_eq!(
            lines[0],
            "\x02.repeat\x02 - Repeat the given values \x0310(repeat)\x0f"
        );
        assert_eq!(
            lines[1],
            "\x02Usage\x02: .repeat [--values <values...>] [<files...>]"
        );
        assert!(lines.iter().any(|line| line == "\x02Example\x02: .repeat --values one two"));
        assert!(!lines.iter().any(|line| line.contains("--hidden")));
    }

    #[test]
    fn formats_index_entries() {
        let catalog = catalog();

        assert_eq!(plugin_summary(&catalog.plugins[0]), "\x02alert\x02: .alert");
    }

    #[test]
    fn formats_plugin_details_with_usage() {
        let catalog = catalog();
        let lines = plugin_details(&catalog.plugins[1]);

        assert_eq!(lines[0], "\x02dig\x02");
        assert_eq!(
            lines[1],
            "\x02.dig\x02 - Look up DNS records for a domain \x0310.dig [-s] <name>\x0f"
        );
    }

    #[test]
    fn prefixes_and_packs_messages() {
        let catalog = catalog();
        let rendered = messages(&catalog, "dig");

        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].starts_with(PREFIX));
        assert!(rendered[0].contains(".dig"));

        let unmatched = messages(&catalog, "missing");

        assert_eq!(unmatched.len(), 1);
        assert!(unmatched[0].contains("no plugin or command matches"));
    }

    #[test]
    fn packs_entries_without_splitting_them() {
        let entries = (0..100).map(|index| format!("\x02entry-{index:02}\x02"));

        let messages = pack_entries(entries);

        assert!(messages.iter().all(|message| message.len() <= MAX_MESSAGE_LENGTH));
        assert!(messages.len() > 1);
        assert!(messages.iter().all(|message| message.contains("entry-")));
    }
}
