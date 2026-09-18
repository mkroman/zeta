//! Describe the plugins the bot has loaded and the commands they handle.
//!
//! The `.help` command reads the plugin catalog published by the registry, so it always reflects
//! the plugins that were successfully registered. Invoked without arguments it replies with a
//! usage hint and an index of the registered plugins, with a plugin name it lists the plugin's
//! commands, and with a command name it shows the command's description and usage synopsis
//! derived from its `argh` argument type, along with descriptions of its arguments and
//! subcommands.
//!
//! Lists longer than the IRCv3 extended message length are split across continuation messages
//! without splitting an entry.

use argh::{CommandInfoWithArgs, FlagInfo, FlagInfoKind, Optionality, PositionalInfo};

use crate::plugin::prelude::*;

/// The `.help` command.
const HELP: CommandSpec = CommandSpec::new(
    ".help",
    "List plugins and commands, or show usage for one",
);

/// The maximum length of a help message, assuming the IRCv3 extended line length of 8191 bytes
/// and leaving room for the sender prefix, `PRIVMSG` framing and line ending overhead.
const MAX_MESSAGE_LENGTH: usize = 7679;

/// The separator between entries in a help list: a comma in the scaffolding color and a space.
const SEPARATOR: &str = "\x0310,\x0f ";

/// Help plugin.
///
/// Responds to `.help` with a usage hint and an index of every plugin, `.help <plugin>` with the
/// plugin's commands, or `.help <command>` with the command's description, usage and arguments
/// derived from its `argh` type.
pub struct Help;

#[async_trait]
impl Plugin<Context> for Help {
    type Settings = NoSettings;

    fn new(_ctx: &Context, _settings: &NoSettings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.command(HELP);
        Ok(Help)
    }

