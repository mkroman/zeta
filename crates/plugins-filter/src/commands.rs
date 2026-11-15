//! The `.filter add`, `list` and `delete` handlers.
//!
//! Each handler validates its arguments, persists through the
//! [`FilterService`](crate::FilterService), and replies with the formatted result of the
//! operation.

use tracing::{debug, warn};

use zeta_plugin::irc::{labeled_reply, send_reply};
use zeta_plugin::{Chain, Client, ZetaError};
use zeta_utils::text;

use crate::FilterPlugin;
use crate::args::{Add, Delete, List};
use crate::format::{changed_response, describe, listing};

impl FilterPlugin {
    /// Adds one or more filters from the `add` subcommand.
    pub(super) async fn add(
        &self,
        client: &Client,
        channel: &str,
        nickname: &str,
        opts: Add,
    ) -> Result<(), ZetaError> {
        let scope = match opts.validate(channel) {
            Ok(scope) => scope,
            Err(error) => {
                send_reply(client, channel, labeled_reply("Filter", &error))?;

                return Ok(());
            }
        };

        let new_filters = opts.new_filters(scope.as_deref(), nickname);
        let total = new_filters.len();
        let mut added = Vec::with_capacity(total);

        for new_filter in new_filters {
            match self.service.add(new_filter).await {
                Ok(filter) => added.push(filter),
                Err(error) => {
                    warn!(
                        exception.message = %Chain(&error),
                        filter.count = total,
                        "could not add the filter"
                    );

                    let response = if added.is_empty() {
                        "could not add the filter".to_owned()
                    } else {
                        format!(
                            "added {} of {total} filters, then could not add the rest",
                            added.len()
                        )
                    };

                    send_reply(client, channel, labeled_reply("Filter", response))?;

                    return Ok(());
                }
            }
        }

        debug!(filter.count = total, "added filters");

        send_reply(client, channel, changed_response("Filter", "Added", &added))?;

        Ok(())
    }

    /// Lists the filters matching the `list` subcommand criteria.
    pub(super) fn list(
        &self,
        client: &Client,
        channel: &str,
        opts: &List,
    ) -> Result<(), ZetaError> {
        let filters = self.service.select(&opts.criteria());

        let response = if filters.is_empty() {
            if self.service.is_empty() {
                labeled_reply("Filter", "No filters are configured.")
            } else {
                labeled_reply("Filter", "No filters match your criteria.")
            }
        } else {
            listing(
                labeled_reply(
                    "Filter",
                    format_args!(
                        "{} matching your criteria: ",
                        text::pluralize(filters.len(), "filter")
                    ),
                ),
                &filters,
                0,
            )
        };

        send_reply(client, channel, response)?;

        Ok(())
    }

    /// Deletes a filter by id, or all filters matching the `delete` subcommand criteria.
    pub(super) async fn delete(
        &self,
        client: &Client,
        channel: &str,
        opts: Delete,
    ) -> Result<(), ZetaError> {
        if let Some(id) = opts.id {
            if opts.criteria().is_some() {
                send_reply(
                    client,
                    channel,
                    labeled_reply("Filter", "specify either a filter id or criteria, not both"),
                )?;

                return Ok(());
            }

            // The removed filter is named in the reply, and an unknown id is answered without
            // touching the database.
            let Some(filter) = self.service.find(id) else {
                send_reply(
                    client,
                    channel,
                    labeled_reply("Filter", format_args!("No filter with id {id}.")),
                )?;

                return Ok(());
            };

            let response = match self.service.delete_ids(&[id]).await {
                Ok(1) => format!("Removed {}.", describe(&filter)),
                Ok(_) => format!("No filter with id {id}."),
                Err(error) => {
                    warn!(
                        exception.message = %Chain(&error),
                        "could not delete the filter"
                    );

                    "could not delete the filter".to_owned()
                }
            };

            send_reply(client, channel, labeled_reply("Filter", &response))?;

            return Ok(());
        }

        let Some(criteria) = opts.criteria() else {
            send_reply(
                client,
                channel,
                labeled_reply(
                    "Filter",
                    "specify a filter id, or at least one criterion to match filters by (e.g. \
                     `.filter delete --host example.com`)",
                ),
            )?;

            return Ok(());
        };

        let filters = self.service.select(&criteria);

        if filters.is_empty() {
            send_reply(
                client,
                channel,
                labeled_reply("Filter", "No filters match your criteria."),
            )?;

            return Ok(());
        }

        let ids: Vec<i32> = filters.iter().map(|filter| filter.id).collect();

        if !opts.force {
            const FORCE_SUFFIX: &str = " - use --force to proceed.";

            let mut confirmation = listing(
                labeled_reply(
                    "Filter",
                    format_args!(
                        "{} matching the criteria will be deleted: ",
                        text::pluralize(filters.len(), "filter")
                    ),
                ),
                &filters,
                FORCE_SUFFIX.len(),
            );

            confirmation.push_str(FORCE_SUFFIX);

            send_reply(client, channel, confirmation)?;

            return Ok(());
        }

        let response = match self.service.delete_ids(&ids).await {
            Ok(removed) if removed == filters.len() as u64 => {
                changed_response("Filter", "Removed", &filters)
            }
            Ok(removed) => labeled_reply(
                "Filter",
                format_args!("Removed {removed} of {} matching filters.", filters.len()),
            ),
            Err(error) => {
                warn!(
                    exception.message = %Chain(&error),
                    filter.count = ids.len(),
                    "could not delete the filters"
                );

                labeled_reply("Filter", "could not delete the filters")
            }
        };

        send_reply(client, channel, response)?;

        Ok(())
    }
}
