//! Notification service, keeping pending notifications in memory and persisting them in the
//! database.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Mutex;
use tracing::{instrument, trace};

use super::{
    Settings,
    error::Error,
    model::{NewNotification, Notification},
    repository::NotificationRepository,
};
use zeta_plugin::database::Database;

/// Pending notifications, keyed by channel and, within each channel, by the target nickname.
///
/// The nested maps let lookups borrow `&str` keys, so delivering a notification does not
/// allocate the keys it looks up.
type PendingByTarget = HashMap<String, HashMap<String, Vec<Notification>>>;

/// Stores notifications in the database and caches the pending ones in memory.
///
/// The cache is keyed by channel, and within each channel by the target nickname, so lookups can
/// be made with borrowed `&str` keys without allocating.
pub struct NotificationService {
    /// The notification repository.
    repo: NotificationRepository,
    /// Pending notifications, keyed by channel and target nickname.
    cache: Mutex<PendingByTarget>,
    /// The number of notifications in `cache`, maintained under the cache lock.
    ///
    /// This is only a hint for the lock-free fast path: the cache remains the source of truth.
    /// It is incremented before a notification is inserted and decremented after one is
    /// removed, so a `take` that reads zero may delay a concurrently created notification to
    /// the next message, but never loses it.
    pending: AtomicUsize,
    /// The maximum number of pending notifications a target may have in a channel.
    max_pending_per_target: usize,
}

impl NotificationService {
    /// Creates a new notification service backed by the given database pool.
    #[must_use]
    pub fn new(db: Database, settings: &Settings) -> Self {
        Self {
            repo: NotificationRepository::new(db),
            cache: Mutex::new(HashMap::new()),
            pending: AtomicUsize::new(0),
            max_pending_per_target: settings.max_pending_per_target,
        }
    }

    /// Loads all notifications from the database into the cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the notifications could not be fetched from the database.
    #[instrument(name = "notification.load", skip_all, err)]
    pub async fn load(&self) -> Result<(), Error> {
        trace!("loading notifications into memory");

        let mut loaded: PendingByTarget = HashMap::new();
        let mut pending = 0;

        for notification in self.repo.list().await? {
            trace!(?notification, "storing notification in cache");

            pending += 1;
            loaded
                .entry(notification.channel.clone())
                .or_default()
                .entry(notification.target.clone())
                .or_default()
                .push(notification);
        }

        let mut cache = self.cache.lock().await;
        *cache = loaded;
        // Store the count together with the replacement cache, so the counter always matches
        // it. `load` runs at startup, before any event is dispatched.
        self.pending.store(pending, Ordering::Relaxed);
        drop(cache);

        Ok(())
    }

    /// Creates a new notification, persisting it in the database and adding it to the cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TooManyPending`] if the target already has the maximum number of pending
    /// notifications in the channel, or [`Error::Database`] if the notification could not be
    /// inserted into the database.
    #[instrument(name = "notification.create", skip_all, err)]
    pub async fn create(&self, notification: NewNotification) -> Result<Notification, Error> {
        if self.max_pending_per_target > 0 {
            let pending = self
                .cache
                .lock()
                .await
                .get(&notification.channel)
                .and_then(|targets| targets.get(&notification.target))
                .map_or(0, Vec::len);

            if pending >= self.max_pending_per_target {
                return Err(Error::TooManyPending(self.max_pending_per_target));
            }
        }

        let notification = self.repo.insert(notification).await?;

        trace!(?notification, "storing notification in cache");

        let mut cache = self.cache.lock().await;
        self.pending.fetch_add(1, Ordering::Relaxed);
        cache
            .entry(notification.channel.clone())
            .or_default()
            .entry(notification.target.clone())
            .or_default()
            .push(notification.clone());
        drop(cache);

        Ok(notification)
    }