    async fn handle_command(
        &self,
        ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let args = command.args();

        let Some(catalog) = ctx.shared.get::<PluginCatalog>() else {
            client.send_privmsg(
                channel,
                format!(
                    "{}{}",
                    heading(""),
                    gray(" the plugin catalog is unavailable")
                ),
            )?;

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
/// An empty query shows a usage hint and an index of every plugin, a plugin name lists its
/// commands, and a command name shows the command's description, usage and arguments.
fn messages(catalog: &PluginCatalog, query: &str) -> Vec<String> {
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

/// Returns the messages showing a usage hint and indexing the catalog's plugins.
fn index_messages(catalog: &PluginCatalog) -> Vec<String> {
    let header = format!(
        "{}{} <plugin|command>{}",
        heading(""),
        gray(": Usage: .help"),
        gray(" Plugins:"),
    );

    let entries = catalog
        .entries
        .iter()
        .filter(|plugin| !plugin.commands.is_empty())
        .map(|plugin| plugin.name.clone())
        .collect::<Vec<_>>();

    with_list(&header, &entries)
}

/// Returns the messages listing the commands handled by `plugin`.
fn plugin_messages(plugin: &CatalogEntry) -> Vec<String> {
    let header = format!(
        "{}{}",
        heading(&format!(" ({})", plugin.name)),
        gray(": Commands:"),
    );

    let entries = plugin
        .commands
        .iter()
        .map(|command| command.trigger().to_owned())
        .collect::<Vec<_>>();

    with_list(&header, &entries)
}

/// Returns the messages describing `command`.
///
/// The reply carries the command's description, its usage synopsis and, when it accepts
/// arguments or subcommands, their descriptions in a parenthesized list.
fn command_messages(command: &CommandSpec) -> Vec<String> {
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

/// A labeled detail of a command: a bold label with a gray description.
struct Detail {
    /// The label of the detail (e.g. the usage form of an argument).
    label: String,
    /// The description of the detail.
    description: String,
}

/// Formats the visible arguments and subcommands of a command as a styled detail list.
fn command_details(info: &CommandInfoWithArgs) -> String {
    let mut details = Vec::new();

    for positional in visible_positionals(info) {
        details.push(Detail {
            label: positional_label(positional),
            description: positional.description.to_owned(),
        });
    }

    for flag in visible_flags(info) {
        details.push(Detail {
            label: flag_label(flag),
            description: flag.description.to_owned(),
        });
    }

    for subcommand in &info.commands {
        details.push(Detail {
            label: subcommand.command.name.to_owned(),
            description: subcommand.command.description.to_owned(),
        });
    }

    details_list(&details)
}

/// Formats `details` as a comma-separated list.
///
/// Each label is bold and each description gray; all but the last detail are comma-terminated
/// (with the comma inside the color when the detail has a description), and details after the
/// first carry a leading space inside their label.
fn details_list(details: &[Detail]) -> String {
    details
        .iter()
        .enumerate()
        .map(|(index, detail)| {
            let comma = if index + 1 == details.len() { "" } else { "," };

            let label = if index == 0 {
                bold(&detail.label)
            } else {
                bold(&format!(" {}", detail.label))
            };

            if detail.description.is_empty() {
                format!("{label}{comma}")
            } else {
                format!(
                    "{label}{}",
                    gray(&format!(": {}{comma}", detail.description))
                )
            }
        })
        .collect()
}

/// Prepends the packed list of `entries` to `header`.
///
/// Lists longer than [`MAX_MESSAGE_LENGTH`] are split at entry boundaries; the continuation
/// messages carry the remaining entries without the header. The header counts against the
/// budget of the first message.
fn with_list(header: &str, entries: &[String]) -> Vec<String> {
    let budget = MAX_MESSAGE_LENGTH.saturating_sub(header.len() + 1);
    let lists = pack(entries, budget);

    if lists.is_empty() {
        return vec![header.to_owned()];
    }

    std::iter::once(format!("{header} {}", lists[0]))
        .chain(lists.into_iter().skip(1))
        .collect()
}

/// Packs `entries` into as few messages as possible without exceeding `max_length` or splitting
/// an entry.
fn pack(entries: &[String], max_length: usize) -> Vec<String> {
    let mut messages = Vec::new();
    let mut message = String::new();

    for entry in entries {
        if !message.is_empty() && message.len() + SEPARATOR.len() + entry.len() > max_length {
            messages.push(std::mem::take(&mut message));
        }

        if !message.is_empty() {
            message.push_str(SEPARATOR);
        }

        message.push_str(entry);
    }

    if !message.is_empty() {
        messages.push(message);
    }

    messages
}

/// Formats the header of a help reply, with `subject` appended inside the bold heading.
fn heading(subject: &str) -> String {
    format!("{REPLY_PREFIX}{RESET}{BOLD} Help{subject}{BOLD}")
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

/// Formats `s` in bold.
fn bold(s: &str) -> String {
    format!("\x02{s}\x02")
}

/// Formats `s` in the muted scaffolding color.
fn gray(s: &str) -> String {
    format!("{COLOR}{s}{RESET}")
}

/// Strips the leading command sigil (`.` or `!`) from `name`.
fn normalize(name: &str) -> &str {
    name.trim_start_matches(['.', '!'])
}

/// Finds the plugin and command handling the given command name.
fn find_command<'a>(
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
fn find_plugin<'a>(catalog: &'a PluginCatalog, query: &str) -> Option<&'a CatalogEntry> {
    let query = normalize(query);

    catalog.entries.iter().find(|plugin| plugin.name == query)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALERT: CommandSpec = CommandSpec::new(".alert", "Schedule an alert to be posted later");
    const ALERT_COMMANDS: &[CommandSpec] = &[ALERT];

    const DIG: CommandSpec =
        CommandSpec::with_args::<DigOpts>(".dig", "Look up DNS records for a domain");
    const DIG_COMMANDS: &[CommandSpec] = &[DIG];

    const IMDB: CommandSpec = CommandSpec::new("!imdb", "Search IMDb and post the top match");
    const IMDB_COMMANDS: &[CommandSpec] = &[IMDB];

    const WEATHER: CommandSpec = CommandSpec::new(".weather", "Show the current weather");
    const WEATHER_COMMANDS: &[CommandSpec] = &[WEATHER];

    const BARE: CommandSpec = CommandSpec::new(".bare", "");

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

    const REPEAT: CommandSpec =
        CommandSpec::with_args::<RepeatOpts>(".repeat", "Repeat the given values");
    const REPEAT_COMMANDS: &[CommandSpec] = &[REPEAT];

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
            entries: vec![
                CatalogEntry {
                    name: "alert".into(),
                    authors: vec!["John Doe <john.doe@example.com>".into()],
                    commands: ALERT_COMMANDS.to_vec(),
                    url_hosts: Vec::new(),
                },
                CatalogEntry {
                    name: "dig".into(),
                    authors: vec![],
                    commands: DIG_COMMANDS.to_vec(),
                    url_hosts: Vec::new(),
                },
                CatalogEntry {
                    name: "imdb".into(),
                    authors: vec![],
                    commands: IMDB_COMMANDS.to_vec(),
                    url_hosts: Vec::new(),
                },
                CatalogEntry {
                    name: "openweathermap".into(),
                    authors: vec![],
                    commands: WEATHER_COMMANDS.to_vec(),
                    url_hosts: Vec::new(),
                },
                CatalogEntry {
                    name: "hooks".into(),
                    authors: vec![],
                    commands: Vec::new(),
                    url_hosts: Vec::new(),
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
        let command = CommandSpec::with_args::<SubcommandOpts>(".stats", "Show statistics");
        let info = command.args_info().unwrap();

        assert_eq!(usage(".stats", &info), ".stats <command> [<args>]");
    }

    #[test]
    fn describes_commands() {
        assert_eq!(
            command_messages(&DIG)[0],
            concat!(
                "\x0310>\x0f\x02 Help (.dig)\x02",
                "\x0310: Look up DNS records for a domain\x0f",
                "\x0310 Usage:\x0f .dig [-s] <name>",
                "\x0310 (\x0f",
                "\x02<name>\x02\x0310: the domain to look up,\x0f",
                "\x02 -s, --short\x02\x0310: only display the answer section\x0f",
                ")"
            )
        );
    }

    #[test]
    fn describes_commands_without_usage() {
        assert_eq!(
            command_messages(&BARE)[0],
            "\x0310>\x0f\x02 Help (.bare)\x02"
        );
    }

    #[test]
    fn describes_repeating_arguments_and_skips_hidden_ones() {
        assert_eq!(
            command_messages(&REPEAT)[0],
            concat!(
                "\x0310>\x0f\x02 Help (.repeat)\x02",
                "\x0310: Repeat the given values\x0f",
                "\x0310 Usage:\x0f .repeat [--values <values...>] [<files...>]",
                "\x0310 (\x0f",
                "\x02[<files...>]\x02\x0310: files to process,\x0f",
                "\x02 --values <values>\x02\x0310: values to repeat\x0f",
                ")"
            )
        );
    }

    #[test]
    fn describes_subcommands() {
        let command = CommandSpec::with_args::<SubcommandOpts>(".stats", "Show statistics");

        assert_eq!(
            command_messages(&command)[0],
            concat!(
                "\x0310>\x0f\x02 Help (.stats)\x02",
                "\x0310: Show statistics\x0f",
                "\x0310 Usage:\x0f .stats <command> [<args>]",
                "\x0310 (\x0f",
                "\x02stats\x02\x0310: Display statistics.\x0f",
                ")"
            )
        );
    }

    #[test]
    fn formats_details_from_arguments() {
        let info = DIG.args_info().unwrap();

        assert_eq!(
            command_details(&info),
            concat!(
                "\x02<name>\x02\x0310: the domain to look up,\x0f",
                "\x02 -s, --short\x02\x0310: only display the answer section\x0f",
            )
        );
    }

    #[test]
    fn omits_descriptions_when_missing() {
        let details = vec![
            Detail {
                label: "<a>".into(),
                description: String::new(),
            },
            Detail {
                label: "<b>".into(),
                description: "b description".into(),
            },
        ];

        assert_eq!(
            details_list(&details),
            "\x02<a>\x02,\x02 <b>\x02\x0310: b description\x0f"
        );
    }

    #[test]
    fn indexes_plugins_with_commands() {
        assert_eq!(
            messages(&catalog(), "")[0],
            concat!(
                "\x0310>\x0f\x02 Help\x02",
                "\x0310: Usage: .help\x0f <plugin|command>",
                "\x0310 Plugins:\x0f alert\x0310,\x0f dig\x0310,\x0f imdb\x0310,\x0f \
                 openweathermap",
            )
        );
    }

    #[test]
    fn lists_plugin_commands() {
        assert_eq!(
            messages(&catalog(), "openweathermap"),
            vec!["\x0310>\x0f\x02 Help (openweathermap)\x02\x0310: Commands:\x0f .weather"]
        );
    }

    #[test]
    fn lists_plugins_without_commands() {
        assert_eq!(
            messages(&catalog(), "hooks"),
            vec!["\x0310>\x0f\x02 Help (hooks)\x02\x0310: Commands:\x0f"]
        );
    }

    #[test]
    fn reports_unmatched_queries() {
        let rendered = messages(&catalog(), "missing");

        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].starts_with("\x0310>\x0f\x02 Help\x02"));
        assert!(rendered[0].contains("no plugin or command matches"));
    }

    #[test]
    fn packs_entries_without_exceeding_the_message_length() {
        let entries = (0..100)
            .map(|index| format!("entry-{index:03}{}", "x".repeat(70)))
            .collect::<Vec<_>>();

        let packed = pack(&entries, MAX_MESSAGE_LENGTH);

        assert!(packed.len() > 1);
        assert!(
            packed
                .iter()
                .all(|message| message.len() <= MAX_MESSAGE_LENGTH)
        );
        assert!(packed.iter().all(|message| message.contains("entry-")));
    }

    #[test]
    fn splits_long_lists_into_unheaded_continuations() {
        let catalog = PluginCatalog {
            entries: (0..100)
                .map(|index| CatalogEntry {
                    name: format!("plugin-{index:03}{}", "x".repeat(90)),
                    authors: vec![],
                    commands: vec![HELP],
                    url_hosts: Vec::new(),
                })
                .collect(),
        };

        let rendered = messages(&catalog, "");

        assert!(rendered.len() > 1);
        assert!(
            rendered
                .iter()
                .all(|message| message.len() <= MAX_MESSAGE_LENGTH)
        );
        assert!(rendered[0].starts_with("\x0310>\x0f\x02 Help\x02"));
        assert!(rendered[0].contains("plugin-000"));
        assert!(rendered[1].contains("plugin-"));
        assert!(!rendered[1].contains("Help"));
    }
}
