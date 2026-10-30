//! The OFN history service: recording posted URLs and answering reposts.

use sqlx::types::chrono::{DateTime, Utc};
use tracing::{debug, error};
use url::Url;

use super::{
    ChannelMessageOrigin,
    error::Error,
    model::{InsertUrlRecord, InsertYouTubeRecord, Statistics, UrlRecord, YouTubeRecord},
    repository::OfnRepository,
};
use plugins_shared::urls::youtube::{UrlKind, parse_youtube_url};
use zeta_plugin::database::Database;
use zeta_url::RedactedUrl;

/// The result of recording one URL in the database.
// The enum only lives for the duration of one URL's processing; boxing the full row in
// `Found` would only add an allocation on the common repost path.
#[allow(clippy::large_enum_variant)]
pub enum Recorded {
    /// The resource was already known.
    Found(Resource),
    /// A new record was inserted.
    Inserted,
    /// The insert failed; the error has been logged.
    Failed,
}

/// A URL history resource from the database, either a YouTube video or a generic URL record.
pub enum Resource {
    /// A YouTube video.
    YouTubeRecord(YouTubeRecord),
    /// A URL record.
    UrlRecord(UrlRecord),
}

impl Resource {
    pub(crate) const fn nickname(&self) -> &str {
        match self {
            Self::YouTubeRecord(rec) => rec.nickname.as_str(),
            Self::UrlRecord(rec) => rec.nickname.as_str(),
        }
    }

    pub(crate) const fn created_at(&self) -> DateTime<Utc> {
        match self {
            Self::YouTubeRecord(rec) => rec.created_at,
            Self::UrlRecord(rec) => rec.created_at,
        }
    }
}

/// Records posted URLs in the database and reports their history.
#[derive(Debug, Clone)]
pub struct OfnService {
    /// The repository backing the service.
    repo: OfnRepository,
}

impl OfnService {
    /// Creates a new service backed by the given database pool.
    #[must_use]
    pub const fn new(db: Database) -> Self {
        Self {
            repo: OfnRepository::new(db),
        }
    }

    /// Records `url` in the database: finding the already-known record for it, or inserting
    /// a new one — YouTube video links are tracked by video id in the dedicated table,
    /// everything else in the url records table.
    ///
    /// A failed insert is logged and reported as [`Recorded::Failed`]; a failed lookup is
    /// returned as an error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if a lookup failed, or [`Error::InsertUrlNoHost`] if the
    /// URL has no host.
    pub async fn record_url(
        &self,
        origin: &ChannelMessageOrigin<'_>,
        url: &Url,
    ) -> Result<Recorded, Error> {
        if let Some(UrlKind::Video(video_id) | UrlKind::Short(video_id)) = parse_youtube_url(url) {
            if let Some(video) = self.find_youtube_video(origin, &video_id).await? {
                return Ok(Recorded::Found(Resource::YouTubeRecord(video)));
            }

            let insert = InsertYouTubeRecord {
                video_id: video_id.clone(),
                nickname: origin.nickname.to_owned(),
                username: origin.username.to_owned(),
                hostname: origin.hostname.to_owned(),
                channel: origin.channel.to_owned(),
                network_id: origin.network.to_owned(),
            };

            debug!(media.id = %video_id, "inserting youtube record");

            return match self.repo.insert_youtube_video(&insert).await {
                Ok(()) => {
                    debug!(?origin, media.id = %video_id, "inserted youtube record");

                    Ok(Recorded::Inserted)
                }
                Err(error) => {
                    error!(
                        exception.message = %error,
                        "could not insert youtube record"
                    );

                    Ok(Recorded::Failed)
                }
            };
        }

        let insert = url_record(origin, url)?;

        if let Some(record) = self.repo.find_url(&insert).await? {
            return Ok(Recorded::Found(Resource::UrlRecord(record)));
        }

        debug!("inserting url into database");

        match self.repo.insert_url(&insert).await {
            Ok(()) => {
                debug!(url.full = %RedactedUrl::from_url(url), "inserted url record");

                Ok(Recorded::Inserted)
            }
            Err(error) => {
                error!(
                    exception.message = %error,
                    "could not insert url record"
                );

                Ok(Recorded::Failed)
            }
        }
    }

    /// Returns statistics about the number of rows in the database.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the counts could not be queried from the database.
    pub async fn stats(&self) -> Result<Statistics, Error> {
        self.repo.statistics(Utc::now().date_naive()).await
    }

    /// Finds the YouTube video record for `video_id` in the origin's channel and network.
    async fn find_youtube_video(
        &self,
        origin: &ChannelMessageOrigin<'_>,
        video_id: &str,
    ) -> Result<Option<YouTubeRecord>, Error> {
        self.repo
            .find_youtube_video(video_id, origin.channel, origin.network)
            .await
    }
}

/// Builds the url record to look up and insert for `url` and its `origin`.
///
/// # Errors
///
/// Returns [`Error::InsertUrlNoHost`] if `url` has no host.
fn url_record(origin: &ChannelMessageOrigin<'_>, url: &Url) -> Result<InsertUrlRecord, Error> {
    let host = url
        .host_str()
        .map(String::from)
        .ok_or(Error::InsertUrlNoHost)?;

    Ok(InsertUrlRecord {
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
    })
}
