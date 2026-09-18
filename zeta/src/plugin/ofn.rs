//! Old Fucking News, aka URL post history.
//!
//! This plugin tracks URLs that are posted in channels and notifies when the URL has been posted
//! before.

mod model;

use argh::{ArgsInfo, FromArgs};
use num_format::{Locale, ToFormattedString};
use sqlx::types::chrono::{DateTime, Utc};
use tracing::{debug, error};
use url::Url;

use crate::{
    duration::TimeInWords,
    plugin::{
        ofn::model::{InsertYouTubeRecord, YouTubeRecord},
        prelude::*,
        youtube::{self, UrlKind},
    },
};
use model::{InsertUrlRecord, UrlRecord};

/// The `.ofn` command.
/// The `.ofn` command.
const OFN: CommandSpec = CommandSpec::with_args::<Opts>(
    ".ofn",
    "Show URL and YouTube repost statistics",
);

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
        let result = sqlx::query_as(
            r"SELECT * FROM url_records
            WHERE
                scheme = $1
                AND host = $2
                AND port IS NOT DISTINCT FROM $3
                AND path IS NOT DISTINCT FROM $4
                AND query IS NOT DISTINCT FROM $5
                AND channel = $6
                AND network_id = $7",
        )
        .bind(url.scheme())
        .bind(host)
        .bind(url.port_or_known_default().map(i32::from))
        .bind(url.path())
        .bind(url.query())
        .bind(origin.channel)
        .bind(origin.network)
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
        let result = sqlx::query_as(
            r"SELECT * FROM youtube_video_urls
            WHERE video_id = $1
                AND channel = $2
                AND network_id = $3",
        )
        .bind(video_id)
        .bind(origin.channel)
        .bind(origin.network)
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

        sqlx::query(
            r"INSERT INTO url_records (
                scheme,
                host,
                port,
                path,
                query,
                fragment,
                nickname,
                username,
                hostname,
                channel,
                network_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id",
        )
        .bind(&insert.scheme)
        .bind(&insert.host)
        .bind(insert.port)
        .bind(&insert.path)
        .bind(&insert.query)
        .bind(&insert.fragment)
        .bind(&insert.nickname)
        .bind(&insert.username)
        .bind(&insert.hostname)
        .bind(&insert.channel)
        .bind(&insert.network_id)
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

        sqlx::query(
            r"INSERT INTO youtube_video_urls (
                video_id,
                nickname,
                username,
                hostname,
                channel,
                network_id
            ) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        )
        .bind(&insert.video_id)
        .bind(&insert.nickname)
        .bind(&insert.username)
        .bind(&insert.hostname)
        .bind(&insert.channel)
        .bind(&insert.network_id)
        .fetch_one(&ctx.db)
        .await
        .map_err(Error::InsertUrl)?;

        Ok(insert)
    }

    /// Returns statistics about the number of rows in the database.
    pub async fn stats(&self, ctx: &Context) -> Result<Statistics, Error> {
        let today = Utc::now().date_naive();
        let (num_urls, num_urls_today, num_yt_ids, num_yt_ids_today): (i64, i64, i64, i64) =
            sqlx::query_as(
                r"SELECT
                    u.num_urls,
                    u.num_urls_today,
                    y.num_yt_ids,
                    y.num_yt_ids_today
                FROM (
                    SELECT
                        COUNT(id) AS num_urls,
                        COUNT(id) FILTER (WHERE created_at >= $1) AS num_urls_today
                    FROM url_records
                ) AS u
                CROSS JOIN (
                    SELECT
                        COUNT(id) AS num_yt_ids,
                        COUNT(id) FILTER (WHERE created_at >= $1) AS num_yt_ids_today
                    FROM youtube_video_urls
                ) AS y",
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

    /// Scrapes URLs posted in chat and updates/checks the database.
    async fn process_single_url(
        &self,
        ctx: &Context,
        client: &Client,
        origin: &ChannelMessageOrigin<'_>,
        url: Url,
    ) -> Result<(), ZetaError> {
        match self.process_urls(ctx, origin, &[url]).await {
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
                let num_urls = stats.num_urls.to_formatted_string(&Locale::en);
                let num_urls_today = stats.num_urls_today.to_formatted_string(&Locale::en);
                let num_yt_ids = stats.num_yt_ids.to_formatted_string(&Locale::en);
                let num_yt_ids_today = stats.num_yt_ids_today.to_formatted_string(&Locale::en);

                let output = format!(
                    "URLs:\x0f {num_urls}\x0310 (\x0f{num_urls_today}\x0310 today) YouTube Videos:\x0f {num_yt_ids}\x0310 (\x0f{num_yt_ids_today}\x0310 today)"
                );

                client.send_privmsg(channel, reply("OFN", &output))?;
            }
        }

        Ok(())
    }

    /// Processes the given list of `urls` with the associated `origin` by querying them from the
    /// database and inserting any of the URLs that aren't already present.
    ///
    /// URLs matching a filter are skipped — neither recorded nor announced.
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
        let filters = Filters::from_context(ctx);
        let sender = Sender::new(origin.nickname, origin.username, origin.hostname);

        let mut found = vec![];
        let mut num_inserted = 0;

        for url in urls {
            if filters.is_filtered(origin.channel, Some(sender), url) {
                debug!(%url, "skipping filtered url");

                continue;
            }

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
    type Settings = NoSettings;

    fn new(_ctx: &Context, _settings: &NoSettings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.command(OFN).url_any();

        Ok(Self::new())
    }

    async fn handle_command(
        &self,
        ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let opts = match command.parse_args::<Opts>() {
            Ok(opts) => opts,
            Err(err) => {
                reply_usage_lines(client, channel, &err, |line| reply("OFN", line))?;

                return Ok(());
            }
        };

        self.run_stats_command(ctx, client, channel, opts).await
    }

    async fn handle_url(
        &self,
        ctx: &Context,
        client: &Client,
        event: &UrlEvent,
    ) -> Result<(), ZetaError> {
        // Command invocations are handled in `handle_command` and are not recorded as history.
        if OFN.parse(event.text()).is_some() {
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

        self.process_single_url(ctx, client, &origin, event.url().clone())
            .await
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

/// Look up whether a URL has been posted before.
#[derive(FromArgs, ArgsInfo, Debug, PartialEq, Eq)]
pub struct Opts {
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

fn formatted_err(s: &str) -> String {
    reply("OFN", format!("Error:\x0f {s}"))
}
