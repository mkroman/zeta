//! Unwall service: the covered-site set, the URL cache, and the submissions.

use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use sqlx::types::chrono::Utc;
use tracing::{debug, instrument, warn};
use url::Url;

use super::{
    client::UnwallClient,
    error::Error,
    model::{CachedUrl, HostStatistics, NewFetch, Site, SiteRemoval, Statistics},
    repository::UnwallRepository,
    sites,
};
use crate::{cache::TtlMap, database::Database};

/// How long a submitted article stays in the in-flight cache, collapsing a burst of the same
/// article into a single submission.
const INFLIGHT_TTL: Duration = Duration::from_mins(10);

/// The covered-site set: the tested domains plus the admin-added sites, minus the removals.
///
/// Held entirely in memory — the database is the source of truth for the added sites and the
/// removals, loaded on [`load`](UnwallService::load) and synced after every change; the tested
/// domains are refreshed from the API in the background. The hot lookup path
/// ([`covers`](UnwallService::covers)) only ever touches this set.
#[derive(Debug, Default)]
struct SiteIndex {
    /// The sites unwall.app has tested, from the snapshot or the live API.
    tested: Vec<String>,
    /// The admin-added sites.
    added: Vec<Site>,
    /// The tombstoned tested domains.
    removed: Vec<SiteRemoval>,
}

impl SiteIndex {
    /// Whether `host` is covered.
    ///
    /// Precedence: an explicit admin add wins over a removal, and a removal wins over the
    /// tested domains — so re-adding a host re-unwalls exactly that host even while the
    /// wildcard that removed it stays in place.
    fn covers(&self, host: &str) -> bool {
        self.added
            .iter()
            .any(|site| sites::matches_site(host, &site.host))
            || (!self.is_removed(host)
                && self
                    .tested
                    .iter()
                    .any(|site| sites::matches_site(host, site)))
    }

    /// Whether `host` is covered only by a tested domain that has been removed.
    fn is_removed(&self, host: &str) -> bool {
        self.removed
            .iter()
            .any(|removal| sites::matches_site(host, &removal.host))
    }

    /// Returns the covered sites matching `pattern`, sorted: the added sites and the tested
    /// domains the removal of which the pattern names.
    fn matching(&self, pattern: &str) -> Vec<String> {
        let mut hosts: Vec<String> = self
            .added
            .iter()
            .map(|site| site.host.clone())
            .chain(self.tested.iter().cloned())
            .filter(|host| sites::matches_site(host, pattern))
            .collect();

        hosts.sort();
        hosts.dedup();

        hosts
    }
}

/// Stores the covered-site set in memory, caches unwalled URLs in the database, and submits
/// articles to unwall.app.
pub struct UnwallService {
    /// The unwall repository.
    repo: UnwallRepository,
    /// The unwall API client.
    client: UnwallClient,
    /// The base URL of the unwall reader, which the replies link to.
    reader_base: Url,
    /// The in-memory covered-site set.
    index: RwLock<SiteIndex>,
    /// Collapses a burst of the same article into a single submission.
    inflight: TtlMap<String, CachedUrl>,
}

impl UnwallService {
    /// Creates a new service backed by the given database pool and API client, covering the
    /// static tested domains until [`load`](Self::load) and the first live refresh.
    #[must_use]
    pub fn new(db: Database, client: UnwallClient, reader_base: Url) -> Self {
        Self {
            repo: UnwallRepository::new(db),
            client,
            reader_base,
            index: RwLock::new(SiteIndex {
                tested: sites::default_sites(),
                added: Vec::new(),
                removed: Vec::new(),
            }),
            inflight: TtlMap::new(INFLIGHT_TTL),
        }
    }

    /// Loads the admin-added sites and removals from the database into the index.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the sites or the removals could not be fetched.
    #[instrument(skip_all, err)]
    pub async fn load(&self) -> Result<(), Error> {
        let added = self.repo.sites().await?;
        let removed = self.repo.removals().await?;

        debug!(added = added.len(), removed = removed.len(), "loaded unwall sites");

        *self.lock_index_mut() = SiteIndex {
            tested: sites::default_sites(),
            added,
            removed,
        };

        Ok(())
    }

