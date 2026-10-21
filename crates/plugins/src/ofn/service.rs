//! Recording and lookup of the `.ofn` URL history.

use tracing::{debug, error};
use url::Url;
use zeta_url::redact_url;

use super::{
    MessageOrigin, Recorded, Resource, UrlHistoryReport, error::Error, model::Statistics,
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
    /// Returns a [`UrlHistoryReport`] that contains information about URLs that were already
    /// known and URLs that were added to the database.
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
        origin: &MessageOrigin<'_>,
        urls: &[Url],
    ) -> Result<UrlHistoryReport, Error> {
        let mut found = vec![];
        let mut inserted = 0;

        for url in urls {
            match self.record_url(origin, url).await? {
                Recorded::Found(resource) => found.push(resource),
                Recorded::Inserted => inserted += 1,
                Recorded::Failed => {}
            }
        }

        Ok(UrlHistoryReport { found, inserted })
    }

    /// Records `url` in the database: finding the already-known record for it, or inserting
    /// a new one — YouTube video links are tracked by video id in the dedicated table,
    /// everything else in the url records table.
    ///
    /// A failed insert is logged and reported as [`Recorded::Failed`]; a failed lookup is
    /// returned as an error.
    async fn record_url(&self, origin: &MessageOrigin<'_>, url: &Url) -> Result<Recorded, Error> {
        if let Some(UrlKind::Video(video_id) | UrlKind::Short(video_id)) =
            youtube::parse_youtube_url(url)
        {
            if let Some(video) = self.repo.find_youtube_video(origin, &video_id).await? {
                return Ok(Recorded::Found(Resource::YouTube(video)));
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
            return Ok(Recorded::Found(Resource::Url(record)));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The origin of a URL posted by `mk` in `channel`.
    fn origin_in(channel: &str) -> MessageOrigin<'_> {
        MessageOrigin {
            channel,
            network: "irc.example:6697",
            nickname: "mk",
            username: "~mk",
            hostname: "user.example",
        }
    }

    #[sqlx::test]
    async fn a_reposted_url_is_reported_from_the_history(db: sqlx::PgPool) {
        let service = OfnService::new(db);
        let origin = origin_in("#news");
        let url: Url = "https://example.com/article".parse().unwrap();

        let first = service
            .process_urls(&origin, std::slice::from_ref(&url))
            .await
            .unwrap();

        assert_eq!(first.inserted, 1);
        assert!(first.found.is_empty());

        let repost = service
            .process_urls(&origin, std::slice::from_ref(&url))
            .await
            .unwrap();

        assert_eq!(repost.inserted, 0);
        assert!(matches!(
            &repost.found[..],
            [Resource::Url(record)] if record.nickname == "mk"
        ));
    }

    #[sqlx::test]
    async fn youtube_links_are_tracked_by_their_video_id(db: sqlx::PgPool) {
        let service = OfnService::new(db);
        let origin = origin_in("#news");
        let watch: Url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
            .parse()
            .unwrap();
        let short: Url = "https://youtu.be/dQw4w9WgXcQ".parse().unwrap();

        let report = service
            .process_urls(&origin, std::slice::from_ref(&watch))
            .await
            .unwrap();

        assert_eq!(report.inserted, 1);
        assert!(report.found.is_empty());

        // The short link matches the watch link through the video id, not the URL.
        let repost = service
            .process_urls(&origin, std::slice::from_ref(&short))
            .await
            .unwrap();

        assert_eq!(repost.inserted, 0);
        assert!(matches!(
            &repost.found[..],
            [Resource::YouTube(record)] if record.video_id == "dQw4w9WgXcQ"
        ));
    }

    #[sqlx::test]
    async fn the_history_is_scoped_to_the_channel(db: sqlx::PgPool) {
        let service = OfnService::new(db);
        let url: Url = "https://example.com/article".parse().unwrap();
        let other_origin = origin_in("#other");
        let origin = origin_in("#news");

        let other = service
            .process_urls(&other_origin, std::slice::from_ref(&url))
            .await
            .unwrap();

        assert_eq!(other.inserted, 1);

        let same = service
            .process_urls(&origin, std::slice::from_ref(&url))
            .await
            .unwrap();

        assert_eq!(same.inserted, 1);
        assert!(same.found.is_empty());
    }

    #[sqlx::test]
    async fn statistics_count_the_recorded_urls_and_videos(db: sqlx::PgPool) {
        let service = OfnService::new(db);
        let origin = origin_in("#news");
        let urls: [Url; 3] = [
            "https://example.com/a".parse().unwrap(),
            "https://example.com/b".parse().unwrap(),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
                .parse()
                .unwrap(),
        ];

        let report = service.process_urls(&origin, &urls).await.unwrap();

        assert_eq!(report.inserted, urls.len());

        let stats = service.statistics().await.unwrap();

        assert_eq!(stats.urls, 2);
        assert_eq!(stats.youtube_videos, 1);
    }
}
