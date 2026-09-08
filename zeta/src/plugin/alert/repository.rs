//! Database access for alerts.

use futures::TryStreamExt;
use tracing::{debug, instrument};

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
        debug!("inserting alert into database");

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

    /// Returns all alerts that are due, i.e. scheduled for a time in the past.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the alerts could not be fetched.
    #[instrument(skip_all, err)]
    pub async fn list_due(&self) -> Result<Vec<Alert>, Error> {
        debug!("fetching due alerts from database");

        let mut alerts = Vec::new();
        let mut stream = sqlx::query_as!(
            Alert,
            "SELECT * FROM alerts WHERE time <= NOW() ORDER BY time"
        )
        .fetch(&self.db);

        while let Some(alert) = stream.try_next().await.map_err(Error::Load)? {
            alerts.push(alert);
        }

        Ok(alerts)
    }

    /// Deletes the alerts with the given `ids`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Delete`] if the alerts could not be deleted.
    #[instrument(skip_all, err)]
    pub async fn delete_all(&self, ids: &[i32]) -> Result<(), Error> {
        debug!(?ids, "deleting alerts from database");

        sqlx::query!("DELETE FROM alerts WHERE id = ANY($1)", ids)
            .execute(&self.db)
            .await
            .map_err(Error::Delete)?;

        Ok(())
    }
}
