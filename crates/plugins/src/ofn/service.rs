//! Recording and lookup of the `.ofn` URL history.

use tracing::{debug, error};
use url::Url;
use zeta_url::redact_url;

use super::{
    ChannelMessageOrigin, Recorded, Report, Resource, error::Error, model::Statistics,
    repository::OfnRepository,
};
use crate::database::Database;
use crate::youtube::{self, UrlKind};

/// Records posted URLs and reports reposts from the database history.
pub struct OfnService {
    /// The URL history repository.
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

    /// Returns statistics about the number of rows in the database.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the counts could not be queried from the database.
    pub async fn statistics(&self) -> Result<Statistics, Error> {
        self.repo.statistics().await
    }

    /// Processes the given list of `urls` with the associated `origin` by querying them from the
    /// database and inserting any of the URLs that aren't already present.
    ///
    /// Returns a [`Report`] that contains information about URLs that were already known and URLs
    /// that were added to the database.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if a lookup or insert fails.
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
    pub async fn process_urls(
        &self,
        origin: &ChannelMessageOrigin<'_>,
        urls: &[Url],
    ) -> Result<Report, Error> {
        let mut found = vec![];
        let mut num_inserted = 0;

        for url in urls {
            match self.record_url(origin, url).await? {
                Recorded::Found(resource) => found.push(resource),
                Recorded::Inserted => num_inserted += 1,
                Recorded::Failed => {}
            }
        }

        Ok(Report {
            found,
            num_inserted,
        })
    }

    /// Records `url` in the database: finding the already-known record for it, or inserting
    /// a new one — YouTube video links are tracked by video id in the dedicated table,
    /// everything else in the url records table.
    ///
    /// A failed insert is logged and reported as [`Recorded::Failed`]; a failed lookup is
    /// returned as an error.
    async fn record_url(
        &self,
        origin: &ChannelMessageOrigin<'_>,
        url: &Url,
    ) -> Result<Recorded, Error> {
        if let Some(UrlKind::Video(video_id) | UrlKind::Short(video_id)) =
            youtube::parse_youtube_url(url)
        {
            if let Some(video) = self.repo.find_youtube_video(origin, &video_id).await? {
                return Ok(Recorded::Found(Resource::YouTubeRecord(video)));
            }

            debug!(%video_id, "inserting youtube record");

            return match self.repo.insert_youtube_video(origin, &video_id).await {
                Ok(_) => {
                    debug!(?origin, %video_id, "inserted youtube record");

                    Ok(Recorded::Inserted)
                }
                Err(error) => {
                    error!("could not insert youtube record: {error}");

                    Ok(Recorded::Failed)
                }
            };
        }

        if let Some(record) = self.repo.find_url(origin, url).await? {
            return Ok(Recorded::Found(Resource::UrlRecord(record)));
        }

        match self.repo.insert_url(origin, url).await {
            Ok(_) => {
                debug!(?origin, url.full = %redact_url(url), "inserted url record");

                Ok(Recorded::Inserted)
            }
            Err(error) => {
                error!("could not insert url record: {error}");

                Ok(Recorded::Failed)
            }
        }
    }
}
