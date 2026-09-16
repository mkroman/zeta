//! Database access for alerts.

use sqlx::types::chrono::{DateTime, Utc};
use tracing::{instrument, trace};

use super::{
    error::Error,
    model::{Alert, NewAlert},
};
use crate::database::Database;

/// Repository for storing and retrieving alerts in the database.
#[derive(Debug, Clone)]
pub struct AlertRepository {
    /// The database connection pool.
    db: Database,
}

impl AlertRepository {
    /// Creates a new repository backed by the given database pool.
    #[must_use]
    pub const fn new(db: Database) -> Self {
        Self { db }
    }

    /// Inserts `alert` into the database, returning the stored instance.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Insert`] if the alert could not be inserted.
    #[instrument(skip_all, err)]
    pub async fn insert(&self, alert: NewAlert) -> Result<Alert, Error> {
        trace!("inserting alert into database");

        sqlx::query_as(
            r"INSERT INTO alerts (
                nickname, username, hostname, channel, message, time
            ) VALUES (
                $1, $2, $3, $4, $5, $6
            ) RETURNING id, nickname, username, hostname, channel, message, time, created_at",
        )
        .bind(alert.nickname)
        .bind(alert.username)
        .bind(alert.hostname)
        .bind(alert.channel)
        .bind(alert.message)
        .bind(alert.time)
        .fetch_one(&self.db)
        .await
        .map_err(Error::Insert)
    }

    /// Returns the alerts that are due at or before `cutoff`, ordered by the time they are due.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the alerts could not be fetched.
    #[instrument(skip_all, err)]
    pub async fn list_due_before(&self, cutoff: DateTime<Utc>) -> Result<Vec<Alert>, Error> {
        trace!(?cutoff, "loading alerts from database");

        sqlx::query_as(
            r"SELECT id, nickname, username, hostname, channel, message, time, created_at
              FROM alerts
              WHERE time <= $1
              ORDER BY time",
        )
        .bind(cutoff)
        .fetch_all(&self.db)
        .await
        .map_err(Error::Load)
    }

    /// Returns the pending alerts of `nickname` in `channel`, ordered by the time they are due.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the alerts could not be fetched.
    #[instrument(skip_all, err)]
    pub async fn list_for(&self, channel: &str, nickname: &str) -> Result<Vec<Alert>, Error> {
        trace!(channel, nickname, "loading pending alerts from database");

        sqlx::query_as(
            r"SELECT id, nickname, username, hostname, channel, message, time, created_at
              FROM alerts
              WHERE channel = $1 AND nickname = $2
              ORDER BY time",
        )
        .bind(channel)
        .bind(nickname)
        .fetch_all(&self.db)
        .await
        .map_err(Error::Load)
    }

    /// Deletes the alerts with the given `ids`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Delete`] if the alerts could not be deleted.
    #[instrument(skip_all, err)]
    pub async fn delete_all(&self, ids: &[i32]) -> Result<(), Error> {
        trace!(?ids, "deleting alerts from database");

        sqlx::query("DELETE FROM alerts WHERE id = ANY($1)")
            .bind(ids)
            .execute(&self.db)
            .await
            .map_err(Error::Delete)?;

        Ok(())
    }
}
