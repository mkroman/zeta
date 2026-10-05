//! Unwall service: the covered-site set, the URL cache, and the submissions.

use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use sqlx::types::chrono::Utc;
use tracing::{debug, instrument, warn};
use url::Url;

use super::{
    client::UnwallClient,
    error::Error,
    model::{CachedUrl, HostStatistics, NewFetch, Site, Statistics},
    repository::UnwallRepository,
    sites,
};
use crate::{cache::TtlMap, database::Database};

/// How long a submitted article stays in the in-flight cache, collapsing a burst of the same
/// article into a single submission.
const INFLIGHT_TTL: Duration = Duration::from_mins(10);

/// Stores the covered-site set in memory, caches unwalled URLs in the database, and submits
/// articles to unwall.app.
pub struct UnwallService {
    /// The unwall repository.
    repo: UnwallRepository,
    /// The unwall API client.
    client: UnwallClient,
    /// The base URL of the unwall reader, which the replies link to.
    reader_base: Url,
    /// The in-memory covered-site set, the sites an admin added with `.unwall add`.
    ///
    /// The database is the source of truth, loaded on [`load`](UnwallService::load) and
    /// reloaded after every change; the hot lookup path ([`covers`](UnwallService::covers))
    /// only ever touches this set.
    covered: RwLock<Vec<Site>>,
    /// Collapses a burst of the same article into a single submission.
    inflight: TtlMap<String, CachedUrl>,
}

impl UnwallService {
    /// Creates a new service backed by the given database pool and API client, covering nothing
    /// until [`load`](Self::load) has read the added sites.
    #[must_use]
    pub fn new(db: Database, client: UnwallClient, reader_base: Url) -> Self {
        Self {
            repo: UnwallRepository::new(db),
            client,
            reader_base,
            covered: RwLock::new(Vec::new()),
            inflight: TtlMap::new(INFLIGHT_TTL),
        }
    }

    /// Loads the covered sites from the database into the in-memory set.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the sites could not be fetched.
    #[instrument(skip_all, err)]
    pub async fn load(&self) -> Result<(), Error> {
        let covered = self.repo.sites().await?;

        debug!(count = covered.len(), "loaded unwall sites");

        *self.lock_covered_mut() = covered;

        Ok(())
    }

    /// Whether `host` is covered.
    #[must_use]
    pub fn covers(&self, host: &str) -> bool {
        let host = host.to_lowercase();

        self.lock_covered()
            .iter()
            .any(|site| sites::matches_site(&host, &site.host))
    }

    /// Returns the unwall.app reader link for `article`: the article with the reader base as
    /// its scheme and authority, its fragment dropped.
    #[must_use]
    pub fn reader_url(&self, article: &Url) -> Option<Url> {
        let host = article.host_str()?;
        let mut link = self
            .reader_base
            .join(&format!("/{host}{}", article.path()))
            .ok()?;

        link.set_query(article.query());

        Some(link)
    }

    /// Returns the cached reader link for `article`, submitting it otherwise.
    ///
    /// A first submission is single-flight per URL — a burst of the same article submits once
    /// — and attributed to the user who posted the link, whether or not the submission
    /// succeeds. The database cache is consulted before the submission, so every later post is
    /// answered without touching the API.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the cache could not be consulted or written, and the
    /// errors of `UnwallClient::submit` otherwise.
    ///
    /// # Panics
    ///
    /// Panics if the database reports a submission cached that a re-read cannot find — a
    /// write that vanished, which would mean the database broke its read-your-writes
    /// guarantee.
    pub async fn resolve(&self, article: &Url, fetch: NewFetch) -> Result<CachedUrl, Error> {
        let key = fetch.url.clone();

        if let Some(cached) = self.repo.cached_url(&key).await? {
            return Ok(cached);
        }

        let unwall_url = self.reader_url(article).ok_or(Error::InvalidUrl)?;

        self.inflight
            .get_or_refresh(key, || async {
                let submitted = self.client.submit(article).await;

                // The fetch was triggered either way; a failed attribution is logged, never
                // fatal to the submission.
                if let Err(error) = self.repo.record_fetch(&fetch).await {
                    warn!(url.full = %fetch.url, %error, "could not record the unwall fetch");
                }

                submitted?;

                self.repo
                    .insert_url(&fetch.url, &fetch.host, unwall_url.as_str())
                    .await?;

                Ok(Some(
                    self.repo
                        .cached_url(&fetch.url)
                        .await?
                        .ok_or(Error::InvalidUrl)?,
                ))
            })
            .await
            .map(|cached| cached.expect("a successful refresh caches the row"))
    }

