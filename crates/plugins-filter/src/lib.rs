//! Silences matched senders and URLs in channels.
//!
//! Filters are managed with the `.filter add|list|delete` command and stored in the database,
//! from which they are loaded into memory at launch. Every filter carries optional criteria —
//! the channel, the host and path of a URL, and the nickname, username (ident), and hostname of
//! the sender — where all of the set criteria must match. Patterns support `*` and `?`
//! wildcards.
//!
//! What a match means depends on the criteria: a filter with URL criteria (host or path)
//! ignores the matching URLs, which the host's dispatcher drops before any URL plugin sees
//! them. A filter with sender criteria only ignores the matching senders entirely: their
//! commands do not trigger, their messages are not delivered to the message subscribers, and
//! their URLs are ignored.
//!
//! The command is restricted to admins through `[irc] admin_hostmasks`, using the plugin API's
//! shared [`Hostmasks`] matcher (also used by `.unwall`).
//!
//! The filter service is published to [`Context::shared`] as a type-erased [`FilterHandle`] when
//! the plugin is constructed; the host's dispatcher consults it through the [`FilterHandle`]
//! before delivering URL events, so filtered URLs never reach the URL plugins.
//!
//! The module is split by concern: `args` holds the command-line types and their validation,
//! `commands` the subcommand handlers, and `format` the reply formatting; `service`, `index`,
//! `model`, and `repository` hold the persistence and matching.

mod args;
mod commands;
mod error;
mod format;
mod index;
mod model;
mod repository;
mod service;
#[cfg(test)]
mod tests;

pub use error::Error;
pub use model::{Criteria, Filter, NewFilter};
pub use service::FilterService;

use std::fmt;
use std::sync::Arc;

use zeta_plugin::irc::{labeled_reply, parse_or_usage, send_reply};
use zeta_plugin::{
    ArgsMode, Client, CommandEvent, CommandSpec, Context, FilterHandle, Hostmasks, Sender,
    Subscriptions, ZetaError, plugin, plugin_err,
};

use args::{Opts, Subcommand};

/// The `.filter` command.
const FILTER: CommandSpec =
    CommandSpec::with_args::<Opts>(".filter", "Manage URL and sender filters (add/list/delete)");

/// Filter plugin.
///
/// Manages the database-backed URL and sender filters and publishes the filter service for the
/// URL-handling plugins to consult.
pub struct FilterPlugin {
    /// The filter service, also published for other plugins to use.
    service: Arc<FilterService>,
    /// The compiled admin hostmasks; senders whose `nick!user@host` matches one of them are
    /// authorized to manage filters.
    admins: Hostmasks,
}

// Manual implementation: the service is summarized by its own redacted view and the admin
// matcher by its compiled patterns.
impl fmt::Debug for FilterPlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FilterPlugin")
            .field("service", &self.service)
            .field("admins", &self.admins)
            .finish_non_exhaustive()
    }
}

/// The reply sent to senders that are not authorized to manage filters.
const UNAUTHORIZED: &str = "You are not authorized to manage filters.";

impl FilterPlugin {
    /// Whether `sender` is authorized to manage filters.
    fn is_admin(&self, sender: Sender<'_>) -> bool {
        self.admins.matches(sender)
    }
}

#[plugin(name = "filter")]
impl FilterPlugin {
    fn new(ctx: &Context, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.command(FILTER);
        let service = Arc::new(FilterService::new(ctx.db().clone()));

        // The host resolves this handle once, when it starts the runtime, so it must be
        // published here rather than from the `loaded` hook: a later publication never reaches
        // the dispatcher. Filter changes go through the service's own index instead.
        ctx.shared()
            .publish(Arc::new(FilterHandle::new(service.clone())));

        let admins = ctx.config().admins().clone();

        Ok(Self { service, admins })
    }

    #[handler(loaded)]
    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        // The host waits for this hook before dispatching (bounded by its startup grace), so a
        // retry keeps that window short instead of leaving filtering disabled for the run.
        zeta_plugin::database::load_with_retry(|| self.service.load())
            .await
            .map_err(plugin_err)?;

        Ok(())
    }

    #[handler(command)]
    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let Some(sender) = command.sender() else {
            return Ok(());
        };

        if !self.is_admin(sender) {
            send_reply(client, channel, labeled_reply("Filter", UNAUTHORIZED))?;

            return Ok(());
        }

        let Some(opts) = parse_or_usage::<Opts>(client, command, ArgsMode::Words, |line| {
            labeled_reply("Filter", line)
        })?
        else {
            return Ok(());
        };

        match opts.command {
            Subcommand::Add(add) => self.add(client, channel, sender.nick, add).await,
            Subcommand::List(list) => self.list(client, channel, &list),
            Subcommand::Delete(delete) => self.delete(client, channel, delete).await,
        }
    }
}