    /// Takes all pending notifications for `target` within `channel`, removing them from the
    /// cache.
    ///
    /// The common case is that there are none: the pending counter then lets the call return
    /// without locking the cache.
    pub async fn take(&self, channel: &str, target: &str) -> Vec<Notification> {
        if self.pending.load(Ordering::Relaxed) == 0 {
            return Vec::new();
        }

        let mut cache = self.cache.lock().await;
        let Some(targets) = cache.get_mut(channel) else {
            return Vec::new();
        };

        let taken = targets.remove(target).unwrap_or_default();

        if targets.is_empty() {
            cache.remove(channel);
        }

        self.pending.fetch_sub(taken.len(), Ordering::Relaxed);
        drop(cache);

        taken
    }

    /// Puts notifications back into the cache as pending.
    ///
    /// Called for notifications that could not be delivered: `take` removed them from the
    /// cache, so without restoring them they would not be redelivered until the next restart.
    pub async fn restore(&self, notifications: Vec<Notification>) {
        if notifications.is_empty() {
            return;
        }

        let mut cache = self.cache.lock().await;
        self.pending
            .fetch_add(notifications.len(), Ordering::Relaxed);

        for notification in notifications {
            trace!(?notification, "restoring undelivered notification");

            cache
                .entry(notification.channel.clone())
                .or_default()
                .entry(notification.target.clone())
                .or_default()
                .push(notification);
        }
    }

    /// Deletes the given notifications from the database.
    ///
    /// The cache is not touched: the notifications being delivered were already removed by
    /// `take`, and undelivered ones are returned to the cache with `restore`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the notifications could not be deleted from the database.
    #[instrument(name = "notification.delete", skip_all, err)]
    pub async fn delete_all(&self, ids: &[i32]) -> Result<(), Error> {
        if ids.is_empty() {
            return Ok(());
        }

        self.repo.delete_all(ids).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification() -> NewNotification {
        NewNotification {
            target: "someone".into(),
            nickname: "creator".into(),
            username: "user".into(),
            hostname: "host.example".into(),
            channel: "#chan".into(),
            message: "hello".into(),
        }
    }

    #[tokio::test]
    async fn taking_without_pending_notifications_does_not_lock_the_cache() {
        // `take` never touches the database, so a lazy pool is enough.
        let service =
            NotificationService::new(zeta_test_support::test_context().db, &Settings::default());

        // Hold the cache lock across `take`: only the lock-free fast path can complete.
        let cache = service.cache.lock().await;
        let taken = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            service.take("#chan", "someone"),
        )
        .await
        .expect("take must not wait on the cache lock when nothing is pending");
        drop(cache);

        assert!(taken.is_empty());
    }

    #[sqlx::test(migrations = "../plugins/migrations")]
    async fn take_with_missing_channel_or_target_keeps_the_pending_count(db: sqlx::PgPool) {
        let service = NotificationService::new(db, &Settings::default());

        service.create(notification()).await.unwrap();

        // A miss must not decrement the counter: it would reach zero while a notification
        // remains, and the final `take` would wrongly fast-path to an empty result.
        assert!(service.take("#other", "someone").await.is_empty());
        assert!(service.take("#chan", "someone-else").await.is_empty());

        assert_eq!(service.take("#chan", "someone").await.len(), 1);
    }

    #[sqlx::test(migrations = "../plugins/migrations")]
    async fn take_removes_pending_notifications_and_restore_puts_them_back(db: sqlx::PgPool) {
        let service = NotificationService::new(db, &Settings::default());

        service.create(notification()).await.unwrap();

        let taken = service.take("#chan", "someone").await;

        assert_eq!(taken.len(), 1);
        assert!(service.take("#chan", "someone").await.is_empty());

        service.restore(taken).await;

        assert_eq!(service.take("#chan", "someone").await.len(), 1);
    }

    #[sqlx::test(migrations = "../plugins/migrations")]
    async fn load_makes_stored_notifications_takeable(db: sqlx::PgPool) {
        NotificationService::new(db.clone(), &Settings::default())
            .create(notification())
            .await
            .unwrap();

        // A fresh service only sees the notification through `load`, which must also update the
        // pending counter so the lock-free `take` path does not skip it.
        let loaded = NotificationService::new(db, &Settings::default());
        loaded.load().await.unwrap();

        assert_eq!(loaded.take("#chan", "someone").await.len(), 1);
    }
}
