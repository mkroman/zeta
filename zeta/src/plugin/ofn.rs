//! Old Fucking News, aka URL post history.
//!
//! This plugin tracks URLs that are posted in channels and notifies when the URL has been posted
//! before.

mod model;

use argh::FromArgs;
use irc::client::prelude::Prefix as IrcPrefix;
use num_format::{Locale, ToFormattedString};
use sqlx::types::chrono::{DateTime, Utc};
use tracing::{debug, error};
use url::Url;

use crate::{
    plugin::{
        ofn::model::{InsertYouTubeRecord, YouTubeRecord},
        prelude::*,
        youtube::{self, UrlKind},
    },
    url::ExtractUrlsExt,
};
use model::{InsertUrlRecord, UrlRecord};

/// The `.ofn` command.
const OFN: Prefix = Prefix::new(".ofn");

pub struct Ofn;

impl Ofn {
    pub const fn new() -> Ofn {
        Ofn
    }

    /// Find and return a [`UrlRecord`] for the given `url` and associated `origin` if present in
    /// the database.
    ///
    /// Returns `Ok(None)` if not present.
    #[tracing::instrument(
        skip_all,
        err,
        fields(url.full = %url)
    )]
    async fn find_url(
        &self,
        ctx: &Context,
        origin: &ChannelMessageOrigin<'_>,
        url: &Url,
    ) -> Result<Option<UrlRecord>, Error> {
        let host = url.host_str().ok_or_else(|| Error::InsertUrlNoHost)?;
        let result = sqlx::query_file_as!(
            UrlRecord,
            "queries/find_url_record.sql",
            url.scheme(),
            host,
            url.port_or_known_default().map(i32::from),
            url.path(),
            url.query(),
            origin.channel,
            origin.network
        )
        .fetch_optional(&ctx.db)
        .await
        .map_err(Error::QueryDatabase)?;

        Ok(result)
    }

    /// Find and return a [`YouTubeRecord`] for the given `video_id` and associated `origin` if
    /// present in the database.
    ///
    /// Returns `Ok(None)` if not present.
    async fn find_youtube_video(
        &self,
        ctx: &Context,
        origin: &ChannelMessageOrigin<'_>,
        video_id: &str,
    ) -> Result<Option<YouTubeRecord>, Error> {
        let result = sqlx::query_file_as!(
            YouTubeRecord,
            "queries/find_youtube_video_url.sql",
            video_id,
            origin.channel,
            origin.network
        )
        .fetch_optional(&ctx.db)
        .await
        .map_err(Error::QueryDatabase)?;

        Ok(result)
    }

    /// Inserts the given `url` into the database with the associated `origin`.
    ///
    /// Returns the [`InsertUrlRecord`] used for the operation if the insert was successful.
    #[tracing::instrument(
        skip_all,
        err,
        fields(url.full = %url)
    )]
    async fn insert_url(
        &self,
        ctx: &Context,
        origin: &ChannelMessageOrigin<'_>,
        url: &Url,
    ) -> Result<InsertUrlRecord, Error> {
        let host = url
            .host_str()
            .map(String::from)
            .ok_or_else(|| Error::InsertUrlNoHost)?;

        let insert = InsertUrlRecord {
            scheme: url.scheme().to_owned(),
            host,
            port: url.port_or_known_default().map(i32::from),
            path: url.path().to_owned(),
            query: url.query().map(String::from),
            fragment: url.fragment().map(String::from),
            nickname: origin.nickname.to_owned(),
            username: origin.username.to_owned(),
            hostname: origin.hostname.to_owned(),
            channel: origin.channel.to_owned(),
            network_id: origin.network.to_owned(),
        };

        debug!("inserting url into database");

        sqlx::query_file!(
            "queries/insert_url_record.sql",
            insert.scheme,
            insert.host,
            insert.port,
            insert.path,
            insert.query,
            insert.fragment,
            insert.nickname,
            insert.username,
            insert.hostname,
            insert.channel,
            insert.network_id
        )
        .fetch_one(&ctx.db)
        .await
        .map_err(Error::InsertUrl)?;

        Ok(insert)
    }

    /// Inserts the given YouTube `video_id` into the database with the associated `origin`.
    ///
    /// Returns the [`InsertYouTubeRecord`] used for the operation if the insert was succesful.
    #[tracing::instrument(skip(self, ctx, origin), err)]
    async fn insert_youtube_video(
        &self,
        ctx: &Context,
        origin: &ChannelMessageOrigin<'_>,
        video_id: &str,
    ) -> Result<InsertYouTubeRecord, Error> {
        let insert = InsertYouTubeRecord {
            video_id: video_id.to_string(),
            nickname: origin.nickname.to_owned(),
            username: origin.username.to_owned(),
            hostname: origin.hostname.to_owned(),
            channel: origin.channel.to_owned(),
            network_id: origin.network.to_owned(),
        };

        debug!("inserting youtube video into database");

        sqlx::query_file!(
            "queries/insert_youtube_video_url.sql",
            insert.video_id,
            insert.nickname,
            insert.username,
            insert.hostname,
            insert.channel,
            insert.network_id
        )
        .fetch_one(&ctx.db)
        .await
        .map_err(Error::InsertUrl)?;

        Ok(insert)
    }

    /// Returns statistics about the number of rows in the database.
    pub async fn stats(&self, ctx: &Context) -> Result<Statistics, Error> {
        let today = Utc::now().date_naive();
        let (num_urls, num_urls_today): (i64, i64) = sqlx::query_as(
            "SELECT 
                COUNT(id) AS num_urls,
                COUNT(id) FILTER (WHERE created_at >= $1) AS num_urls_today
            FROM url_records",
        )
        .bind(today)
        .fetch_one(&ctx.db)
        .await
        .map_err(Error::QueryDatabase)?;

        let (num_yt_ids, num_yt_ids_today): (i64, i64) = sqlx::query_as(
            "SELECT 
                COUNT(id) AS num_urls,
                COUNT(id) FILTER (WHERE created_at >= $1) AS num_urls_today
            FROM youtube_video_urls",
        )
        .bind(today)
        .fetch_one(&ctx.db)
        .await
        .map_err(Error::QueryDatabase)?;

        Ok(Statistics {
            num_urls,
            num_urls_today,
            num_yt_ids,
            num_yt_ids_today,
        })
    }

    /// Scrapes standard chat messages for URLs and updates/checks the database.
    async fn handle_urls(
        &self,
        ctx: &Context,
        client: &Client,
        origin: &ChannelMessageOrigin<'_>,
        msg: &str,
    ) -> Result<(), ZetaError> {
        let urls: Vec<Url> = msg.urls().collect();
        if urls.is_empty() {
            return Ok(());
        }

        match self.process_urls(ctx, origin, &urls).await {
            Ok(report) => {
                for resource in report.found {
                    let time_ago = distance_of_time_in_words(resource.created_at());

                    client.send_privmsg(
                        origin.channel,
                        format!(
                            "{}: OFN - posted by {} {time_ago}",
                            origin.nickname,
                            resource.nickname()
                        ),
                    )?;
                }

                if report.num_inserted > 0 {
                    debug!("inserted {} new urls", report.num_inserted);
                }
            }
            Err(error) => {
                client.send_privmsg(origin.channel, formatted_err(&error.to_string()))?;
            }
        }

        Ok(())
    }

    /// Handles explicit plugin commands (e.g., `.ofn stats`).
    async fn run_stats_command(
        &self,
        ctx: &Context,
        client: &Client,
        channel: &str,
        opts: Opts,
    ) -> Result<(), ZetaError> {
        match opts.command {
            Subcommand::Stats(_) => {
                let stats = self.stats(ctx).await.map_err(plugin_err)?;
                let output = format!(
                    "URLs:\x0f {}\x0310 YouTube Videos:\x0f {}\x0310 Recorded today:\x0f {}\x0310/\x0f{}",
                    stats.num_urls.to_formatted_string(&Locale::en),
                    stats.num_yt_ids.to_formatted_string(&Locale::en),
                    stats.num_urls_today.to_formatted_string(&Locale::en),
                    stats.num_yt_ids_today.to_formatted_string(&Locale::en)
                );

                client.send_privmsg(channel, formatted(&output))?;
            }
        }

        Ok(())
    }

    /// Processes the given list of `urls` with the associated `origin` by querying them from the
    /// database and inserting any of the URLs that aren't already present.
    ///
    /// Returns a [`Report`] that contains information about URLs that were already known and URLs
    /// that were added to the database.
    #[tracing::instrument(
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
    async fn process_urls(
        &self,
        ctx: &Context,
        origin: &ChannelMessageOrigin<'_>,
        urls: &[Url],
    ) -> Result<Report, Error> {
        let mut found = vec![];
        let mut num_inserted = 0;

        for url in urls {
            if let Some(UrlKind::Video(video_id) | UrlKind::Short(video_id)) =
                youtube::parse_youtube_url(url)
            {
                if let Some(video) = self.find_youtube_video(ctx, origin, &video_id).await? {
                    found.push(Resource::YouTubeRecord(video));
                } else {
                    debug!(%video_id, "inserting youtube record");

                    match self.insert_youtube_video(ctx, origin, &video_id).await {
                        Ok(_record) => {
                            debug!(?origin, %video_id, "inserted youtube record");
                            num_inserted += 1;
                        }
                        Err(error) => {
                            error!("could not insert youtube record: {error}");
                        }
                    }
                }
            } else {
                if let Some(record) = self.find_url(ctx, origin, url).await? {
                    found.push(Resource::UrlRecord(record));
                } else {
                    match self.insert_url(ctx, origin, url).await {
                        Ok(_) => {
                            debug!(?origin, ?url, "inserted url record");
                            num_inserted += 1;
                        }
                        Err(error) => {
                            error!("could not insert url record: {error}");
                        }
                    }
                }
            }
        }

        Ok(Report {
            found,
            num_inserted,
        })
    }
}

