//! Alert service, persisting alerts in the database and broadcasting due alerts to a subscriber.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use sqlx::types::chrono::{DateTime, Utc};
use tokio::sync::{Mutex, broadcast};
use tracing::{debug, error, instrument, warn};

use super::{
    error::Error,
    model::{Alert, NewAlert},
    repository::AlertRepository,
};
use crate::database::Database;

/// How often the scheduler checks the cache for due alerts.
const SCHEDULER_INTERVAL: Duration = Duration::from_mins(5);

/// The capacity of the broadcast channel used to deliver due alerts.
const BROADCAST_CAPACITY: usize = 256;

/// Returns the deadline until which alerts are cached, i.e. the time of the next scheduler poll
/// relative to `now`.
fn next_poll(now: DateTime<Utc>) -> DateTime<Utc> {
    now + SCHEDULER_INTERVAL
}

/// Stores alerts in the database, caching them in memory and broadcasting due alerts on a
/// [`tokio::sync::broadcast`] channel.
///
/// Only the alerts that will trigger before the next scheduler poll are cached; the database
/// remains the source of truth for alerts further in the future. The scheduler task broadcasts
/// each alert as it becomes due, deleting it from both the cache and the database once broadcast,
/// and refreshes the cache with the alerts triggering before its next poll. The plugin holds a
/// receiver and delivers the alerts it receives over IRC.
#[derive(Clone)]
pub struct AlertService {
    /// The alert repository.
    repo: AlertRepository,
    /// The sender half of the broadcast channel.
    tx: broadcast::Sender<Alert>,
    /// The alerts that will trigger before the next scheduler poll, keyed by their database id.
    cache: Arc<Mutex<HashMap<i32, Alert>>>,
}

impl AlertService {
    /// Creates a new alert service backed by the given database pool.
    #[must_use]
    pub fn new(db: Database) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);

        Self {
            repo: AlertRepository::new(db),
            tx,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Returns a new receiver for due alerts.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Alert> {
        self.tx.subscribe()
    }

    /// Loads the alerts that will trigger before the next scheduler poll into the cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the alerts could not be fetched from the database.
    #[instrument(skip_all, err)]
    pub async fn load(&self) -> Result<(), Error> {
        self.refresh_cache(Utc::now()).await
    }

    /// Creates a new alert, persisting it in the database.
    ///
    /// The alert is also added to the cache if it will trigger before the next scheduler poll;
    /// alerts further in the future are picked up by a later poll.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Insert`] if the alert could not be inserted into the database.
    #[instrument(skip_all, err)]
    pub async fn create(&self, alert: NewAlert) -> Result<Alert, Error> {
        let alert = self.repo.insert(alert).await?;

        if alert.time <= next_poll(Utc::now()) {
            debug!(?alert, "storing alert in cache");
            self.cache.lock().await.insert(alert.id, alert.clone());
        }

        Ok(alert)
    }

    /// Spawns the scheduler task, broadcasting due alerts until the runtime shuts down.
    pub fn start_scheduler(&self) {
        let service = self.clone();

        tokio::spawn(async move {
            debug!("starting alert scheduler");

            let mut ticker = tokio::time::interval(SCHEDULER_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                ticker.tick().await;

                if let Err(err) = service.tick().await {
                    error!(?err, "scheduler tick failed");
                }
            }
        });
    }

    /// Broadcasts all alerts that are due, refreshing the cache with the alerts that will trigger
    /// before the next poll.
    ///
    /// # Errors
    ///
    /// Returns the repository error if the alerts could not be fetched or deleted.
    async fn tick(&self) -> Result<(), Error> {
        let now = Utc::now();

        self.broadcast_due(now).await?;
        self.refresh_cache(now).await
    }

    /// Broadcasts all alerts that are due at `now` and removes them from the cache and the
    /// database once sent.
    ///
    /// # Errors
    ///
    /// Returns the repository error if the alerts could not be deleted.
    async fn broadcast_due(&self, now: DateTime<Utc>) -> Result<(), Error> {
        let due: Vec<Alert> = self
            .cache
            .lock()
            .await
            .values()
            .filter(|alert| alert.time <= now)
            .cloned()
            .collect();

        if due.is_empty() {
            return Ok(());
        }

        let mut sent_ids = Vec::with_capacity(due.len());

        for alert in due {
            debug!(?alert, "broadcasting alert");

            let id = alert.id;

            match self.tx.send(alert) {
                Ok(_) => sent_ids.push(id),
                Err(broadcast::error::SendError(alert)) => {
                    warn!(
                        alert_id = alert.id,
                        "no active subscribers, keeping the alert for the next tick"
                    );
                }
            }
        }

        if !sent_ids.is_empty() {
            self.repo.delete_all(&sent_ids).await?;

            let sent: HashSet<i32> = sent_ids.into_iter().collect();
            self.cache.lock().await.retain(|id, _| !sent.contains(id));
        }

        Ok(())
    }

    /// Merges the alerts that will trigger before the next scheduler poll into the cache.
    ///
    /// # Errors
    ///
    /// Returns the repository error if the alerts could not be fetched.
    async fn refresh_cache(&self, now: DateTime<Utc>) -> Result<(), Error> {
        let deadline = next_poll(now);

        let alerts: HashMap<i32, Alert> = self
            .repo
            .list_until(deadline)
            .await?
            .into_iter()
            .map(|alert| (alert.id, alert))
            .collect();

        debug!(count = alerts.len(), ?deadline, "refreshed alert cache");

        // Merges instead of replacing, so an alert persisted after the query ran is not evicted
        // from the cache by `create` in the meantime.
        self.cache.lock().await.extend(alerts);

        Ok(())
    }
}
