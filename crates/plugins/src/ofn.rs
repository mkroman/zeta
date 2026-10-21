//! Old Fucking News — tracks posted URLs and calls out reposts.
//!
//! Every URL posted in a channel is recorded in the database (scoped to the channel and
//! network) and checked against the history; a repost is replied to with when and by whom it
//! was previously posted. YouTube videos are additionally tracked by their video id in
//! a dedicated table, so reposts match across `watch?v=`, `shorts/`, and `youtu.be` links.
//!
//! URLs arrive already filtered by the host dispatcher, and `.ofn` command invocations are
//! skipped.
//!
//! The `.ofn stats` command shows aggregate statistics: the number of recorded URLs and
//! YouTube videos in total, and how many were added today.
//!
//! The plugin requires the `database` feature and has no settings.

mod error;
mod model;
mod repository;
mod service;

// The module types are re-exported as part of the module's API surface, even though the plugin
// itself only handles them by value.
#[allow(unused_imports)]
pub use error::Error;
pub use model::Statistics;
pub use service::OfnService;

use argh::{ArgsInfo, FromArgs};
use num_format::{Locale, ToFormattedString};
use sqlx::types::chrono::{DateTime, Utc};
use tracing::debug;
use url::Url;

use crate::prelude::*;
use zeta_utils::duration::TimeInWords;

use model::{UrlRecord, YouTubeRecord};

/// The `.ofn` command.
const OFN: CommandSpec =
    CommandSpec::with_args::<Options>(".ofn", "Show URL and YouTube repost statistics");

/// The ofn plugin: records posted URLs and reports reposts.
pub struct Ofn {
    /// The URL history service.
    service: OfnService,
}

/// The result of recording one URL in the database.
// The enum only lives for the duration of one URL's processing; boxing the full row in
// `Found` would only add an allocation on the common repost path.
#[allow(clippy::large_enum_variant)]
enum Recorded {
    /// The resource was already known.
    Found(Resource),
    /// A new record was inserted.
    Inserted,
    /// The insert failed; the error has been logged.
    Failed,
}

impl Ofn {
    /// Scrapes URLs posted in chat and updates/checks the database.
    async fn process_url(
        &self,
        client: &IrcClient,
        origin: &MessageOrigin<'_>,
        url: &Url,
    ) -> Result<(), PluginError> {
        match self
            .service
            .process_urls(origin, std::slice::from_ref(url))
            .await
        {
            Ok(report) => {
                for resource in report.found {
                    let time_ago = resource.created_at().time_ago();

                    client.send_privmsg(
                        origin.channel,
                        format!(
                            "{}: OFN - posted by {} {time_ago}",
                            origin.nickname,
                            resource.nickname()
                        ),
                    )?;
                }

                if report.inserted > 0 {
                    debug!("inserted {} new urls", report.inserted);
                }
            }
            Err(error) => {
                client.send_privmsg(origin.channel, format_error(&error.to_string()))?;
            }
        }

        Ok(())
    }

    /// Handles explicit plugin commands (e.g., `.ofn stats`).
    async fn run_stats_command(
        &self,
        client: &IrcClient,
        channel: &str,
        opts: Options,
    ) -> Result<(), PluginError> {
        match opts.command {
            Subcommand::Stats(_) => {
                let stats = self.service.statistics().await.map_err(plugin_err)?;
                let urls = stats.urls.to_formatted_string(&Locale::en);
                let urls_today = stats.urls_today.to_formatted_string(&Locale::en);
                let youtube_videos = stats.youtube_videos.to_formatted_string(&Locale::en);
                let youtube_videos_today =
                    stats.youtube_videos_today.to_formatted_string(&Locale::en);

                let output = format!(
                    "URLs:\x0f {urls}\x0310 (\x0f{urls_today}\x0310 today) YouTube Videos:\x0f {youtube_videos}\x0310 (\x0f{youtube_videos_today}\x0310 today)"
                );

                client.send_privmsg(channel, reply("OFN", &output))?;
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Plugin<Context> for Ofn {
    type Settings = NoSettings;

    fn new(
        ctx: &Context,
        _settings: &NoSettings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, PluginError> {
        subscriptions
            .subscribe_command(OFN)
            .subscribe_urls(UrlScope::Any);

        Ok(Self {
            service: OfnService::new(ctx.db.clone()),
        })
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &IrcClient,
        command: &CommandEvent,
    ) -> Result<(), PluginError> {
        let channel = command.channel();
        let opts = match command.parse_args::<Options>() {
            Ok(opts) => opts,
            Err(err) => {
                reply_usage_lines(client, channel, &err, |line| reply("OFN", line))?;

                return Ok(());
            }
        };

        self.run_stats_command(client, channel, opts).await
    }

    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &IrcClient,
        event: &UrlEvent,
    ) -> Result<(), PluginError> {
        // Command invocations are handled in `handle_command` and are not recorded as history.
        if OFN.arguments_from(event.text()).is_some() {
            return Ok(());
        }

        let Some(sender) = event.sender() else {
            return Ok(());
        };

        let origin = MessageOrigin {
            channel: event.channel(),
            network: "irc.rwx.im:6697", // TODO: support multiple networks dynamically
            nickname: sender.nick,
            username: sender.username,
            hostname: sender.hostname,
        };

        self.process_url(client, &origin, event.url()).await
    }
}

/// Holds information about the origin of a URL, i.e., where it was posted and by whom.
#[derive(Debug)]
pub struct MessageOrigin<'a> {
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

/// Look up whether a URL has been posted before.
#[derive(FromArgs, ArgsInfo, Debug, PartialEq, Eq)]
pub struct Options {
    #[argh(subcommand)]
    command: Subcommand,
}

#[derive(FromArgs, ArgsInfo, Debug, PartialEq, Eq)]
#[argh(subcommand)]
enum Subcommand {
    Stats(Stats),
}

/// Display database statistics
#[derive(FromArgs, ArgsInfo, Debug, PartialEq, Eq)]
#[argh(subcommand, name = "stats")]
pub struct Stats {}

/// The result of processing a batch of URLs: prior records found and how many were inserted.
pub struct UrlHistoryReport {
    /// List of URL records that were found in the database.
    found: Vec<Resource>,
    /// Number of resources that were added to the database.
    inserted: usize,
}

/// A URL history resource from the database, either a YouTube video or a generic URL record.
pub enum Resource {
    /// A YouTube video.
    YouTube(YouTubeRecord),
    /// A URL record.
    Url(UrlRecord),
}

impl Resource {
    const fn nickname(&self) -> &str {
        match self {
            Self::YouTube(rec) => rec.nickname.as_str(),
            Self::Url(rec) => rec.nickname.as_str(),
        }
    }

    const fn created_at(&self) -> DateTime<Utc> {
        match self {
            Self::YouTube(rec) => rec.created_at,
            Self::Url(rec) => rec.created_at,
        }
    }
}

fn format_error(s: &str) -> String {
    reply("OFN", format!("Error:\x0f {s}"))
}
