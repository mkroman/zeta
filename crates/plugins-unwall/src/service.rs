//! Unwall service: the covered-site set, the URL cache, and the submissions.
//!
//! The module is split by concern: `admin` holds the covered-site management and the
//! statistics behind the admin commands, `resolve` the cached submission path, and `tests`
//! covers the service; this module holds the public [`UnwallService`], its construction and
//! the read path.

mod admin;
mod resolve;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tracing::{debug, instrument};
use url::Url;

use self::resolve::INFLIGHT_TTL;
use super::{
    client::UnwallClient,
    error::Error,
    model::{CachedUrl, Site},
    repository::UnwallRepository,
    sites,
};
use zeta_cache::TtlMap;
use zeta_plugin::database::Database;

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
    covered: ArcSwap<Vec<Site>>,
    /// Collapses a burst of the same article into a single submission.
    inflight: TtlMap<String, CachedUrl>,
}

// Manual implementation: the repository holds the database pool and the client the HTTP
// connection state, so only the reader's host and the covered-site count are shown.
impl fmt::Debug for UnwallService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnwallService")
            .field("reader_host", &self.reader_base.host_str())
            .field("covered", &self.covered.load().len())
            .finish_non_exhaustive()
    }
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
            covered: ArcSwap::from_pointee(Vec::new()),
            inflight: TtlMap::new(INFLIGHT_TTL),
        }
    }

    /// Loads the covered sites from the database into the in-memory set.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the sites could not be fetched.
    #[instrument(name = "unwall.load", skip_all, err)]
    pub async fn load(&self) -> Result<(), Error> {
        let covered = self.repo.sites().await?;

        debug!(unwall.site.count = covered.len(), "loaded unwall sites");

        self.covered.store(Arc::new(covered));

        Ok(())
    }

    /// Whether `host` is covered.
    #[must_use]
    pub fn covers(&self, host: &str) -> bool {
        let covered = self.covered.load();

        // Most deployments cover nothing, so skip the lookup — and its normalization — entirely.
        if covered.is_empty() {
            return false;
        }

        // HTTP(S) hosts are already lowercase (`Url::host_str`), so this only allocates when a
        // direct caller passes an uppercase host.
        let host = lowercase_host(host);

        covered
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
}

/// Returns `host` lowercased when it contains an uppercase ASCII byte, borrowing it otherwise.
///
/// Hostnames are ASCII (internationalized ones reach `Url::host_str` as punycode), so the ASCII
/// predicate covers every host that comes from a URL.
fn lowercase_host(host: &str) -> Cow<'_, str> {
    if host.bytes().any(|byte| byte.is_ascii_uppercase()) {
        Cow::Owned(host.to_ascii_lowercase())
    } else {
        Cow::Borrowed(host)
    }
}
