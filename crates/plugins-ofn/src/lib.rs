//! Old Fucking News — tracks posted URLs and calls out reposts.
//!
//! Every URL posted in a channel is recorded in the database (scoped to the channel and
//! network) and checked against the history; a repost is replied to with when and by whom it
//! was previously posted. YouTube videos are additionally tracked by their video id in
//! a dedicated table, so reposts match across `watch?v=`, `shorts/`, and `youtu.be` links.
//!
//! The host's dispatcher applies the shared URL and sender filters before delivery, so a
//! filtered URL never reaches this plugin. `.ofn` command invocations are not recorded as
//! history.
//!
//! The `.ofn stats` command shows aggregate statistics: the number of recorded URLs and
//! YouTube videos in total, and how many were added today.
//!
//! The plugin requires the `database` feature and has no settings.

mod error;
mod model;
mod repository;
mod service;

pub use error::Error;
pub use model::Statistics;
pub use service::Resource;

use clap::Parser;
use num_format::{Locale, ToFormattedString};
use tracing::warn;
use url::Url;

use self::service::OfnService;
use zeta_plugin::database::Attribution;
use zeta_plugin::irc::{labeled_reply, send_reply};
use zeta_plugin::{Client, Context, UrlEvent, ZetaError, plugin, plugin_err};
use zeta_utils::duration::TimeInWordsExt;

/// The user-facing reply for a URL that could not be recorded.
#[derive(Debug, thiserror::Error)]
enum RecordFailure {
    /// The URL has no host and cannot be recorded.
    #[error("could not record the URL: it has no host")]
    NoHost,
    /// The record could not be stored.
    #[error("could not record the URL")]
    Database(#[source] Error),
}

impl RecordFailure {
    /// Renders a failed record, naming the domain reason for a URL without a host.
    fn new(error: Error) -> Self {
        match error {
            Error::InsertUrlNoHost => Self::NoHost,
            other @ Error::Database(_) => Self::Database(other),
        }
    }
}

/// The ofn plugin: records posted URLs and reports reposts.
pub struct Ofn {
    /// The history service.
    service: OfnService,
}

impl Ofn {
    /// Scrapes a URL posted in chat and updates/checks the database.
    ///
    /// The host's dispatcher has already applied the shared filters, so every URL delivered
    /// here is recorded. A failed record is logged and answered with a fixed reply, so the
    /// database error never reaches the channel.
    #[tracing::instrument(
        name = "ofn.record_url",
        skip_all,
        err,
        fields(
            irc.user.nick = %origin.nickname,
            irc.user.name = %origin.username,
            irc.user.host = %origin.hostname,
            irc.channel = %origin.channel,
            irc.network = %origin.network
        )
    )]
    async fn process_url(
        &self,
        client: &Client,
        origin: &ChannelMessageOrigin<'_>,
        url: &Url,
    ) -> Result<(), ZetaError> {
        match self.service.record_url(origin, url).await {
            Ok(Some(resource)) => {
                let time_ago = resource.created_at().time_ago();

                send_reply(
                    client,
                    origin.channel,
                    format!(
                        "{}: OFN - posted by {} {time_ago}",
                        origin.nickname,
                        resource.nickname()
                    ),
                )?;
            }
            Ok(None) => {}
            Err(error) => {
                warn!(exception.message = %error, "could not record the url");

                send_reply(
                    client,
                    origin.channel,
                    formatted_err(RecordFailure::new(error)),
                )?;
            }
        }

        Ok(())
    }
}

#[plugin(name = "ofn", urls(any))]
impl Ofn {
    fn new(ctx: &Context) -> Result<Self, ZetaError> {
        Ok(Self {
            service: OfnService::new(ctx.db.clone()),
        })
    }

