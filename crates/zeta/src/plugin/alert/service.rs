//! Alert service, persisting alerts in the database and delivering due alerts to a subscriber.

use std::{
    cmp::{Ordering, Reverse},
    collections::BinaryHeap,
    sync::Arc,
    time::Duration,
};

use sqlx::types::chrono::{DateTime, Utc};
use tokio::sync::{Mutex, Notify, mpsc};
use tracing::{Instrument, debug, error, instrument, trace, warn};

use super::{
    Settings,
    error::Error,
    model::{Alert, NewAlert},
    repository::AlertRepository,
};
use crate::database::Database;

/// An alert scheduled for delivery, ordered by the time it is due.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Scheduled(Alert);

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
struct Scheduler {
    /// The alert repository.
    repo: AlertRepository,
    /// The sender half of the channel used to deliver due alerts.
    tx: mpsc::UnboundedSender<Alert>,
    /// The alerts due within the window, ordered by the time they are due.
    cache: Arc<Mutex<BinaryHeap<Reverse<Scheduled>>>>,
    /// Wakes the scheduler to sync the window when an alert is created.
    notify: Arc<Notify>,
    /// How long the scheduler waits before retrying after a failed tick.
    retry_delay: Duration,
    /// How often the window of upcoming alerts is synced from the database.
    sync_interval: Duration,
    /// How far ahead of their due time alerts are kept in the cache.
    window: Duration,
}

/// Stores alerts in the database, caching a window of upcoming alerts in memory and delivering
/// due alerts over an unbounded [`tokio::sync::mpsc`] channel.
///
/// The cache holds only the alerts due within the next window (15 minutes by default), synced
/// from the database every few minutes; the database remains the source of truth for crash
/// recovery. The scheduler task sleeps until the next cached alert is due, sends it to the
/// delivery task, and deletes it from the database once sent. Delivery is at-least-once: alerts
/// are only deleted after being sent, so a failed deletion results in a duplicate delivery. The
/// plugin holds the receiver and delivers the alerts it receives over IRC.
pub struct AlertService {
    /// The state shared with the scheduler task.
    scheduler: Scheduler,
    /// The receiver of due alerts, taken by the delivery task on load.
    receiver: Option<mpsc::UnboundedReceiver<Alert>>,
}

impl AlertService {
    /// Creates a new alert service backed by the given database pool.
    #[must_use]
    pub fn new(db: Database, settings: &Settings) -> Self {
        let (tx, receiver) = mpsc::unbounded_channel();

        Self {
            scheduler: Scheduler {
                repo: AlertRepository::new(db),
                tx,
                cache: Arc::new(Mutex::new(BinaryHeap::new())),
                notify: Arc::new(Notify::new()),
                retry_delay: settings.retry_delay,
                sync_interval: settings.sync_interval,
                window: settings.window,
            },
            receiver: Some(receiver),
        }
    }

    /// Takes the receiver of due alerts, to be handed to the delivery task.
    ///
    /// Returns `None` if the receiver has already been taken.
    #[must_use]
    pub const fn take_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<Alert>> {
        self.receiver.take()
    }

    /// Loads the alerts due within the window from the database into the cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the alerts could not be fetched from the database.
    #[instrument(skip_all, err)]
    pub async fn load(&self) -> Result<(), Error> {
        self.scheduler.sync().await.map(|_| ())
    }

    /// Creates a new alert, persisting it in the database and waking the scheduler to sync it
    /// into the cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the alert could not be inserted into the database.
    #[instrument(skip_all, err)]
    pub async fn create(&self, alert: NewAlert) -> Result<Alert, Error> {
        self.scheduler.create(alert).await
    }

    /// Returns the pending alerts of `nickname` in `channel`, ordered by the time they are due.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the alerts could not be fetched from the database.
    #[instrument(skip_all, err)]
    pub async fn pending_for(&self, channel: &str, nickname: &str) -> Result<Vec<Alert>, Error> {
        self.scheduler.pending_for(channel, nickname).await
    }

    /// Spawns the scheduler task, delivering due alerts until the runtime shuts down.
    pub fn start_scheduler(&self) {
        let scheduler = self.scheduler.clone();

        tokio::spawn(
            async move {
                scheduler.run().await;
            }
            .instrument(tracing::info_span!("alert_scheduler")),
        );
    }
}

