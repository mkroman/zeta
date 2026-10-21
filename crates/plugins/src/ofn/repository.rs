//! Database access for the `.ofn` URL history.

use sqlx::types::chrono::Utc;
use tracing::instrument;
use url::Url;
use zeta_url::redact_url;

use super::{
    MessageOrigin,
    error::Error,
    model::{NewUrlRecord, NewYouTubeRecord, Statistics, UrlRecord, YouTubeRecord},
};
use crate::database::Database;

/// Repository for the recorded URL and YouTube video history.
#[derive(Debug, Clone)]
pub struct OfnRepository {
    /// The database connection pool.
    db: Database,
}

impl OfnRepository {
    /// Creates a new repository backed by the given database pool.
    #[must_use]
    pub const fn new(db: Database) -> Self {
        Self { db }
    }

    /// Returns the record of `url` posted in `origin`'s channel and network, if it is known.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the record could not be fetched.
    #[instrument(
        name = "SELECT url_records",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "SELECT",
            db.collection.name = "url_records",
            db.query.summary = "SELECT url_records",
            url.full = %redact_url(url),
        )
    )]
    pub async fn find_url(
        &self,
        origin: &MessageOrigin<'_>,
        url: &Url,
    ) -> Result<Option<UrlRecord>, Error> {
        let host = url.host_str().ok_or(Error::InsertUrlNoHost)?;
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
        .fetch_optional(&self.db)
        .await
        .map_err(Error::QueryDatabase)?;

        Ok(result)
    }

    /// Returns the record of the YouTube `video_id` posted in `origin`'s channel and network, if
    /// it is known.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the record could not be fetched.
    #[instrument(
        name = "SELECT youtube_video_urls",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "SELECT",
            db.collection.name = "youtube_video_urls",
            db.query.summary = "SELECT youtube_video_urls",
        )
    )]
    pub async fn find_youtube_video(
        &self,
        origin: &MessageOrigin<'_>,
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
        .fetch_optional(&self.db)
        .await
        .map_err(Error::QueryDatabase)?;

        Ok(result)
    }

    /// Inserts `url` as posted in `origin`'s channel and network.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InsertUrlNoHost`] if the URL has no host, or [`Error::Database`] if the
    /// record could not be inserted.
    #[instrument(
        name = "INSERT url_records",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "INSERT",
            db.collection.name = "url_records",
            db.query.summary = "INSERT url_records",
            url.full = %redact_url(url),
        )
    )]
    pub async fn insert_url(
        &self,
        origin: &MessageOrigin<'_>,
        url: &Url,
    ) -> Result<NewUrlRecord, Error> {
        let host = url
            .host_str()
            .map(String::from)
            .ok_or(Error::InsertUrlNoHost)?;

        let insert = NewUrlRecord {
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
        .fetch_one(&self.db)
        .await
        .map_err(Error::InsertUrl)?;

        Ok(insert)
    }

    /// Inserts the YouTube `video_id` as posted in `origin`'s channel and network.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the record could not be inserted.
    #[instrument(
        name = "INSERT youtube_video_urls",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "INSERT",
            db.collection.name = "youtube_video_urls",
            db.query.summary = "INSERT youtube_video_urls",
        )
    )]
    pub async fn insert_youtube_video(
        &self,
        origin: &MessageOrigin<'_>,
        video_id: &str,
    ) -> Result<NewYouTubeRecord, Error> {
        let insert = NewYouTubeRecord {
            video_id: video_id.to_string(),
            nickname: origin.nickname.to_owned(),
            username: origin.username.to_owned(),
            hostname: origin.hostname.to_owned(),
            channel: origin.channel.to_owned(),
            network_id: origin.network.to_owned(),
        };

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
        .fetch_one(&self.db)
        .await
        .map_err(Error::InsertUrl)?;

        Ok(insert)
    }

    /// Returns statistics about the number of rows in the database.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the counts could not be queried.
    #[instrument(
        name = "SELECT ofn statistics",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "SELECT",
            db.collection.name = "url_records",
            db.query.summary = "SELECT ofn statistics",
        )
    )]
    pub async fn statistics(&self) -> Result<Statistics, Error> {
        let today = Utc::now().date_naive();
        let (urls, urls_today, youtube_videos, youtube_videos_today): (i64, i64, i64, i64) =
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
            .fetch_one(&self.db)
            .await
            .map_err(Error::QueryDatabase)?;

        Ok(Statistics {
            urls,
            urls_today,
            youtube_videos,
            youtube_videos_today,
        })
    }
}
