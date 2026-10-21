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
//! The command is restricted to admins through `[irc] admin_hostmasks`: the hostmask matching
//! lives in the crate-private `hostmask` module, shared with `.unwall`.
//!
//! The filter service is published to [`Context::shared`] as a type-erased [`FilterHandle`] handle
//! when the plugin is constructed; the host's dispatcher consults it through the [`FilterGate`]
//! facade before delivering URL events, so filtered URLs never reach the URL plugins.

mod command;
mod error;
mod index;
mod model;
mod pattern;
mod reply;
mod repository;
mod service;

// The module types are re-exported as part of the module's API surface, even though the plugin
// itself only handles them by value.
#[allow(unused_imports)]
pub use error::Error;
pub use model::{Criteria, FilterRule, NewFilterRule};
pub use service::FilterService;

use std::sync::Arc;

use tracing::debug;

use self::command::{Add, Delete, FILTER, List, Options, Subcommand};
use self::reply::{changed_response, describe, listing, plural};
use crate::hostmask::AdminHostmasks;
use crate::prelude::*;

/// The plugin name used in replies.
const NAME: &str = "Filter";

/// The reply sent to senders that are not authorized to manage filters.
const UNAUTHORIZED: &str = "You are not authorized to manage filters.";

/// Filter plugin.
///
/// Manages the database-backed URL and sender filters and publishes the filter service for the
/// URL-handling plugins to consult.
pub struct Filter {
    /// The filter service, also published for other plugins to use.
    service: Arc<FilterService>,
    /// The compiled admin hostmasks; senders whose `nick!user@host` matches one of them are
    /// authorized to manage filters.
    admins: AdminHostmasks,
}

impl Filter {
    /// Adds one or more filters from the `add` subcommand.
    async fn add(
        &self,
        client: &IrcClient,
        channel: &str,
        nickname: &str,
        opts: Add,
    ) -> Result<(), PluginError> {
        let new_filters = match opts.into_filters(channel, nickname) {
            Ok(new_filters) => new_filters,
            Err(error) => {
                client.send_privmsg(channel, reply(NAME, error))?;

                return Ok(());
            }
        };

        let total = new_filters.len();
        let mut added = Vec::with_capacity(total);

        for new_filter in new_filters {
            match self.service.create(new_filter).await {
                Ok(filter) => added.push(filter),
                Err(error) => {
                    let response = if added.is_empty() {
                        format!("could not add the filter: {error}")
                    } else {
                        format!(
                            "added {} of {total} filters, then could not add the rest: {error}",
                            added.len()
                        )
                    };

                    client.send_privmsg(channel, reply(NAME, response))?;

                    return Ok(());
                }
            }
        }

        debug!(count = total, "added filters");

        client.send_privmsg(channel, changed_response("Added", &added))?;

        Ok(())
    }

    /// Lists the filters matching the `list` subcommand criteria.
    fn list(&self, client: &IrcClient, channel: &str, opts: &List) -> Result<(), PluginError> {
        let filters = self.service.matching(&Criteria::from(opts));

        let response = if filters.is_empty() {
            if self.service.is_empty() {
                reply(NAME, "No filters are configured.")
            } else {
                reply(NAME, "No filters match your criteria.")
            }
        } else {
            listing(
                reply(
                    NAME,
                    format!(
                        "{} matching your criteria: ",
                        plural(filters.len(), "filter")
                    ),
                ),
                &filters,
                0,
            )
        };

        client.send_privmsg(channel, response)?;

        Ok(())
    }

    /// Deletes a filter by id, or all filters matching the `delete` subcommand criteria.
    async fn delete(
        &self,
        client: &IrcClient,
        channel: &str,
        opts: Delete,
    ) -> Result<(), PluginError> {
        if let Some(id) = opts.id {
            if opts.criteria().is_some() {
                client.send_privmsg(
                    channel,
                    reply(NAME, "specify either a filter id or criteria, not both"),
                )?;

                return Ok(());
            }

            return self.delete_by_id(client, channel, id).await;
        }

        let Some(criteria) = opts.criteria() else {
            client.send_privmsg(
                channel,
                reply(
                    NAME,
                    "specify a filter id, or at least one criterion to match filters by (e.g. \
                     `.filter delete --host example.com`)",
                ),
            )?;

            return Ok(());
        };

        self.delete_matching(client, channel, &criteria, opts.force)
            .await
    }

