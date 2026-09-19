//! Database access for notifications.

use tracing::{instrument, trace};

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
        trace!("loading notifications from database");

        sqlx::query_as(
            r"SELECT id, target, nickname, channel, message, created_at
              FROM notifications",
        )
        .fetch_all(&self.db)
        .await
        .map_err(Error::load)
    }

    /// Inserts `notification` into the database, returning the stored instance.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Insert`] if the notification could not be inserted.
    #[instrument(skip_all, err)]
    pub async fn insert(&self, notification: NewNotification) -> Result<Notification, Error> {
        trace!("inserting notification into database");

        sqlx::query_as(
            r"INSERT INTO notifications (
                target, nickname, username, hostname, channel, message
            ) VALUES (
                $1, $2, $3, $4, $5, $6
            ) RETURNING *",
        )
        .bind(notification.target)
        .bind(notification.nickname)
        .bind(notification.username)
        .bind(notification.hostname)
        .bind(notification.channel)
        .bind(notification.message)
        .fetch_one(&self.db)
        .await
        .map_err(Error::insert)
    }

    /// Deletes the notification with the given `ids`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Delete`] if the notification could not be deleted.
    #[instrument(skip_all, err)]
    pub async fn delete_all(&self, ids: &[i32]) -> Result<(), Error> {
        trace!(?ids, "deleting notifications from database");

        crate::database::delete_ids(&self.db, "notifications", ids)
            .await
            .map_err(Error::delete)?;

        Ok(())
    }
}
