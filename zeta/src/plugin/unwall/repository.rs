//! Database access for unwalled URLs, sites, and fetches.

use sqlx::types::chrono::{DateTime, NaiveDate, Utc};
use tracing::{instrument, trace};

use super::{
    error::Error,
    model::{CachedUrl, HostStatistics, NewFetch, Site, SiteRemoval, Statistics},
};
use crate::database::Database;

/// Repository for storing and retrieving unwalled URLs, sites, and fetches.
#[derive(Debug, Clone)]
pub struct UnwallRepository {
    /// The database connection pool.
    db: Database,
}

impl UnwallRepository {
    /// Creates a new repository backed by the given database pool.
    #[must_use]
    pub const fn new(db: Database) -> Self {
        Self { db }
    }

    /// Returns the cached URL row for `url`, if it has been unwalled before.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the URL could not be fetched.
    #[instrument(
        name = "SELECT unwall_urls",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "SELECT",
            db.collection.name = "unwall_urls",
            db.query.summary = "SELECT unwall_urls",
        )
    )]
    pub async fn cached_url(&self, url: &str) -> Result<Option<CachedUrl>, Error> {
        trace!("loading the cached unwall URL from database");

        sqlx::query_as(
            r"SELECT id, url, host, unwall_url, created_at
              FROM unwall_urls
              WHERE url = $1",
        )
        .bind(url)
        .fetch_optional(&self.db)
        .await
        .map_err(Error::load)
    }

    /// Caches `url` as unwalled, pointing at `unwall_url`. An existing row for the URL is left
    /// alone, keeping its original creation time.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Insert`] if the URL could not be inserted.
    #[instrument(
        name = "INSERT unwall_urls",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "INSERT",
            db.collection.name = "unwall_urls",
            db.query.summary = "INSERT unwall_urls",
        )
    )]
    pub async fn insert_url(&self, url: &str, host: &str, unwall_url: &str) -> Result<(), Error> {
        trace!(host, "caching the unwalled URL in database");

        sqlx::query(
            r"INSERT INTO unwall_urls (url, host, unwall_url)
              VALUES ($1, $2, $3)
              ON CONFLICT (url) DO NOTHING",
        )
        .bind(url)
        .bind(host)
        .bind(unwall_url)
        .execute(&self.db)
        .await
        .map_err(Error::insert)?;

        Ok(())
    }

    /// Records an attributed initial fetch of `fetch.url`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Insert`] if the fetch could not be inserted.
    #[instrument(
        name = "INSERT unwall_fetches",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "INSERT",
            db.collection.name = "unwall_fetches",
            db.query.summary = "INSERT unwall_fetches",
        )
    )]
    pub async fn record_fetch(&self, fetch: &NewFetch) -> Result<(), Error> {
        trace!(host = fetch.host, channel = fetch.channel, "recording the unwall fetch in database");

        sqlx::query(
            r"INSERT INTO unwall_fetches (
                url, host, nickname, username, hostname, channel, network_id
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7
            )",
        )
        .bind(&fetch.url)
        .bind(&fetch.host)
        .bind(&fetch.nickname)
        .bind(&fetch.username)
        .bind(&fetch.hostname)
        .bind(&fetch.channel)
        .bind(&fetch.network_id)
        .execute(&self.db)
        .await
        .map_err(Error::insert)?;

        Ok(())
    }

    /// Returns the admin-added sites, ordered by host.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the sites could not be fetched.
    #[instrument(
        name = "SELECT unwall_sites",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "SELECT",
            db.collection.name = "unwall_sites",
            db.query.summary = "SELECT unwall_sites",
        )
    )]
    pub async fn sites(&self) -> Result<Vec<Site>, Error> {
        trace!("loading the unwalled sites from database");

        sqlx::query_as(
            r"SELECT id, host, nickname, created_at
              FROM unwall_sites
              ORDER BY host",
        )
        .fetch_all(&self.db)
        .await
        .map_err(Error::load)
    }

    /// Adds `host` as a covered site. An existing entry for the host is left alone.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Insert`] if the site could not be inserted.
    #[instrument(
        name = "INSERT unwall_sites",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "INSERT",
            db.collection.name = "unwall_sites",
            db.query.summary = "INSERT unwall_sites",
        )
    )]
    pub async fn insert_site(&self, host: &str, nickname: &str) -> Result<(), Error> {
        trace!(host, "adding the unwalled site to database");

        sqlx::query(
            r"INSERT INTO unwall_sites (host, nickname)
              VALUES ($1, $2)
              ON CONFLICT (host) DO NOTHING",
        )
        .bind(host)
        .bind(nickname)
        .execute(&self.db)
        .await
        .map_err(Error::insert)?;

        Ok(())
    }

    /// Removes the given hosts from the admin-added sites, returning the number removed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Delete`] if the sites could not be deleted.
    #[instrument(
        name = "DELETE unwall_sites",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "DELETE",
            db.collection.name = "unwall_sites",
            db.query.summary = "DELETE unwall_sites",
        )
    )]
    pub async fn delete_sites(&self, hosts: &[String]) -> Result<u64, Error> {
        trace!(count = hosts.len(), "removing the unwalled sites from database");

        sqlx::query("DELETE FROM unwall_sites WHERE host = ANY($1)")
            .bind(hosts)
            .execute(&self.db)
            .await
            .map(|result| result.rows_affected())
            .map_err(Error::delete)
    }

    /// Returns the removal tombstones, ordered by host.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the removals could not be fetched.
    #[instrument(
        name = "SELECT unwall_site_removals",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "SELECT",
            db.collection.name = "unwall_site_removals",
            db.query.summary = "SELECT unwall_site_removals",
        )
    )]
    pub async fn removals(&self) -> Result<Vec<SiteRemoval>, Error> {
        trace!("loading the unwalled site removals from database");

        sqlx::query_as(
            r"SELECT id, host, nickname, created_at
              FROM unwall_site_removals
              ORDER BY host",
        )
        .fetch_all(&self.db)
        .await
        .map_err(Error::load)
    }

    /// Tombstones `host`, so a tested domain it matches is no longer covered. An existing
    /// tombstone for the host is left alone.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Insert`] if the removal could not be inserted.
    #[instrument(
        name = "INSERT unwall_site_removals",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "INSERT",
            db.collection.name = "unwall_site_removals",
            db.query.summary = "INSERT unwall_site_removals",
        )
    )]
    pub async fn insert_removal(&self, host: &str, nickname: &str) -> Result<(), Error> {
        trace!(host, "adding the unwalled site removal to database");

        sqlx::query(
            r"INSERT INTO unwall_site_removals (host, nickname)
              VALUES ($1, $2)
              ON CONFLICT (host) DO NOTHING",
        )
        .bind(host)
        .bind(nickname)
        .execute(&self.db)
        .await
        .map_err(Error::insert)?;

        Ok(())
    }

    /// Deletes the given removal tombstones, returning the number deleted.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Delete`] if the removals could not be deleted.
    #[instrument(
        name = "DELETE unwall_site_removals",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "DELETE",
            db.collection.name = "unwall_site_removals",
            db.query.summary = "DELETE unwall_site_removals",
        )
    )]
    pub async fn delete_removals(&self, hosts: &[String]) -> Result<u64, Error> {
        trace!(count = hosts.len(), "removing the unwalled site removals from database");

        sqlx::query("DELETE FROM unwall_site_removals WHERE host = ANY($1)")
            .bind(hosts)
            .execute(&self.db)
            .await
            .map(|result| result.rows_affected())
            .map_err(Error::delete)
    }

    /// Returns the overall statistics of the unwalled URLs and fetches.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the statistics could not be queried.
    #[instrument(
        name = "SELECT unwall statistics",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "SELECT",
            db.collection.name = "unwall_urls",
            db.query.summary = "SELECT unwall statistics",
        )
    )]
    pub async fn statistics(&self, today: NaiveDate) -> Result<Statistics, Error> {
        trace!(%today, "loading the unwall statistics from database");

        let (urls, urls_today, hosts, fetches, fetches_today, users): (
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
        ) = sqlx::query_as(
            r"SELECT
                (SELECT COUNT(*) FROM unwall_urls),
                (SELECT COUNT(*) FROM unwall_urls WHERE created_at >= $1),
                (SELECT COUNT(DISTINCT host) FROM unwall_urls),
                (SELECT COUNT(*) FROM unwall_fetches),
                (SELECT COUNT(*) FROM unwall_fetches WHERE created_at >= $1),
                (SELECT COUNT(DISTINCT (nickname, username, hostname)) FROM unwall_fetches)",
        )
        .bind(today)
        .fetch_one(&self.db)
        .await
        .map_err(Error::load)?;

        Ok(Statistics {
            urls,
            urls_today,
            hosts,
            fetches,
            fetches_today,
            users,
        })
    }

    /// Returns the statistics for `host`, or [`None`] when nothing has been unwalled for it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the statistics could not be queried.
    #[instrument(
        name = "SELECT unwall host statistics",
        skip_all,
        err,
        fields(
            db.system.name = "postgresql",
            db.namespace = %self.db.connect_options().get_database().unwrap_or_default(),
            db.operation.name = "SELECT",
            db.collection.name = "unwall_urls",
            db.query.summary = "SELECT unwall host statistics",
        )
    )]
    pub async fn host_statistics(
        &self,
        host: &str,
        today: NaiveDate,
    ) -> Result<Option<HostStatistics>, Error> {
        trace!(host, %today, "loading the unwall host statistics from database");

        let (urls, urls_today, users, latest): (i64, i64, i64, Option<DateTime<Utc>>) =
            sqlx::query_as(
                r"SELECT
                    (SELECT COUNT(*) FROM unwall_urls WHERE host = $1),
                    (SELECT COUNT(*) FROM unwall_urls WHERE host = $1 AND created_at >= $2),
                    (SELECT COUNT(DISTINCT (nickname, username, hostname))
                       FROM unwall_fetches WHERE host = $1),
                    (SELECT MAX(created_at) FROM unwall_urls WHERE host = $1)",
            )
            .bind(host)
            .bind(today)
            .fetch_one(&self.db)
            .await
            .map_err(Error::load)?;

        if urls == 0 {
            return Ok(None);
        }

        Ok(Some(HostStatistics {
            urls,
            urls_today,
            users,
            latest,
        }))
    }
}
