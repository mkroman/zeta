//! Describe the plugins the bot has loaded and the commands they handle.
//!
//! The `.help` command reads the plugin catalog published by the registry, so it always reflects
//! the plugins that were successfully registered. Invoked without arguments it replies with a
//! usage hint and an index of the registered plugins that handle commands, with a plugin name it
//! lists the plugin's commands, and with a command name it shows the command's description and
//! usage synopsis derived from its `clap` argument type, along with descriptions of its arguments
//! and subcommands.
//!
//! Lists longer than one IRC line are split across continuation messages without splitting an
//! entry.

mod format;

use zeta_plugin::prelude::*;

use format::{gray, heading, messages};

/// Help plugin.
///
/// Responds to `.help` with a usage hint and an index of every plugin that handles commands,
/// `.help <plugin>` with the plugin's commands, or `.help <command>` with the command's
/// description, usage and arguments derived from its `clap` type.
#[derive(Default)]
pub struct Help;

#[plugin(name = "help")]
impl Help {
    /// Replies with a usage hint and plugin index, a plugin's commands, or a command's usage.
    #[command(".help", "List plugins and commands, or show usage for one", event)]
    async fn help(
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
