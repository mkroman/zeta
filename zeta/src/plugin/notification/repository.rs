//! Database access for notifications.

use futures::TryStreamExt;
use tracing::{debug, instrument};

use super::{
    error::Error,
    model::{NewNotification, Notification},
};
use crate::database::Database;

/// Repository for storing and retrieving notifications in the database.
#[derive(Debug, Clone)]
pub struct NotificationRepository {
    /// The database connection pool.
    db: Database,
}

impl NotificationRepository {
    /// Creates a new repository backed by the given database pool.
    #[must_use]
    pub const fn new(db: Database) -> Self {
        Self { db }
    }

    /// Returns all notifications in the database.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the notifications could not be fetched.
    #[instrument(skip_all, err)]
    pub async fn list(&self) -> Result<Vec<Notification>, Error> {
        debug!("loading notifications from database");

        let mut notifications = Vec::new();
        let mut stream =
            sqlx::query_as!(Notification, "SELECT * FROM notifications").fetch(&self.db);

        while let Some(notification) = stream.try_next().await.map_err(Error::Load)? {
            notifications.push(notification);
        }

        Ok(notifications)
    }

    /// Inserts `notification` into the database, returning the stored instance.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Insert`] if the notification could not be inserted.
    #[instrument(skip_all, err)]
    pub async fn insert(&self, notification: NewNotification) -> Result<Notification, Error> {
        debug!("inserting notification into database");

        sqlx::query_file_as!(
            Notification,
            "queries/insert_notification.sql",
            notification.target,
            notification.nickname,
            notification.username,
            notification.hostname,
            notification.channel,
            notification.message
        )
        .fetch_one(&self.db)
        .await
        .map_err(Error::Insert)
    }

    /// Deletes the notification with the given `ids`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Delete`] if the notification could not be deleted.
    #[instrument(skip_all, err)]
    pub async fn delete_all(&self, ids: &[i32]) -> Result<(), Error> {
        debug!(?ids, "deleting notifications from database");

        sqlx::query!("DELETE FROM notifications WHERE id = ANY($1)", ids)
            .execute(&self.db)
            .await
            .map_err(Error::Delete)?;

        Ok(())
    }
}
