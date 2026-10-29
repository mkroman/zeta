//! Formatting and catalog introspection for help replies.
//!
//! Renders the plugin index, a plugin's commands, and a command's description, usage synopsis,
//! and argument details, splitting long lists across continuation messages.

use clap::{Arg, ArgAction, Command};

use zeta_plugin::prelude::*;
use zeta_utils::irc::append_entries_within_budget;

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
fn command_details(info: &Command) -> String {
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
fn with_list<S: AsRef<str>>(header: &str, entries: &[S]) -> Vec<String> {
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
fn pack<S: AsRef<str>>(entries: &[S], max_length: usize) -> Vec<String> {
    let mut messages = Vec::new();
    let mut remaining = entries;

    while !remaining.is_empty() {
        let mut message = String::new();
        let appended =
            append_entries_within_budget(&mut message, remaining.iter(), SEPARATOR, max_length, 0);

        messages.push(message);
        remaining = &remaining[appended..];
    }

    messages
}

/// Formats the header of a help reply, with `subject` appended inside the bold heading.
pub fn heading(subject: &str) -> String {
    format!("{REPLY_MARKER}{RESET}{BOLD} Help{subject}{BOLD}")
}

/// Builds the usage synopsis of a command from its argument information.
///
/// Optional flags and arguments are bracketed and repeating ones are suffixed with `...`;
/// subcommands are summarized rather than listed.
fn usage(prefix: &str, info: &Command) -> String {
    let mut usage = prefix.to_string();

    if info.has_subcommands() {
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
fn visible_flags(info: &Command) -> impl Iterator<Item = &Arg> {
    info.get_opts().filter(|flag| !flag.is_hide_set())
}

/// Returns the positional arguments that should appear in help output.
fn visible_positionals(info: &Command) -> impl Iterator<Item = &Arg> {
    info.get_positionals()
        .filter(|positional| !positional.is_hide_set())
}

/// Returns the subcommands that should appear in help output.
fn visible_subcommands(info: &Command) -> impl Iterator<Item = &Command> {
    info.get_subcommands()
        .filter(|subcommand| !subcommand.is_hide_set())
}

/// Whether the argument accepts more than one value.
fn is_repeating(argument: &Arg) -> bool {
    matches!(*argument.get_action(), ArgAction::Append)
        || argument
            .get_num_args()
            .is_some_and(|range| range.max_values() > 1)
}

/// Formats an argument's long form, e.g. `--short`, when it has one.
fn long_flag(argument: &Arg) -> Option<String> {
    argument.get_long().map(|long| format!("--{long}"))
}

/// Returns the name clap uses for an argument's value, e.g. `HOST`.
///
/// clap derives a value name from the field name and lets the plugin type override it, so prefer
/// it over the internal id, which only carries the Rust field name.
fn value_name_text(argument: &Arg) -> String {
    argument
        .get_value_names()
        .and_then(|names| names.first())
        .map_or_else(|| argument.get_id().to_string(), ToString::to_string)
}

/// Formats an argument's value placeholder, e.g. `<HOST>`.
fn value_name(argument: &Arg) -> String {
    format!("<{}>", value_name_text(argument))
}

/// Formats an argument's value placeholder, marking a repeating argument with `...`.
fn value_label(argument: &Arg) -> String {
    if is_repeating(argument) {
        format!("<{}...>", value_name_text(argument))
    } else {
        value_name(argument)
    }
}

/// Formats a flag for the usage synopsis, using its short form when it has one.
fn flag_usage(flag: &Arg) -> String {
    let mut label = flag.get_short().map_or_else(
        || long_flag(flag).unwrap_or_else(|| format!("--{}", flag.get_id())),
        |short| format!("-{short}"),
    );

    if flag.get_action().takes_values() {
        label.push(' ');
        label.push_str(&value_label(flag));
    }

    if flag.is_required_set() {
        label
    } else {
        format!("[{label}]")
    }
}

/// Formats a flag for the argument list, showing both its short and long forms.
///
/// A short-only argument is rendered as just `-x`; clap does not accept the long form that the
/// field id would suggest. Arguments with neither form fall back to the id so the list stays
/// readable.
fn flag_label(flag: &Arg) -> String {
    let mut label = String::new();

    if let Some(short) = flag.get_short() {
        label.push('-');
        label.push(short);
    }

    if let Some(long) = long_flag(flag) {
        if !label.is_empty() {
            label.push_str(", ");
        }

        label.push_str(&long);
    }

    if label.is_empty() {
        label.push_str("--");
        label.push_str(flag.get_id().as_str());
    }

    if flag.get_action().takes_values() {
        label.push(' ');
        label.push_str(&value_name(flag));
    }

    label
}

/// Formats a positional argument, marking its optionality and repetition.
fn positional_label(positional: &Arg) -> String {
    let label = value_label(positional);

    if positional.is_required_set() {
        label
    } else {
        format!("[{label}]")
    }
}

/// Formats `s` in bold.
fn bold(s: &str) -> String {
    format!("{BOLD}{s}{BOLD}")
}

/// Formats `s` in the muted scaffolding color.
pub fn gray(s: &str) -> String {
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
    use crate::HELP;
    use clap::Parser;
    use zeta_utils::irc::strip_formatting;

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
    #[derive(Parser)]
    #[allow(dead_code)]
    struct DigOpts {
        /// the domain to look up
        name: String,
        /// only display the answer section
        #[arg(short = 's', long)]
        short: bool,
    }

    /// Repeat values.
    #[derive(Parser)]
    #[allow(dead_code)]
    struct RepeatOpts {
        /// values to repeat
        #[arg(long)]
        values: Vec<String>,
        /// files to process
        files: Vec<String>,
        /// not shown in help
        #[arg(long, hide = true)]
        hidden: bool,
    }

    const REPEAT: CommandSpec =
        CommandSpec::with_args::<RepeatOpts>(".repeat", "Repeat the given values");
    const REPEAT_COMMANDS: &[CommandSpec] = &[REPEAT];

    /// Short-only flag fixture.
    #[derive(Parser)]
    #[allow(dead_code)]
    struct ShortOnlyOpts {
        /// a switch with no long form
        #[arg(short = 'q')]
        quiet: bool,
    }

    const SHORT_ONLY: CommandSpec =
        CommandSpec::with_args::<ShortOnlyOpts>(".short", "Short-only flag");

    /// Subcommand fixture.
    #[derive(Parser)]
    #[allow(dead_code)]
    struct SubcommandOpts {
        #[command(subcommand)]
        command: Subcommands,
    }

    #[derive(clap::Subcommand)]
    #[allow(dead_code)]
    enum Subcommands {
        Stats(Stats),
    }

    /// Display statistics.
    #[derive(clap::Args)]
    #[allow(dead_code)]
    struct Stats {}

    fn catalog() -> PluginCatalog {
        PluginCatalog {
            entries: vec![
                CatalogEntry {
                    name: "alert".to_owned(),
                    commands: ALERT_COMMANDS.to_vec(),
                    url_scope: UrlScope::None,
                },
                CatalogEntry {
                    name: "dig".to_owned(),
                    commands: DIG_COMMANDS.to_vec(),
                    url_scope: UrlScope::None,
                },
                CatalogEntry {
                    name: "imdb".to_owned(),
                    commands: IMDB_COMMANDS.to_vec(),
                    url_scope: UrlScope::None,
                },
                CatalogEntry {
                    name: "openweathermap".to_owned(),
                    commands: WEATHER_COMMANDS.to_vec(),
                    url_scope: UrlScope::None,
                },
                CatalogEntry {
                    name: "hooks".to_owned(),
                    commands: Vec::new(),
                    url_scope: UrlScope::None,
                },
            ],
        }
    }

    #[test]
    fn finds_commands_and_plugins_by_name() {
        let catalog = catalog();

        for query in ["alert", ".alert", ".dig", "imdb", "!imdb"] {
            assert!(find_command(&catalog, query).is_some(), "command {query:?}");
        }
        assert!(find_command(&catalog, "missing").is_none());

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
    fn derives_usage_from_argument_info() {
        let info = DIG_COMMANDS[0].args_info().unwrap();
        assert_eq!(usage(".dig", &info), ".dig [-s] <NAME>");

        let info = REPEAT_COMMANDS[0].args_info().unwrap();
        assert_eq!(
            usage(".repeat", &info),
            ".repeat [--values <VALUES...>] [<FILES...>]"
        );

        let command = CommandSpec::with_args::<SubcommandOpts>(".stats", "Show statistics");
        let info = command.args_info().unwrap();
        assert_eq!(usage(".stats", &info), ".stats <command> [<args>]");
    }

    #[test]
    fn describes_commands() {
        // The exact layout of `command_messages` is pinned here; the other cases below assert
        // only the visible content.
        assert_eq!(
            command_messages(&DIG)[0],
            concat!(
                "\x0310>\x0f\x02 Help (.dig)\x02",
                "\x0310: Look up DNS records for a domain\x0f",
                "\x0310 Usage:\x0f .dig [-s] <NAME>",
                "\x0310 (\x0f",
                "\x02<NAME>\x02\x0310: the domain to look up,\x0f",
                "\x02 -s, --short\x02\x0310: only display the answer section\x0f",
                ")"
            )
        );
    }

    #[test]
    fn describes_commands_without_usage_or_with_details() {
        let bare = strip_formatting(&command_messages(&BARE)[0]);
        assert!(bare.contains("Help (.bare)"), "{bare}");
        assert!(!bare.contains("Usage"), "{bare}");

        let repeat = strip_formatting(&command_messages(&REPEAT)[0]);
        for expected in [
            "Help (.repeat)",
            "Repeat the given values",
            "Usage: .repeat [--values <VALUES...>] [<FILES...>]",
            "[<FILES...>]: files to process",
            "--values <VALUES>: values to repeat",
        ] {
            assert!(repeat.contains(expected), "missing {expected:?}: {repeat}");
        }
        assert!(!repeat.contains("hidden"), "hidden flag leaked: {repeat}");

        let command = CommandSpec::with_args::<SubcommandOpts>(".stats", "Show statistics");
        let subcommands = strip_formatting(&command_messages(&command)[0]);
        for expected in [
            "Help (.stats)",
            "Show statistics",
            "Usage: .stats <command> [<args>]",
            "stats: Display statistics",
        ] {
            assert!(
                subcommands.contains(expected),
                "missing {expected:?}: {subcommands}"
            );
        }
    }

    #[test]
    fn describes_short_only_flags_without_a_long_form() {
        let message = strip_formatting(&command_messages(&SHORT_ONLY)[0]);

        assert!(
            message.contains("-q: a switch with no long form"),
            "{message}"
        );
        assert!(!message.contains("--quiet"), "{message}");
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
            strip_formatting(&details_list(&details)),
            "<a>, <b>: b description"
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
        let cases = [
            (
                "openweathermap",
                "\x0310>\x0f\x02 Help (openweathermap)\x02\x0310: Commands:\x0f .weather",
            ),
            (
                "hooks",
                "\x0310>\x0f\x02 Help (hooks)\x02\x0310: Commands:\x0f",
            ),
        ];

        for (query, expected) in cases {
            assert_eq!(messages(&catalog(), query), vec![expected], "{query}");
        }
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
                    commands: vec![HELP],
                    url_scope: UrlScope::None,
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