impl Scheduler {
    /// Syncs the cache with the database, replacing it with the alerts due within the window.
    ///
    /// Only the scheduler task mutates the cache, so an alert persisted by `create` just before
    /// the sync either is part of the fetched window or is picked up by the sync woken by the
    /// create; it is never lost.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the alerts could not be fetched from the database.
    async fn sync(&self) -> Result<usize, Error> {
        let cutoff = Utc::now() + self.window;

        let alerts = self.repo.list_due_before(cutoff).await?;
        let count = alerts.len();

        debug!(count, "synced alerts from database");

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
    async fn create(&self, alert: NewAlert) -> Result<Alert, Error> {
        let alert = self.repo.insert(alert).await?;

        trace!(?alert, "created alert");

        // Wakes the scheduler to sync the window, pulling the alert into the cache.
        self.notify.notify_one();

        Ok(alert)
    }

    /// Returns the pending alerts of `nickname` in `channel`, ordered by the time they are due.
    ///
    /// The alerts are read from the database, as the cache only holds the window of alerts due
    /// in the near future.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the alerts could not be fetched from the database.
    async fn pending_for(&self, channel: &str, nickname: &str) -> Result<Vec<Alert>, Error> {
        self.repo.list_for(channel, nickname).await
    }

    /// Runs the scheduler loop, syncing the window of upcoming alerts from the database every
    /// `sync_interval` and delivering each alert as it becomes due.
    async fn run(self) {
        debug!("starting alert scheduler");

        // The window is synced once at startup through `load`, so the first in-loop sync is a
        // full sync interval away.
        let mut next_sync = tokio::time::Instant::now() + self.sync_interval;

        loop {
            match self.tick().await {
                // The delivery channel is closed: the plugin's delivery task has stopped, so
                // nothing can be delivered until the bot restarts.
                Err(Error::Closed) => break,
                Err(_) => {
                    debug!("retrying scheduler tick shortly");

                    tokio::time::sleep(self.retry_delay).await;
                }
                Ok(()) => {}
            }

            if tokio::time::Instant::now() >= next_sync {
                if let Err(err) = self.sync().await {
                    warn!(?err, "could not sync alerts from database");
                }

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
    async fn tick(&self) -> Result<(), Error> {
        let due = self.pop_due(Utc::now()).await;

        if due.is_empty() {
            return Ok(());
        }

        for Scheduled(alert) in &due {
            trace!(?alert, "delivering alert");

            if self.tx.send(alert.clone()).is_err() {
                error!(alert_id = alert.id, "alert delivery channel is closed");

                self.requeue(&due).await;

                return Err(Error::Closed);
            }
        }

        let sent: Vec<i32> = due.iter().map(|Scheduled(alert)| alert.id).collect();

        if let Err(err) = self.repo.delete_all(&sent).await {
            warn!(
                ?err,
                count = due.len(),
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::TimeDelta;

    use super::*;

    /// A new alert fixture.
    fn new_alert(nickname: &str, channel: &str, message: &str, time: DateTime<Utc>) -> NewAlert {
        NewAlert {
            nickname: nickname.into(),
            username: nickname.into(),
            hostname: nickname.into(),
            channel: channel.into(),
            message: message.into(),
            time,
        }
    }

    #[sqlx::test]
    async fn scheduler_delivers_created_alerts(db: sqlx::PgPool) {
        let mut service = AlertService::new(db.clone(), &Settings::default());
        let mut receiver = service.take_receiver().unwrap();

        service.load().await.unwrap();
        service.start_scheduler();

        // The scheduler wakes for an alert created while its cache is empty.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let now = Utc::now();
        let first = service
            .create(new_alert(
                "smoke",
                "#smoke",
                "first",
                now + TimeDelta::try_seconds(1).unwrap(),
            ))
            .await
            .unwrap();
        let second = service
            .create(new_alert(
                "smoke",
                "#smoke",
                "second",
                now + TimeDelta::try_seconds(3).unwrap(),
            ))
            .await
            .unwrap();

        // The alerts are delivered in order, roughly when they are due. Only the order is
        // asserted: a latency bound would flake on loaded machines.
        for expected in [&first, &second] {
            let delivered = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
                .await
                .unwrap()
                .unwrap();

            assert_eq!(delivered.id, expected.id);
        }

        // The alerts are deleted from the database once delivered. The deletion is not
        // synchronous with the delivery, so poll until it lands — with a deadline rather than
        // a fixed sleep, which would either flake on loaded machines or slow the happy path.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

        loop {
            let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE id = ANY($1)")
                .bind([first.id, second.id])
                .fetch_one(&db)
                .await
                .unwrap();

            if remaining == 0 {
                break;
            }

            assert!(
                tokio::time::Instant::now() < deadline,
                "the alerts were not deleted after delivery: {remaining} remaining"
            );

            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[sqlx::test]
    async fn pending_for_filters_by_channel_and_nickname(db: sqlx::PgPool) {
        let service = AlertService::new(db, &Settings::default());

        let now = Utc::now();

        let sooner = service
            .create(new_alert(
                "lister",
                "#smoke",
                "sooner",
                now + TimeDelta::try_minutes(5).unwrap(),
            ))
            .await
            .unwrap();
        let later = service
            .create(new_alert(
                "lister",
                "#smoke",
                "later",
                now + TimeDelta::try_minutes(10).unwrap(),
            ))
            .await
            .unwrap();
        let _other = service
            .create(new_alert(
                "other",
                "#smoke",
                "other",
                now + TimeDelta::try_minutes(1).unwrap(),
            ))
            .await
            .unwrap();
        let _elsewhere = service
            .create(new_alert(
                "lister",
                "#other",
                "elsewhere",
                now + TimeDelta::try_minutes(1).unwrap(),
            ))
            .await
            .unwrap();

        let pending = service.pending_for("#smoke", "lister").await.unwrap();

        assert_eq!(pending, vec![sooner, later]);
    }

    #[sqlx::test]
    async fn sync_only_caches_alerts_within_the_window(db: sqlx::PgPool) {
        let service = AlertService::new(db, &Settings::default());

        let now = Utc::now();

        let inside = service
            .create(new_alert(
                "window",
                "#smoke",
                "inside",
                now + TimeDelta::try_minutes(5).unwrap(),
            ))
            .await
            .unwrap();
        let _outside = service
            .create(new_alert(
                "window",
                "#smoke",
                "outside",
                now + TimeDelta::try_hours(1).unwrap(),
            ))
            .await
            .unwrap();

        // The scheduler is not running, so the cache is only populated by the sync.
        service.load().await.unwrap();

        let cached = service.scheduler.cache.lock().await;

        assert_eq!(
            cached
                .iter()
                .filter(|Reverse(scheduled)| scheduled.0.nickname == "window")
                .map(|Reverse(scheduled)| scheduled.0.id)
                .collect::<Vec<_>>(),
            vec![inside.id]
        );

        drop(cached);
    }
}