    /// Adds `hosts` as covered sites.
    ///
    /// The hosts are expected normalized (see `sites::normalize_site`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the sites could not be inserted or reloaded.
    #[instrument(skip_all, err)]
    pub async fn add_sites(&self, hosts: &[String], nickname: &str) -> Result<(), Error> {
        for host in hosts {
            self.repo.insert_site(host, nickname).await?;
        }

        self.load().await
    }

    /// Removes every covered site matching `pattern`.
    ///
    /// Returns the hosts removed, sorted.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the sites could not be deleted or reloaded.
    #[instrument(skip_all, err, fields(pattern = %pattern))]
    pub async fn remove_matching(&self, pattern: &str) -> Result<Vec<String>, Error> {
        let mut affected: Vec<String> = self
            .lock_covered()
            .iter()
            .filter(|site| sites::matches_site(&site.host, pattern))
            .map(|site| site.host.clone())
            .collect();

        if affected.is_empty() {
            return Ok(Vec::new());
        }

        affected.sort();

        self.repo.delete_sites(&affected).await?;
        self.load().await?;

        Ok(affected)
    }

    /// Returns a snapshot of the covered sites, ordered by host.
    #[must_use]
    pub fn sites(&self) -> Vec<Site> {
        self.lock_covered().clone()
    }

    /// Returns the overall statistics of the unwalled URLs and fetches.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the statistics could not be queried.
    #[instrument(skip_all, err)]
    pub async fn statistics(&self) -> Result<Statistics, Error> {
        self.repo.statistics(Utc::now().date_naive()).await
    }

    /// Returns the statistics for `host`, or [`None`] when nothing has been unwalled for it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the statistics could not be queried.
    #[instrument(skip_all, err)]
    pub async fn host_statistics(&self, host: &str) -> Result<Option<HostStatistics>, Error> {
        self.repo
            .host_statistics(&host.to_lowercase(), Utc::now().date_naive())
            .await
    }

