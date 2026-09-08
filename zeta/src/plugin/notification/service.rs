//! Notification service, keeping pending notifications in memory and persisting them in the
//! database.

use std::collections::HashMap;

use tokio::sync::Mutex;
use tracing::{debug, instrument};

use super::{
    error::Error,
    model::{NewNotification, Notification},
    repository::NotificationRepository,
};
use crate::database::Database;

/// Stores notifications in the database and caches the pending ones in memory.
///
/// The cache is keyed by channel, and within each channel by the target nickname, so lookups can
/// be made with borrowed `&str` keys without allocating.
pub struct NotificationService {
    /// The notification repository.
    repo: NotificationRepository,
    /// Pending notifications, keyed by channel and target nickname.
    cache: Mutex<HashMap<String, HashMap<String, Vec<Notification>>>>,
}

impl NotificationService {
    /// Creates a new notification service backed by the given database pool.
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self {
            repo: NotificationRepository::new(db),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Loads all notifications from the database into the cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the notifications could not be fetched from the database.
    #[instrument(skip_all, err)]
    pub async fn load(&self) -> Result<(), Error> {
        debug!("loading notifications into memory");

        let mut cache: HashMap<String, HashMap<String, Vec<Notification>>> = HashMap::new();

        for notification in self.repo.list().await? {
            debug!(?notification, "storing notification in cache");

            cache
                .entry(notification.channel.clone())
                .or_default()
                .entry(notification.target.clone())
                .or_default()
                .push(notification);
        }

        *self.cache.lock().await = cache;

        Ok(())
    }

    /// Creates a new notification, persisting it in the database and adding it to the cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Insert`] if the notification could not be inserted into the database.
    #[instrument(skip_all, err)]
    pub async fn create(&self, notification: NewNotification) -> Result<Notification, Error> {
        let notification = self.repo.insert(notification).await?;

        debug!(?notification, "storing notification in cache");
        self.cache
            .lock()
            .await
            .entry(notification.channel.clone())
            .or_default()
            .entry(notification.target.clone())
            .or_default()
            .push(notification.clone());

        Ok(notification)
    }

    /// Takes all pending notifications for `target` within `channel`, removing them from the
    /// cache.
    pub async fn take(&self, channel: &str, target: &str) -> Vec<Notification> {
        let mut cache = self.cache.lock().await;
        let Some(targets) = cache.get_mut(channel) else {
            return Vec::new();
        };

        let taken = targets.remove(target).unwrap_or_default();

        if targets.is_empty() {
            cache.remove(channel);
        }

        taken
    }

    /// Deletes the notification with the given `id` from both the database and the cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Delete`] if the notification could not be deleted from the database.
    #[instrument(skip_all, err)]
    pub async fn delete(&self, id: i32) -> Result<(), Error> {
        self.repo.delete(id).await?;

        self.cache.lock().await.retain(|_, targets| {
            targets.retain(|_, notifications| {
                notifications.retain(|notification| notification.id != id);
                !notifications.is_empty()
            });

            !targets.is_empty()
        });

        Ok(())
    }
}
