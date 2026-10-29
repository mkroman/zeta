//! Database access for filters.

use tracing::trace;

use super::{
    error::Error,
    model::{Filter, NewFilter},
};
use zeta_plugin::instrument_database;

zeta_plugin::impl_repository! {
    /// Repository for storing and retrieving filters in the database.
    FilterRepository
}

impl FilterRepository {
    /// Inserts `filter` into the database, returning the stored instance.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Insert`] if the filter could not be inserted.
    #[instrument_database(operation = "INSERT", collection = "filters")]
    pub async fn insert(&self, filter: NewFilter) -> Result<Filter, Error> {
        trace!("inserting filter into database");

        sqlx::query_as(
            r"INSERT INTO filters (
                channel, host, path, nickname, username, hostname, created_by
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7
            ) RETURNING id, channel, host, path, nickname, username, hostname",
        )
        .bind(filter.channel)
        .bind(filter.host)
        .bind(filter.path)
        .bind(filter.nickname)
        .bind(filter.username)
        .bind(filter.hostname)
        .bind(filter.created_by)
        .fetch_one(&self.db)
        .await
        .map_err(Error::insert)
    }

    /// Returns all filters in the database.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the filters could not be fetched.
    #[instrument_database(operation = "SELECT", collection = "filters")]
    pub async fn list(&self) -> Result<Vec<Filter>, Error> {
        trace!("loading filters from database");

        sqlx::query_as(
            r"SELECT id, channel, host, path, nickname, username, hostname
              FROM filters
              ORDER BY id",
        )
        .fetch_all(&self.db)
        .await
        .map_err(Error::load)
    }

    /// Deletes the filters with the given `ids`, returning the number of rows removed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Delete`] if the filters could not be deleted.
    #[instrument_database(operation = "DELETE", collection = "filters")]
    pub async fn delete_all(&self, ids: &[i32]) -> Result<u64, Error> {
        trace!(?ids, "deleting filters from database");

        zeta_plugin::database::delete_ids(&self.db, "filters", ids)
            .await
            .map_err(Error::delete)
    }
}
