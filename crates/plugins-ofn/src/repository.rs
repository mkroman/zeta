//! Database access for the OFN URL history.

use sqlx::types::chrono::NaiveDate;
use tracing::trace;

use super::{
    error::Error,
    model::{InsertUrlRecord, InsertYouTubeRecord, Statistics, UrlRecord, YouTubeRecord},
};
use zeta_plugin::instrument_database;

zeta_plugin::impl_repository! {
    /// Repository for storing and retrieving the posted URLs and YouTube videos.
    OfnRepository
}

impl OfnRepository {
    /// Returns the URL record matching `record`, if it has been posted before.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the URL record could not be fetched.
    #[instrument_database(operation = "SELECT", collection = "url_records")]
    pub async fn find_url(&self, record: &InsertUrlRecord) -> Result<Option<UrlRecord>, Error> {
        trace!(host = %record.host, "loading url record from database");

        sqlx::query_as(
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
        .bind(&record.scheme)
        .bind(&record.host)
        .bind(record.port)
        .bind(&record.path)
        .bind(&record.query)
        .bind(&record.channel)
        .bind(&record.network_id)
        .fetch_optional(&self.db)
        .await
        .map_err(Error::load)
    }

    /// Returns the YouTube video record for `video_id` in `channel` on `network_id`, if it has
    /// been posted before.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the video record could not be fetched.
    #[instrument_database(operation = "SELECT", collection = "youtube_video_urls")]
    pub async fn find_youtube_video(
        &self,
        video_id: &str,
        channel: &str,
        network_id: &str,
    ) -> Result<Option<YouTubeRecord>, Error> {
        trace!(
            video_id,
            channel, "loading youtube video record from database"
        );

        sqlx::query_as(
            r"SELECT * FROM youtube_video_urls
            WHERE video_id = $1
                AND channel = $2
                AND network_id = $3",
        )
        .bind(video_id)
        .bind(channel)
        .bind(network_id)
        .fetch_optional(&self.db)
        .await
        .map_err(Error::load)
    }

    /// Inserts `record` into the url records table.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the record could not be inserted.
    #[instrument_database(operation = "INSERT", collection = "url_records")]
    pub async fn insert_url(&self, record: &InsertUrlRecord) -> Result<(), Error> {
        trace!("inserting url record into database");

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
        .bind(&record.scheme)
        .bind(&record.host)
        .bind(record.port)
        .bind(&record.path)
        .bind(&record.query)
        .bind(&record.fragment)
        .bind(&record.nickname)
        .bind(&record.username)
        .bind(&record.hostname)
        .bind(&record.channel)
        .bind(&record.network_id)
        .fetch_one(&self.db)
        .await
        .map_err(Error::insert)?;

        Ok(())
    }

    /// Inserts `record` into the YouTube video urls table.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the record could not be inserted.
    #[instrument_database(operation = "INSERT", collection = "youtube_video_urls")]
    pub async fn insert_youtube_video(&self, record: &InsertYouTubeRecord) -> Result<(), Error> {
        trace!(
            video_id = %record.video_id,
            "inserting youtube video record into database"
        );

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
        .bind(&record.video_id)
        .bind(&record.nickname)
        .bind(&record.username)
        .bind(&record.hostname)
        .bind(&record.channel)
        .bind(&record.network_id)
        .fetch_one(&self.db)
        .await
        .map_err(Error::insert)?;

        Ok(())
    }

    /// Returns statistics about the number of rows in the database.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the counts could not be queried.
    #[instrument_database(
        operation = "SELECT",
        collection = "url_records",
        summary = "SELECT ofn statistics"
    )]
    pub async fn statistics(&self, today: NaiveDate) -> Result<Statistics, Error> {
        trace!(%today, "loading ofn statistics from database");

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
            .fetch_one(&self.db)
            .await
            .map_err(Error::load)?;

        Ok(Statistics {
            num_urls,
            num_urls_today,
            num_yt_ids,
            num_yt_ids_today,
        })
    }
}