    /// Whether `host` is covered.
    #[must_use]
    pub fn covers(&self, host: &str) -> bool {
        self.lock_index().covers(&host.to_lowercase())
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
    /// A wildcard add overrides the removals it matches; an exact add keeps them, winning over
    /// them for its own host only (see `SiteIndex::covers`).
    ///
    /// The hosts are expected normalized (see `sites::normalize_site`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the sites could not be inserted or the index synced.
    #[instrument(skip_all, err)]
    pub async fn add_sites(&self, hosts: &[String], nickname: &str) -> Result<(), Error> {
        for host in hosts {
            self.repo.insert_site(host, nickname).await?;
        }

        // A re-added wildcard overrides the removals it matches.
        let overridden: Vec<String> = self
            .repo
            .removals()
            .await?
            .into_iter()
            .map(|removal| removal.host)
            .filter(|removal| {
                hosts
                    .iter()
                    .any(|host| host.starts_with("*.") && sites::matches_site(removal, host))
            })
            .collect();

        if !overridden.is_empty() {
            self.repo.delete_removals(&overridden).await?;
        }

        self.sync_index().await
    }

    /// Removes every covered site matching `pattern`, tombstoning the matching tested domains.
    ///
    /// Returns the hosts removed, sorted.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the sites or the removals could not be written, or the
    /// index synced.
    #[instrument(skip_all, err, fields(pattern = %pattern))]
    pub async fn remove_matching(
        &self,
        pattern: &str,
        nickname: &str,
    ) -> Result<Vec<String>, Error> {
        let affected = self.lock_index().matching(pattern);

        if affected.is_empty() {
            return Ok(Vec::new());
        }

        let (added, tested): (Vec<String>, Vec<String>) = {
            let index = self.lock_index();

            (
                index
                    .added
                    .iter()
                    .map(|site| site.host.clone())
                    .filter(|host| sites::matches_site(host, pattern))
                    .collect(),
                index
                    .tested
                    .iter()
                    .filter(|site| sites::matches_site(site, pattern))
                    .cloned()
                    .collect(),
            )
        };

        if !added.is_empty() {
            self.repo.delete_sites(&added).await?;
        }

        for host in &tested {
            self.repo.insert_removal(host, nickname).await?;
        }

        self.sync_index().await?;

        Ok(affected)
    }

    /// Returns a snapshot of the admin-added sites, ordered by host.
    #[must_use]
    pub fn added_sites(&self) -> Vec<Site> {
        self.lock_index().added.clone()
    }

    /// Returns the coverage counts: the tested domains, the removals, and the added sites.
    #[must_use]
    pub fn coverage(&self) -> (usize, usize, usize) {
        let index = self.lock_index();

        (index.tested.len(), index.removed.len(), index.added.len())
    }

    /// Refreshes the tested domains from the API, replacing the tested list in the index.
    ///
    /// A failed refresh keeps the previous list: the set is served stale rather than empty.
    ///
    /// # Errors
    ///
    /// Returns the errors of `UnwallClient::tested_domains`.
    #[instrument(skip_all, err)]
    pub async fn refresh_tested_domains(&self) -> Result<usize, Error> {
        let tested = self.client.tested_domains().await?;
        let count = tested.len();

        self.lock_index_mut().tested = tested;

        debug!(count, "refreshed the tested domains");

        Ok(count)
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

    /// Reloads the added sites and the removals from the database into the index.
    async fn sync_index(&self) -> Result<(), Error> {
        let added = self.repo.sites().await?;
        let removed = self.repo.removals().await?;

        {
            let mut index = self.lock_index_mut();
            index.added = added;
            index.removed = removed;
        }

        Ok(())
    }

    /// Locks the index for reading, continuing on poisoning.
    fn lock_index(&self) -> RwLockReadGuard<'_, SiteIndex> {
        crate::sync::read(&self.index)
    }

    /// Locks the index for writing, continuing on poisoning.
    fn lock_index_mut(&self) -> RwLockWriteGuard<'_, SiteIndex> {
        crate::sync::write(&self.index)
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
    async fn the_tested_domains_cover_out_of_the_box(db: sqlx::PgPool) {
        let service = db_service(db);
        service.load().await.unwrap();

        assert!(service.covers("ft.com"));
        assert!(service.covers("www.ft.com"));
        assert!(service.covers("NYTimes.com"));
        assert!(!service.covers("example.com"));
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
    async fn removing_matches_added_sites_and_tombstones_tested_domains(db: sqlx::PgPool) {
        let service = db_service(db);
        service.load().await.unwrap();

        service
            .add_sites(&["bloomberg.com".to_owned(), "*.bloomberg.com".to_owned()], "admin")
            .await
            .unwrap();

        let removed = service.remove_matching("*.bloomberg.com", "admin").await.unwrap();

        assert_eq!(removed, ["*.bloomberg.com", "bloomberg.com"]);
        assert!(!service.covers("www.bloomberg.com"));
        assert!(!service.covers("bloomberg.com"));
    }

    #[sqlx::test]
    async fn a_removed_tested_domain_stays_removed_until_readded(db: sqlx::PgPool) {
        let service = db_service(db);
        service.load().await.unwrap();

        let removed = service.remove_matching("*.ft.com", "admin").await.unwrap();

        assert_eq!(removed, ["ft.com"]);
        assert!(!service.covers("ft.com"));
        assert!(!service.covers("www.ft.com"));

        // The tombstone survives a fresh load.
        service.load().await.unwrap();

        assert!(!service.covers("ft.com"));

        // Re-adding overrides it.
        service.add_sites(&["ft.com".to_owned()], "admin").await.unwrap();

        assert!(service.covers("ft.com"));
    }

    #[sqlx::test]
    async fn an_exact_add_wins_over_a_matching_tombstone_for_its_own_host(db: sqlx::PgPool) {
        let service = db_service(db);
        service.load().await.unwrap();

        service.remove_matching("*.ft.com", "admin").await.unwrap();
        service
            .add_sites(&["www.ft.com".to_owned()], "admin")
            .await
            .unwrap();

        // The added site overrides the tombstone it matches, but the rest of the wildcard
        // tombstone keeps its coverage removed.
        assert!(service.covers("www.ft.com"));
        assert!(!service.covers("ft.com"));
        assert!(!service.covers("api.ft.com"));
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
