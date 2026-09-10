//! List the plugins the bot has loaded and the commands they handle.
//!
//! The `.help` command reads the plugin catalog published by the registry, so it always reflects
//! the plugins that were successfully registered.

use crate::plugin::prelude::*;

/// The `.help` command.
const HELP: Prefix = Prefix::new(".help");

/// The maximum length of a help line, leaving room for formatting and the IRC line terminator.
const MAX_LINE_LENGTH: usize = 350;

/// Separator between plugin entries on a help line.
const SEPARATOR: &str = "  ";

/// Help plugin.
///
/// Responds to `.help` with every loaded plugin and the commands it handles, or with details for
/// a single plugin when given a plugin name or command as an argument.
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

    fn commands(&self) -> &'static [Prefix] {
        &[HELP]
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
            for line in plugin_lines(&catalog) {
                client.send_privmsg(channel, formatted(&line))?;
            }
        } else {
            let line = find_plugin(&catalog, query).map_or_else(
                || format!("no plugin or command matches `{query}`"),
                detailed_line,
            );

            client.send_privmsg(channel, formatted(&line))?;
        }

        Ok(())
    }
}

/// Returns the plugin catalog lines, batching entries so each line fits in an IRC message.
fn plugin_lines(catalog: &PluginCatalog) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();

    for plugin in &catalog.plugins {
        if plugin.commands.is_empty() {
            continue;
        }

        let entry = plugin_line(plugin);

        if !line.is_empty() && line.len() + SEPARATOR.len() + entry.len() > MAX_LINE_LENGTH {
            lines.push(std::mem::take(&mut line));
        }

        if !line.is_empty() {
            line.push_str(SEPARATOR);
        }

        line.push_str(&entry);
    }

    if !line.is_empty() {
        lines.push(line);
    }

    lines
}

/// Formats a plugin and the commands it handles.
fn plugin_line(plugin: &PluginInfo) -> String {
    format!("{}: {}", plugin.name, commands(plugin))
}

/// Formats a plugin with its commands and authors.
fn detailed_line(plugin: &PluginInfo) -> String {
    let authors = plugin
        .authors
        .iter()
        .map(Author::as_str)
        .collect::<Vec<_>>()
        .join(", ");

    let authors = if authors.is_empty() {
        String::new()
    } else {
        format!(" (by {authors})")
    };

    format!("{}: {}{authors}", plugin.name, commands(plugin))
}

/// Returns the plugin's commands as a comma-separated list.
fn commands(plugin: &PluginInfo) -> String {
    plugin
        .commands
        .iter()
        .map(Prefix::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Finds the plugin with the given name, or the one handling the given command.
fn find_plugin<'a>(catalog: &'a PluginCatalog, query: &str) -> Option<&'a PluginInfo> {
    let query = query.trim_start_matches('.');

    catalog.plugins.iter().find(|plugin| {
        plugin.name == query
            || plugin
                .commands
                .iter()
                .any(|command| command.as_str().trim_start_matches('.') == query)
    })
}

/// Formats `s` as a help response.
fn formatted(s: &str) -> String {
    format!("\x0310>\x0f\x02 Help\x02\x0310: {s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALERT: Prefix = Prefix::new(".alert");
    const ALERT_COMMANDS: &[Prefix] = &[ALERT];

    fn catalog() -> PluginCatalog {
        PluginCatalog {
            plugins: vec![
                PluginInfo {
                    name: "alert".into(),
                    authors: vec!["John Doe <john.doe@example.com>".into()],
                    commands: ALERT_COMMANDS,
                },
                PluginInfo {
                    name: "quiet".into(),
                    authors: vec![],
                    commands: &[],
                },
            ],
        }
    }

    #[test]
    fn finds_plugins_by_name_and_command() {
        let catalog = catalog();

        assert_eq!(
            find_plugin(&catalog, "alert").map(|plugin| plugin.name.as_str()),
            Some("alert")
        );
        assert_eq!(
            find_plugin(&catalog, ".alert").map(|plugin| plugin.name.as_str()),
            Some("alert")
        );
        assert!(find_plugin(&catalog, "missing").is_none());
    }

    #[test]
    fn skips_plugins_without_commands() {
        let lines = plugin_lines(&catalog());

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("alert: .alert"));
        assert!(!lines[0].contains("quiet"));
    }

    #[test]
    fn detailed_line_includes_authors() {
        let catalog = catalog();
        let plugin = find_plugin(&catalog, "alert").unwrap();

        assert_eq!(
            detailed_line(plugin),
            "alert: .alert (by John Doe <john.doe@example.com>)"
        );
    }
}
