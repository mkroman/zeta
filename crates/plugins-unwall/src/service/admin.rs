//! Covered-site management and the statistics behind the admin commands.

use std::sync::Arc;

use sqlx::types::chrono::Utc;
use tracing::instrument;

use super::UnwallService;
use crate::error::Error;
use crate::model::{HostStatistics, Site, Statistics};
use crate::sites;

impl UnwallService {
    /// Adds `hosts` as covered sites.
    ///
    /// The hosts are expected normalized (see `sites::normalize_site`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the sites could not be inserted or reloaded.
    #[instrument(name = "unwall.site.add", skip_all)]
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
    #[instrument(name = "unwall.site.remove", skip_all, fields(unwall.site.pattern = %pattern))]
    pub async fn remove_matching(&self, pattern: &str) -> Result<Vec<String>, Error> {
        let mut affected: Vec<String> = {
            let covered = self.covered.load();
            covered
                .iter()
                .filter(|site| sites::matches_site(&site.host, pattern))
                .map(|site| site.host.clone())
                .collect()
        };

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
        Arc::unwrap_or_clone(self.covered.load_full())
    }

    /// Returns the overall statistics of the unwalled URLs and fetches.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the statistics could not be queried.
    #[instrument(name = "unwall.statistics", skip_all)]
    pub async fn statistics(&self) -> Result<Statistics, Error> {
        self.repo.statistics(Utc::now().date_naive()).await
    }

    /// Returns the statistics for `host`, or [`None`] when nothing has been unwalled for it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the statistics could not be queried.
    #[instrument(name = "unwall.host.statistics", skip_all)]
    pub async fn host_statistics(&self, host: &str) -> Result<Option<HostStatistics>, Error> {
        self.repo
            .host_statistics(&host.trim().to_ascii_lowercase(), Utc::now().date_naive())
            .await
    }
}