#[async_trait]
impl Plugin<Context> for Ofn {
    fn new(_ctx: &Context) -> Result<Self, ZetaError> {
        Ok(Self::new())
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "ofn".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn commands(&self) -> &'static [Prefix] {
        &[OFN]
    }

    async fn handle_command(
        &self,
        ctx: &Context,
        client: &Client,
        channel: &str,
        command: &Prefix,
        args: &str,
    ) -> Result<(), ZetaError> {
        let opts = match command.parse_args::<Opts>(args) {
            Ok(opts) => opts,
            Err(err) => {
                for line in err.to_string().lines().filter(|s| !s.is_empty()) {
                    client.send_privmsg(channel, formatted(line))?;
                }
                return Ok(());
            }
        };

        self.run_stats_command(ctx, client, channel, opts).await
    }

    async fn handle_message(
        &self,
        ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        let Command::PRIVMSG(channel, msg) = &message.command else {
            return Ok(());
        };

        let Some(IrcPrefix::Nickname(nickname, username, hostname)) = &message.prefix else {
            return Ok(());
        };

        if self
            .commands()
            .iter()
            .any(|command| command.parse(msg).is_some())
        {
            self.dispatch_command(ctx, client, message).await
        } else {
            let origin = ChannelMessageOrigin {
                channel,
                network: "irc.rwx.im:6697", // TODO: support multiple networks dynamically
                nickname,
                username,
                hostname,
            };

            self.handle_urls(ctx, client, &origin, msg).await
        }
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

/// Statistics for database rows.
#[derive(Debug)]
#[allow(clippy::struct_field_names)]
pub struct Statistics {
    /// The number of URLs in total.
    pub num_urls: i64,
    /// The number of URLs that were added today.
    pub num_urls_today: i64,
    /// The number of YouTube videos in total.
    pub num_yt_ids: i64,
    /// The number of YouTube videos that have been added today.
    pub num_yt_ids_today: i64,
}

/// Old Fucking News (URL history)
#[derive(FromArgs, Debug, PartialEq, Eq)]
pub struct Opts {
    #[argh(subcommand)]
    command: Subcommand,
}

#[derive(FromArgs, Debug, PartialEq, Eq)]
#[argh(subcommand)]
enum Subcommand {
    Stats(Stats),
}

/// Display database statistics
#[derive(FromArgs, Debug, PartialEq, Eq)]
#[argh(subcommand, name = "stats")]
pub struct Stats {}

pub struct Report {
    /// List of URL records that were found in the database.
    found: Vec<Resource>,
    /// Number of resources that were added to the database.
    num_inserted: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not query database: {0}")]
    QueryDatabase(#[source] sqlx::Error),
    #[error("could not insert url in database: {0}")]
    InsertUrl(#[source] sqlx::Error),
    #[error("can't insert url with no host")]
    InsertUrlNoHost,
}

pub enum Resource {
    /// A YouTube video.
    YouTubeRecord(YouTubeRecord),
    /// A URL record.
    UrlRecord(UrlRecord),
}

impl Resource {
    const fn nickname(&self) -> &str {
        match self {
            Resource::YouTubeRecord(rec) => rec.nickname.as_str(),
            Resource::UrlRecord(rec) => rec.nickname.as_str(),
        }
    }

    const fn created_at(&self) -> DateTime<Utc> {
        match self {
            Resource::YouTubeRecord(rec) => rec.created_at,
            Resource::UrlRecord(rec) => rec.created_at,
        }
    }
}

fn formatted(s: &str) -> String {
    format!("\x0310>\x0f\x02 OFN\x02\x0310: {s}")
}

fn formatted_err(s: &str) -> String {
    formatted(&format!("Error:\x0f {s}"))
}

fn distance_of_time_in_words(delta: DateTime<Utc>) -> String {
    let now = Utc::now();
    let total_seconds = (now - delta).num_seconds();

    // Handle zero or negative durations
    if total_seconds <= 0 {
        return "0 minutes".to_string();
    }

    // Calculate time units
    let weeks = total_seconds / (7 * 24 * 60 * 60);
    let remaining_after_weeks = total_seconds % (7 * 24 * 60 * 60);
    let days = remaining_after_weeks / (24 * 60 * 60);
    let remaining_after_days = remaining_after_weeks % (24 * 60 * 60);
    let hours = remaining_after_days / (60 * 60);
    let remaining_after_hours = remaining_after_days % (60 * 60);
    let minutes = remaining_after_hours / 60;
    let remaining_after_mins = remaining_after_days % (60 * 60);
    let seconds = remaining_after_mins % 60;

    // Build the parts vector with non-zero units
    let mut parts = Vec::new();

    if weeks > 0 {
        parts.push(format!(
            "{} week{}",
            weeks,
            if weeks == 1 { "" } else { "s" }
        ));
    }
    if days > 0 {
        parts.push(format!("{} day{}", days, if days == 1 { "" } else { "s" }));
    }

    if hours > 0 {
        parts.push(format!(
            "{} hour{}",
            hours,
            if hours == 1 { "" } else { "s" }
        ));
    }

    if minutes > 0 {
        parts.push(format!(
            "{} minute{}",
            minutes,
            if minutes == 1 { "" } else { "s" }
        ));
    }

    if seconds > 0 {
        parts.push(format!(
            "{} second{}",
            seconds,
            if seconds == 1 { "" } else { "s" }
        ));
    }

    // Format the output with proper grammar
    match parts.len() {
        0 => "0 minutes ago".to_string(),
        1 => format!("{} ago", parts[0].clone()),
        2 => format!("{} and {} ago", parts[0], parts[1]),
        _ => {
            let last = parts.pop().unwrap();
            format!("{}, and {} ago", parts.join(", "), last)
        }
    }
}
