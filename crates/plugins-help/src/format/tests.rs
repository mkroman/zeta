//! Tests for help reply formatting and catalog introspection.

use clap::Parser;

use zeta_plugin::irc;
use zeta_plugin::{CatalogEntry, CommandSpec, PluginCatalog, UrlScope};

use super::catalog::{command_messages, find_command, find_plugin};
use super::details::{Detail, details_list};
use super::layout::pack;
use super::usage::usage;
use super::*;
use crate::HELP;

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
    PluginCatalog::new(vec![
        CatalogEntry::new(
            "alert".to_owned(),
            ALERT_COMMANDS.to_vec(),
            UrlScope::None,
            Vec::new(),
        ),
        CatalogEntry::new(
            "dig".to_owned(),
            DIG_COMMANDS.to_vec(),
            UrlScope::None,
            Vec::new(),
        ),
        CatalogEntry::new(
            "imdb".to_owned(),
            IMDB_COMMANDS.to_vec(),
            UrlScope::None,
            Vec::new(),
        ),
        CatalogEntry::new(
            "openweathermap".to_owned(),
            WEATHER_COMMANDS.to_vec(),
            UrlScope::None,
            Vec::new(),
        ),
        CatalogEntry::new("hooks".to_owned(), Vec::new(), UrlScope::None, Vec::new()),
    ])
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
    let bare = irc::strip_formatting(&command_messages(&BARE)[0]);
    assert!(bare.contains("Help (.bare)"), "{bare}");
    assert!(!bare.contains("Usage"), "{bare}");

    let repeat = irc::strip_formatting(&command_messages(&REPEAT)[0]);
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
    let subcommands = irc::strip_formatting(&command_messages(&command)[0]);
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
    let message = irc::strip_formatting(&command_messages(&SHORT_ONLY)[0]);

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
        irc::strip_formatting(&details_list(&details)),
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
    let catalog = PluginCatalog::new(
        (0..100)
            .map(|index| {
                CatalogEntry::new(
                    format!("plugin-{index:03}{}", "x".repeat(90)),
                    vec![HELP],
                    UrlScope::None,
                    Vec::new(),
                )
            })
            .collect(),
    );

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
