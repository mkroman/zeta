//! The scheduled-alert queue, the delivery channel and the scheduler task.
//!
//! Alerts due within the window are kept in a binary heap ordered by their due time. The
//! scheduler task syncs the heap from the database, sleeps until the next alert is due (or
//! until an alert is created that may be due sooner), and hands each due alert to the delivery
//! task over the unbounded channel created in [`Scheduler::new`].

use std::{
    cmp::{Ordering, Reverse},
    collections::BinaryHeap,
    sync::Arc,
    time::Duration,
};

use parking_lot::Mutex as ParkingMutex;
use sqlx::types::chrono::{DateTime, Utc};
use tokio::sync::{Mutex, Notify, mpsc};
use tracing::{Instrument, debug, error, instrument, trace, warn};

use crate::{
    Settings,
    error::Error,
    model::{Alert, NewAlert},
    repository::AlertRepository,
};
use zeta_plugin::{Chain, database::Database};

/// An alert scheduled for delivery, ordered by the time it is due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Scheduled(pub(super) Alert);

impl Ord for Scheduled {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .time
            .cmp(&other.0.time)
            .then(self.0.id.cmp(&other.0.id))
    }
}

impl PartialOrd for Scheduled {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The state shared between the service and the scheduler task.
#[derive(Clone)]
pub(super) struct Scheduler {
    /// The alert repository.
    pub(super) repo: AlertRepository,
    /// The sender half of the channel used to deliver due alerts, taken to close the channel
    /// on shutdown.
    ///
    /// No user code runs under the lock — locking only clones or takes the sender — so the
    /// `Option` cannot be left half-updated by a panic, which is what makes this non-poisoning
    /// lock sound.
    tx: Arc<ParkingMutex<Option<mpsc::UnboundedSender<Alert>>>>,
    /// The alerts due within the window, ordered by the time they are due.
    pub(super) cache: Arc<Mutex<BinaryHeap<Reverse<Scheduled>>>>,
    /// Wakes the scheduler to sync the window when an alert is created.
    notify: Arc<Notify>,
    /// How long the scheduler waits before retrying after a failed tick.
    pub(super) retry_delay: Duration,
    /// How often the window of upcoming alerts is synced from the database.
    pub(super) sync_interval: Duration,
    /// How far ahead of their due time alerts are kept in the cache.
    pub(super) window: Duration,
}

impl Scheduler {
    /// Creates the scheduler and the channel it delivers due alerts over, returning the
    /// receiver for the delivery task.
    #[must_use]
    pub(super) fn new(db: Database, settings: &Settings) -> (Self, mpsc::UnboundedReceiver<Alert>) {
        let (tx, receiver) = mpsc::unbounded_channel();

        let scheduler = Self {
            repo: AlertRepository::new(db),
            tx: Arc::new(ParkingMutex::new(Some(tx))),
            cache: Arc::new(Mutex::new(BinaryHeap::new())),
            notify: Arc::new(Notify::new()),
            retry_delay: settings.retry_delay,
            sync_interval: settings.sync_interval,
            window: settings.window,
        };

        (scheduler, receiver)
    }

    /// Closes the delivery channel by dropping the sender, leaving anything already queued for
    /// the delivery task to drain.
    pub(super) fn close(&self) {
        self.tx.lock().take();
    }

    /// Returns a clone of the delivery sender, or `None` once the channel has been closed.
    fn sender(&self) -> Option<mpsc::UnboundedSender<Alert>> {
        self.tx.lock().clone()
    }

    /// Syncs the cache with the database, replacing it with the alerts due within the window.
    ///
    /// Only the scheduler task mutates the cache, so an alert persisted by `create` just before
    /// the sync either is part of the fetched window or is picked up by the sync woken by the
    /// create; it is never lost.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the alerts could not be fetched from the database.
    pub(super) async fn sync(&self) -> Result<usize, Error> {
        let cutoff = Utc::now() + self.window;

        let alerts = self.repo.list_due_before(cutoff).await?;
        let count = alerts.len();

        debug!(alert.count = count, "synced alerts from database");

        {
            let mut cache = self.cache.lock().await;

            *cache = alerts
                .into_iter()
                .map(|alert| Reverse(Scheduled(alert)))
                .collect();
        }

        Ok(count)
    }

    /// Creates a new alert, persisting it in the database and waking the scheduler to sync it
    /// into the cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the alert could not be inserted into the database.
    pub(super) async fn create(&self, alert: NewAlert) -> Result<Alert, Error> {
        let alert = self.repo.insert(alert).await?;

        trace!(?alert, "created alert");

        // Wakes the scheduler to sync the window, pulling the alert into the cache.
        self.notify.notify_one();

        Ok(alert)
    }

