//! Database access for notifications.

use tracing::trace;

use super::{
    error::Error,
    model::{NewNotification, Notification},
};
use zeta_plugin::instrument_database;

zeta_plugin::impl_repository! {
    /// Repository for storing and retrieving notifications in the database.
    NotificationRepository
}

impl NotificationRepository {
    /// Returns all notifications in the database.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the notifications could not be fetched.
    #[instrument_database(operation = "SELECT", collection = "notifications")]
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
    #[instrument_database(operation = "INSERT", collection = "notifications")]
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
    #[instrument_database(operation = "DELETE", collection = "notifications")]
    pub async fn delete_all(&self, ids: &[i32]) -> Result<(), Error> {
        trace!(?ids, "deleting notifications from database");

        zeta_plugin::database::delete_ids(&self.db, "notifications", ids)
            .await
            .map_err(Error::delete)?;

        Ok(())
    }
}