    #[handler(url)]
    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &Client,
        event: &UrlEvent,
    ) -> Result<(), ZetaError> {
        // Command invocations are handled in `run_stats` and are not recorded as history.
        if OFN.trailing_args(event.text()).is_some() {
            return Ok(());
        }

        let Some(sender) = event.sender() else {
            return Ok(());
        };

        let origin = ChannelMessageOrigin {
            channel: event.channel(),
            network: "irc.rwx.im:6697", // TODO: support multiple networks dynamically
            nickname: sender.nick,
            username: sender.username,
            hostname: sender.hostname,
        };

        self.process_url(client, &origin, event.url()).await
    }

    /// Handles explicit plugin commands (e.g., `.ofn stats`).
    #[command(
        ".ofn",
        "Show URL and YouTube repost statistics",
        args = Opts,
        label = "OFN"
    )]
    async fn run_stats(
        &self,
        _ctx: &Context,
        client: &Client,
        channel: &str,
        opts: Opts,
    ) -> Result<(), ZetaError> {
        match opts.command {
            Subcommand::Stats(_) => {
                let stats = self.service.stats().await.map_err(plugin_err)?;
                let num_urls = stats.num_urls.to_formatted_string(&Locale::en);
                let num_urls_today = stats.num_urls_today.to_formatted_string(&Locale::en);
                let num_yt_ids = stats.num_yt_ids.to_formatted_string(&Locale::en);
                let num_yt_ids_today = stats.num_yt_ids_today.to_formatted_string(&Locale::en);

                let output = format!(
                    "URLs:\x0f {num_urls}\x0310 (\x0f{num_urls_today}\x0310 today) YouTube Videos:\x0f {num_yt_ids}\x0310 (\x0f{num_yt_ids_today}\x0310 today)"
                );

                send_reply(client, channel, labeled_reply("OFN", &output))?;
            }
        }

        Ok(())
    }
}

/// Holds information about the origin of a URL, i.e., where it was posted and by whom.
#[derive(Debug)]
pub struct ChannelMessageOrigin<'a> {
    /// The name of the channel.
    channel: &'a str,
    /// The identifier of the network.
    network: &'a str,
    /// The nickname of the sender.
    nickname: &'a str,
    /// The username of the sender.
    username: &'a str,
    /// The hostname of the sender.
    hostname: &'a str,
}

impl ChannelMessageOrigin<'_> {
    /// The attribution of a record to the sender and channel of this message.
    fn attribution(&self) -> Attribution {
        Attribution::new(self.nickname, self.username, self.hostname, self.channel)
    }
}

/// Look up whether a URL has been posted before.
#[derive(Parser, Debug, PartialEq, Eq)]
pub struct Opts {
    #[command(subcommand)]
    command: Subcommand,
}

#[derive(clap::Subcommand, Debug, PartialEq, Eq)]
enum Subcommand {
    Stats(Stats),
}

/// Display database statistics
#[derive(clap::Args, Debug, PartialEq, Eq)]
pub struct Stats {}

fn formatted_err(error: impl std::fmt::Display) -> String {
    labeled_reply("OFN", format_args!("Error:\x0f {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        // clap reports contradictory definitions through asserts; evaluating them here turns a
        // bad definition into a test failure instead of a panic in debug builds.
        Opts::command().debug_assert();
    }

    #[test]
    fn record_failure_names_a_url_without_a_host() {
        let failure = RecordFailure::new(Error::InsertUrlNoHost);

        assert_eq!(
            failure.to_string(),
            "could not record the URL: it has no host"
        );
        assert!(std::error::Error::source(&failure).is_none());
    }

    #[test]
    fn record_failure_hides_database_errors() {
        let error = Error::Database(zeta_plugin::database::DbError::insert(
            "ofn records",
            sqlx::Error::RowNotFound,
        ));
        let failure = RecordFailure::new(error);

        assert_eq!(failure.to_string(), "could not record the URL");
        assert!(!failure.to_string().contains("ofn records"));
        assert!(std::error::Error::source(&failure).is_some());
    }
}
