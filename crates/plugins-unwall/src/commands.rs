//! The `.unwall` command handlers, their replies, and admin authorization.

use std::fmt::Write as _;

use tracing::{debug, warn};

use zeta_plugin::irc::{self, MAX_LISTING_LENGTH};
use zeta_plugin::irc::{COLOR, RESET, labeled_reply, send_reply};
use zeta_plugin::{Chain, Client, Sender, ZetaError};
use zeta_utils::duration::TimeInWordsExt;

use super::args::Add;
use super::{NAME, Unwall, sites};

/// The reply sent to senders that are not authorized to manage unwall sites.
const UNAUTHORIZED: &str = "You are not authorized to manage unwall sites.";

impl Unwall {
    /// Whether `sender` is authorized to manage unwall sites.
    pub(super) fn is_admin(&self, sender: Sender<'_>) -> bool {
        self.admins.matches(sender)
    }

    /// Replies with the unauthorized message and yields `false` when `sender` is not an admin.
    pub(super) fn authorized(
        &self,
        client: &Client,
        channel: &str,
        sender: Sender<'_>,
    ) -> Result<bool, ZetaError> {
        if self.is_admin(sender) {
            return Ok(true);
        }

        send_reply(client, channel, labeled_reply(NAME, UNAUTHORIZED))?;

        Ok(false)
    }

    /// Adds the sites of the `add` subcommand to the covered set.
    pub(super) async fn add(
        &self,
        client: &Client,
        channel: &str,
        nickname: &str,
        opts: Add,
    ) -> Result<(), ZetaError> {
        let mut hosts = Vec::new();

        for input in &opts.hosts {
            match sites::normalize_site(input) {
                Some(host) if !hosts.contains(&host) => hosts.push(host),
                Some(_) => {}
                None => {
                    send_reply(
                        client,
                        channel,
                        labeled_reply(
                            NAME,
                            format_args!("{RESET}{input}{COLOR} does not look like a hostname."),
                        ),
                    )?;

                    return Ok(());
                }
            }
        }

        if hosts.is_empty() {
            send_reply(
                client,
                channel,
                labeled_reply(NAME, "specify at least one hostname to unwall."),
            )?;

            return Ok(());
        }

        if let Err(error) = self.service.add_sites(&hosts, nickname).await {
            warn!(
                exception.message = %Chain(&error),
                unwall.site.count = hosts.len(),
                "could not add the unwall sites"
            );

            send_reply(
                client,
                channel,
                labeled_reply(NAME, "could not add the sites."),
            )?;

            return Ok(());
        }

        debug!(unwall.site.count = hosts.len(), "added unwall sites");

        send_reply(
            client,
            channel,
            labeled_reply(
                NAME,
                format_args!("Now unwalling {}{COLOR}.", join_and(&hosts)),
            ),
        )?;

        Ok(())
    }

    /// Removes the covered sites matching the `rm`/`del`/`delete` subcommand pattern.
    pub(super) async fn remove(
        &self,
        client: &Client,
        channel: &str,
        pattern: &str,
    ) -> Result<(), ZetaError> {
        let Some(pattern) = sites::normalize_site(pattern) else {
            send_reply(
                client,
                channel,
                labeled_reply(
                    NAME,
                    format_args!(
                        "{RESET}{pattern}{COLOR} does not look like a hostname or wildcard."
                    ),
                ),
            )?;

            return Ok(());
        };

        let removed = match self.service.remove_matching(&pattern).await {
            Ok(removed) => removed,
            Err(error) => {
                warn!(
                    exception.message = %Chain(&error),
                    "could not remove the unwall sites"
                );

                send_reply(
                    client,
                    channel,
                    labeled_reply(NAME, "could not remove the sites."),
                )?;

                return Ok(());
            }
        };

        let response = if removed.is_empty() {
            format!("No unwalled sites match{RESET} {pattern}{COLOR}.")
        } else {
            format!("Removed {}{COLOR} from unwalled sites.", join_and(&removed))
        };

        send_reply(client, channel, labeled_reply(NAME, response))?;

        Ok(())
    }

    /// Summarizes the covered sites in a single line.
    pub(super) fn list(&self, client: &Client, channel: &str) -> Result<(), ZetaError> {
        let sites = self.service.sites();

        let response = if sites.is_empty() {
            labeled_reply(
                NAME,
                format_args!(
                    "{RESET}No unwalled sites yet{COLOR}: add one with\
                     {RESET} .unwall add <host>{COLOR}, e.g.{RESET} .unwall add bloomberg.com{COLOR}."
                ),
            )
        } else {
            let mut message = labeled_reply(
                NAME,
                format_args!("{RESET}{}{COLOR} unwalled sites:{RESET} ", sites.len()),
            );

            irc::append_entries_with_more(
                &mut message,
                sites.iter().map(|site| site.host.as_str()),
                sites.len(),
                &format!("{COLOR}, {RESET}"),
                MAX_LISTING_LENGTH,
                0,
                |dropped| format!("{COLOR} ({dropped} more)"),
            );

            message
        };

        send_reply(client, channel, response)?;

        Ok(())
    }

    /// Replies with the overall statistics of the unwalled links.
    pub(super) async fn stats(&self, client: &Client, channel: &str) -> Result<(), ZetaError> {
        let response = match self.service.statistics().await {
            Ok(stats) => labeled_reply(
                NAME,
                format_args!(
                    "{RESET}{}{COLOR} links unwalled across{RESET} {}{COLOR} sites \
                     by{RESET} {}{COLOR} users ({RESET}{}{COLOR} today).",
                    stats.urls, stats.hosts, stats.users, stats.urls_today
                ),
            ),
            Err(error) => {
                warn!(
                    exception.message = %Chain(&error),
                    "could not load the unwall statistics"
                );

                labeled_reply(NAME, "could not load the statistics.")
            }
        };

        send_reply(client, channel, response)?;

        Ok(())
    }

    /// Replies with the statistics of one host.
    pub(super) async fn info(
        &self,
        client: &Client,
        channel: &str,
        host: &str,
    ) -> Result<(), ZetaError> {
        let response = match self.service.host_statistics(host).await {
            Ok(Some(stats)) => labeled_reply(
                NAME,
                format_args!(
                    "{RESET}{}{COLOR} links unwalled for{RESET} {host}{COLOR} \
                     by{RESET} {}{COLOR} users ({RESET}{}{COLOR} today, most recent\
                     {RESET} {}{COLOR}).",
                    stats.urls,
                    stats.users,
                    stats.urls_today,
                    stats.latest.map_or_default(|latest| latest.time_ago()),
                ),
            ),
            Ok(None) => labeled_reply(
                NAME,
                format_args!("No unwalled links for{RESET} {host}{COLOR}."),
            ),
            Err(error) => {
                warn!(
                    exception.message = %Chain(&error),
                    "could not load the unwall statistics"
                );

                labeled_reply(NAME, "could not load the statistics.")
            }
        };

        send_reply(client, channel, response)?;

        Ok(())
    }
}

/// Joins `items` with cyan commas and a final cyan `and`, each item emphasized in the default
/// color: `\x0fa\x0310, \x0fb\x0310 and\x0f c`.
pub(crate) fn join_and(items: &[String]) -> String {
    let mut joined = String::new();

    for (index, item) in items.iter().enumerate() {
        if index == 0 {
            let _ = write!(joined, "{RESET}{item}");
        } else if index + 1 == items.len() {
            let _ = write!(joined, "{COLOR} and{RESET} {item}");
        } else {
            let _ = write!(joined, "{COLOR},{RESET} {item}");
        }
    }

    joined
}