    /// Runs the scheduler loop, syncing the window of upcoming alerts from the database every
    /// `sync_interval` and delivering each alert as it becomes due.
    pub(super) async fn run(self) {
        tracing::info_span!("alert.scheduler.start").in_scope(|| {
            debug!("starting alert scheduler");
        });

        // The window is synced once at startup through `load`, so the first in-loop sync is a
        // full sync interval away.
        let mut next_sync = tokio::time::Instant::now() + self.sync_interval;

        loop {
            match self.tick().await {
                // The delivery channel is closed: the scheduler was stopped for shutdown, so
                // nothing can be delivered until the bot restarts.
                Err(Error::Closed) => break,
                Err(_) => {
                    tracing::info_span!("alert.scheduler.retry").in_scope(|| {
                        debug!("retrying scheduler tick shortly");
                    });

                    tokio::time::sleep(self.retry_delay).await;
                }
                Ok(()) => {}
            }

            if tokio::time::Instant::now() >= next_sync {
                async {
                    if let Err(err) = self.sync().await {
                        warn!(
                            exception.message = %Chain(&err),
                            "could not sync alerts from database"
                        );
                    }
                }
                .instrument(tracing::info_span!("alert.scheduler.sync"))
                .await;

                next_sync = tokio::time::Instant::now() + self.sync_interval;

                continue;
            }

            let now = Utc::now();
            let next_due = self
                .cache
                .lock()
                .await
                .peek()
                .map(|Reverse(alert)| alert.0.time);

            // How long until the window is synced again; saturates at zero, as the tick above
            // and locking the cache may take past the sync deadline.
            let sync_delay = next_sync.saturating_duration_since(tokio::time::Instant::now());

            match next_due {
                // Sleeps until the next alert is due, the window is synced, or an alert is
                // created that is due sooner.
                Some(due) if due > now => {
                    let due_delay = (due - now).to_std().unwrap_or_default();

                    tokio::select! {
                        () = tokio::time::sleep(due_delay.min(sync_delay)) => {}
                        () = self.notify.notified() => {
                            next_sync = tokio::time::Instant::now();
                        }
                    }
                }
                // The cache is empty; waits for the window to be synced or an alert to be
                // created.
                None => {
                    tokio::select! {
                        () = tokio::time::sleep(sync_delay) => {}
                        () = self.notify.notified() => {
                            next_sync = tokio::time::Instant::now();
                        }
                    }
                }
                // The next alert is already due.
                Some(_) => {}
            }
        }
    }

    /// Delivers all alerts that are due at `now`.
    ///
    /// Delivery is at-least-once: an alert is only deleted from the database after being sent,
    /// so if the deletion fails the alert is kept in the cache and delivered again once the
    /// database recovers.
    ///
    /// # Errors
    ///
    /// Returns the repository error if the alerts could not be deleted, or [`Error::Closed`] if
    /// the delivery channel is closed.
    #[instrument(name = "alert.schedule", skip_all)]
    pub(super) async fn tick(&self) -> Result<(), Error> {
        let due = self.pop_due(Utc::now()).await;

        let Some(tx) = self.sender() else {
            self.requeue(&due).await;

            return Err(Error::Closed);
        };

        if due.is_empty() {
            return Ok(());
        }

        for Scheduled(alert) in &due {
            trace!(?alert, "delivering alert");

            if tx.send(alert.clone()).is_err() {
                error!(alert.id = alert.id, "alert delivery channel is closed");

                self.requeue(&due).await;

                return Err(Error::Closed);
            }
        }

        let sent: Vec<i32> = due.iter().map(|Scheduled(alert)| alert.id).collect();

        if let Err(err) = self.repo.delete_ids(&sent).await {
            warn!(
                exception.message = %Chain(&err),
                alert.count = due.len(),
                "could not delete delivered alerts, they will be delivered again"
            );

            self.requeue(&due).await;

            return Err(err);
        }

        Ok(())
    }

    /// Pops all alerts that are due at `now` from the cache.
    async fn pop_due(&self, now: DateTime<Utc>) -> Vec<Scheduled> {
        let mut cache = self.cache.lock().await;
        let mut due = Vec::new();

        while cache
            .peek()
            .is_some_and(|Reverse(alert)| alert.0.time <= now)
        {
            due.push(cache.pop().expect("peeked").0);
        }

        drop(cache);

        due
    }

    /// Puts alerts back into the cache after a failed delivery or deletion.
    async fn requeue(&self, alerts: &[Scheduled]) {
        self.cache
            .lock()
            .await
            .extend(alerts.iter().cloned().map(Reverse));
    }
}