    /// Deletes the single filter with `id`, naming it in the reply.
    async fn delete_by_id(
        &self,
        client: &IrcClient,
        channel: &str,
        id: i32,
    ) -> Result<(), PluginError> {
        // The removed filter is named in the reply, and an unknown id is answered without
        // touching the database.
        let Some(filter) = self.service.get(id) else {
            client.send_privmsg(channel, reply(NAME, format!("No filter with id {id}.")))?;

            return Ok(());
        };

        let response = match self.service.remove_by_ids(&[id]).await {
            Ok(1) => format!("Removed {}.", describe(&filter)),
            Ok(_) => format!("No filter with id {id}."),
            Err(error) => format!("could not delete the filter: {error}"),
        };

        client.send_privmsg(channel, reply(NAME, &response))?;

        Ok(())
    }

    /// Deletes the filters matching `criteria`, after a confirmation unless `force` is set.
    async fn delete_matching(
        &self,
        client: &IrcClient,
        channel: &str,
        criteria: &Criteria,
        force: bool,
    ) -> Result<(), PluginError> {
        let filters = self.service.matching(criteria);

        if filters.is_empty() {
            client.send_privmsg(channel, reply(NAME, "No filters match your criteria."))?;

            return Ok(());
        }

        if !force {
            const FORCE_SUFFIX: &str = " - use --force to proceed.";

            let mut confirmation = listing(
                reply(
                    NAME,
                    format!(
                        "{} matching the criteria will be deleted: ",
                        plural(filters.len(), "filter")
                    ),
                ),
                &filters,
                FORCE_SUFFIX.len(),
            );

            confirmation.push_str(FORCE_SUFFIX);

            client.send_privmsg(channel, confirmation)?;

            return Ok(());
        }

        let ids: Vec<i32> = filters.iter().map(|filter| filter.id).collect();

        let response = match self.service.remove_by_ids(&ids).await {
            Ok(removed) if removed == filters.len() as u64 => changed_response("Removed", &filters),
            Ok(removed) => format!("Removed {removed} of {} matching filters.", filters.len()),
            Err(error) => format!("could not delete the filters: {error}"),
        };

        client.send_privmsg(channel, reply(NAME, response))?;

        Ok(())
    }
}

#[async_trait]
impl Plugin<Context> for Filter {
    type Settings = NoSettings;

    fn new(
        ctx: &Context,
        _settings: &NoSettings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, PluginError> {
        subscriptions.subscribe_command(FILTER);

        let service = Arc::new(FilterService::new(ctx.db.clone()));

        ctx.shared
            .publish(Arc::new(FilterHandle::new(service.clone())));

        let admins = AdminHostmasks::compile(&ctx.config.admin_hostmasks, "manage filters");

        Ok(Self { service, admins })
    }

    async fn start(&mut self, _ctx: &Context, _client: &IrcClient) -> Result<(), PluginError> {
        self.service.load().await.map_err(plugin_err)?;

        Ok(())
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &IrcClient,
        command: &CommandEvent,
    ) -> Result<(), PluginError> {
        let channel = command.channel();
        let Some(sender) = command.sender() else {
            return Ok(());
        };

        if !self.admins.matches(sender) {
            client.send_privmsg(channel, reply(NAME, UNAUTHORIZED))?;

            return Ok(());
        }

        let Some(opts) =
            parse_words_or_usage::<Options>(client, command, |line| reply(NAME, line))?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publishes_a_url_filters_handle_for_the_host() {
        // The host's dispatcher consults the `FilterGate` facade, which only sees the service
        // through the type-erased handle published here — without it, no URL is ever filtered.
        let ctx = zeta_test_support::test_context();
        let mut subscriptions = Subscriptions::new();

        <Filter as Plugin<Context>>::new(&ctx, &NoSettings, &mut subscriptions).unwrap();

        assert!(ctx.shared.get::<FilterHandle>().is_some());
    }
}
