//! Database access for alerts.

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

        sqlx::query_file_as!(
            Alert,
            "queries/insert_alert.sql",
            alert.nickname,
            alert.username,
            alert.hostname,
            alert.channel,
            alert.message,
            alert.time
        )
        .fetch_one(&self.db)
        .await
        .map_err(Error::Insert)
    }

    /// Returns all alerts in the database.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the alerts could not be fetched.
    #[instrument(skip_all, err)]
    pub async fn list(&self) -> Result<Vec<Alert>, Error> {
        trace!("loading alerts from database");

        sqlx::query_as!(
            Alert,
            r#"SELECT id, nickname, username, hostname, channel, message, time, created_at
               FROM alerts
               ORDER BY time"#
        )
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

        sqlx::query!("DELETE FROM alerts WHERE id = ANY($1)", ids)
            .execute(&self.db)
            .await
            .map_err(Error::Delete)?;

        Ok(())
    }
}