    /// Locks the covered sites for reading, continuing on poisoning.
    fn lock_covered(&self) -> RwLockReadGuard<'_, Vec<Site>> {
        crate::sync::read(&self.covered)
    }

    /// Locks the covered sites for writing, continuing on poisoning.
    fn lock_covered_mut(&self) -> RwLockWriteGuard<'_, Vec<Site>> {
        crate::sync::write(&self.covered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HttpConfig;
    use crate::http;
    use zeta_test_support::wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    /// A service against an empty database and an unreachable API, for the paths that never
    /// submit.
    fn db_service(db: Database) -> UnwallService {
        let client = UnwallClient::new(
            http::build_client(&HttpConfig::default()),
            Duration::from_secs(60),
            "http://127.0.0.1:9".parse().unwrap(),
        );

        UnwallService::new(db, client, "https://unwall.app/".parse().unwrap())
    }

    /// The attribution of a fetch posted by `mk`.
    fn fetch(url: &Url) -> NewFetch {
        NewFetch {
            url: url.as_str().to_owned(),
            host: url.host_str().unwrap_or_default().to_owned(),
            nickname: "mk".into(),
            username: "~mk".into(),
            hostname: "user.example".into(),
            channel: "#news".into(),
            network_id: "irc.rwx.im:6697".into(),
        }
    }

    #[sqlx::test]
    async fn nothing_is_covered_before_an_admin_adds_a_site(db: sqlx::PgPool) {
        let service = db_service(db);
        service.load().await.unwrap();

        // Neither a site unwall.app tests nor any other host is covered out of the box.
        assert!(!service.covers("ft.com"));
        assert!(!service.covers("www.ft.com"));
        assert!(!service.covers("example.com"));
        assert!(service.sites().is_empty());
    }

    #[sqlx::test]
    async fn added_sites_cover_their_host_www_variant_and_wildcards(db: sqlx::PgPool) {
        let service = db_service(db);
        service.load().await.unwrap();

        service
            .add_sites(&["bloomberg.com".to_owned()], "admin")
            .await
            .unwrap();

        assert!(service.covers("bloomberg.com"));
        assert!(service.covers("www.bloomberg.com"));
        assert!(!service.covers("api.bloomberg.com"));

        service
            .add_sites(&["*.bloomberg.com".to_owned()], "admin")
            .await
            .unwrap();

        assert!(service.covers("api.bloomberg.com"));

        // The added sites survive a fresh load: the database is the source of truth.
        service.load().await.unwrap();

        assert!(service.covers("api.bloomberg.com"));
    }

    #[sqlx::test]
    async fn removing_matches_the_added_sites_and_survives_a_reload(db: sqlx::PgPool) {
        let service = db_service(db);
        service.load().await.unwrap();

        service
            .add_sites(&["bloomberg.com".to_owned(), "*.bloomberg.com".to_owned()], "admin")
            .await
            .unwrap();

        let removed = service.remove_matching("*.bloomberg.com").await.unwrap();

        assert_eq!(removed, ["*.bloomberg.com", "bloomberg.com"]);
        assert!(!service.covers("www.bloomberg.com"));
        assert!(!service.covers("bloomberg.com"));
        assert!(service.sites().is_empty());

        // The removal survives a fresh load, and re-adding brings the host back.
        service.load().await.unwrap();

        assert!(!service.covers("bloomberg.com"));

        service
            .add_sites(&["bloomberg.com".to_owned()], "admin")
            .await
            .unwrap();

        assert!(service.covers("bloomberg.com"));
    }

    #[sqlx::test]
    async fn removing_without_a_match_removes_nothing(db: sqlx::PgPool) {
        let service = db_service(db);
        service.load().await.unwrap();

        service
            .add_sites(&["bloomberg.com".to_owned()], "admin")
            .await
            .unwrap();

        let removed = service.remove_matching("*.ft.com").await.unwrap();

        assert!(removed.is_empty());
        assert!(service.covers("bloomberg.com"));
    }

    #[sqlx::test]
    async fn resolve_submits_once_then_answers_from_the_cache(db: sqlx::PgPool) {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/fetch"))
            .and(query_param("url", "https://www.bloomberg.com/news/a"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let client = UnwallClient::new(
            http::build_client(&HttpConfig::default()),
            Duration::from_secs(60),
            crate::plugin::unwall::client::base_url(&server.uri()).unwrap(),
        );
        let service = UnwallService::new(db, client, "https://unwall.app/".parse().unwrap());

        let mut article: Url = "https://www.bloomberg.com/news/a#frag".parse().unwrap();
        article.set_fragment(None);

        let first = service.resolve(&article, fetch(&article)).await.unwrap();
        assert_eq!(
            first.unwall_url,
            "https://unwall.app/www.bloomberg.com/news/a"
        );

        let second = service.resolve(&article, fetch(&article)).await.unwrap();
        assert_eq!(first, second);

        // Exactly one fetch was recorded, attributed to one user.
        let stats = service.statistics().await.unwrap();

        assert_eq!(stats.urls, 1);
        assert_eq!(stats.hosts, 1);
        assert_eq!(stats.fetches, 1);
        assert_eq!(stats.users, 1);

        let host_stats = service
            .host_statistics("WWW.Bloomberg.com")
            .await
            .unwrap()
            .expect("the host has been unwalled");

        assert_eq!(host_stats.urls, 1);
        assert_eq!(host_stats.users, 1);
        assert!(host_stats.latest.is_some());

        assert!(
            service
                .host_statistics("example.com")
                .await
                .unwrap()
                .is_none()
        );
    }
}
