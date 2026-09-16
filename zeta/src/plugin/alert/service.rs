//! Alert service, persisting alerts in the database and delivering due alerts to a subscriber.

use std::{
    cmp::{Ordering, Reverse},
    collections::BinaryHeap,
    sync::Arc,
    time::Duration,
};

use sqlx::types::chrono::{DateTime, Utc};
use tokio::sync::{Mutex, Notify, mpsc};
use tracing::{debug, error, instrument, trace, warn};

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
    /// All pending alerts, ordered by the time they are due.
    cache: Arc<Mutex<BinaryHeap<Reverse<Scheduled>>>>,
    /// Wakes the scheduler when an alert is created.
    notify: Arc<Notify>,
    /// How long the scheduler waits before retrying after a failed tick.
    retry_delay: Duration,
    /// The maximum number of pending alerts a user may have.
    max_pending_per_user: usize,
}

/// Stores alerts in the database, caching them in memory and delivering due alerts over an
/// unbounded [`tokio::sync::mpsc`] channel.
///
/// All pending alerts are cached in a min-heap ordered by due time; the database remains the
/// source of truth for crash recovery. The scheduler task sleeps until the next alert is due,
/// sends it to the delivery task, and deletes it from the cache and the database once sent.
/// Delivery is at-least-once: alerts are only deleted after being sent, so a failed deletion
/// results in a duplicate delivery. The plugin holds the receiver and delivers the alerts it
/// receives over IRC.
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
                max_pending_per_user: settings.max_pending_per_user,
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

    /// Loads all pending alerts from the database into the cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Load`] if the alerts could not be fetched from the database.
    #[instrument(skip_all, err)]
    pub async fn load(&self) -> Result<(), Error> {
        self.scheduler.load().await
    }

    /// Creates a new alert, persisting it in the database and scheduling it for delivery.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Insert`] if the alert could not be inserted into the database.
    #[instrument(skip_all, err)]
    pub async fn create(&self, alert: NewAlert) -> Result<Alert, Error> {
        self.scheduler.create(alert).await
    }

    /// Returns the pending alerts of `nickname` in `channel`, ordered by the time they are due.
    #[instrument(skip_all)]
    pub async fn pending_for(&self, channel: &str, nickname: &str) -> Vec<Alert> {
        self.scheduler.pending_for(channel, nickname).await
    }

    /// Spawns the scheduler task, delivering due alerts until the runtime shuts down.
    pub fn start_scheduler(&self) {
        let scheduler = self.scheduler.clone();

        tokio::spawn(async move {
            debug!("starting alert scheduler");

            scheduler.run().await;
        });
    }
}

impl Scheduler {
    /// Loads all alerts in the database into the cache.
    async fn load(&self) -> Result<(), Error> {
        let alerts: BinaryHeap<Reverse<Scheduled>> = self
            .repo
            .list()
            .await?
            .into_iter()
            .map(|alert| Reverse(Scheduled(alert)))
            .collect();

        debug!(count = alerts.len(), "loaded alerts into cache");

        // Merges instead of replacing, so an alert persisted by `create` while the query ran is
        // not evicted from the cache.
        self.cache.lock().await.extend(alerts);

        Ok(())
    }

    /// Creates a new alert, persisting it in the database and scheduling it for delivery.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TooManyPending`] if the user already has the maximum number of pending
    /// alerts, or [`Error::Insert`] if the alert could not be inserted into the database.
    async fn create(&self, alert: NewAlert) -> Result<Alert, Error> {
        if self.max_pending_per_user > 0 {
            let pending = self
                .cache
                .lock()
                .await
                .iter()
                .filter(|Reverse(scheduled)| scheduled.0.nickname == alert.nickname)
                .count();

            if pending >= self.max_pending_per_user {
                return Err(Error::TooManyPending(self.max_pending_per_user));
            }
        }

        let alert = self.repo.insert(alert).await?;

        trace!(?alert, "scheduling alert");

        self.cache
            .lock()
            .await
            .push(Reverse(Scheduled(alert.clone())));

        // Wakes the scheduler, as the alert may be due sooner than the one it is waiting for.
        self.notify.notify_one();

        Ok(alert)
    }

    /// Returns the pending alerts of `nickname` in `channel`, ordered by the time they are due.
    async fn pending_for(&self, channel: &str, nickname: &str) -> Vec<Alert> {
        let mut pending = self
            .cache
            .lock()
            .await
            .iter()
            .filter(|Reverse(scheduled)| {
                scheduled.0.channel == channel && scheduled.0.nickname == nickname
            })
            .map(|Reverse(scheduled)| scheduled.0.clone())
            .collect::<Vec<_>>();

        pending.sort_by_key(|alert| (alert.time, alert.id));

        pending
    }

    /// Runs the scheduler loop, delivering each alert as it becomes due.
    async fn run(self) {
        loop {
            let now = Utc::now();
            let next_due = self
                .cache
                .lock()
                .await
                .peek()
                .map(|Reverse(alert)| alert.0.time);

            match next_due {
                // Sleeps until the next alert is due, or wakes early if an alert is created
                // that is due sooner.
                Some(due) if due > now => {
                    let delay = (due - now).to_std().unwrap_or_default();

                    tokio::select! {
                        () = tokio::time::sleep(delay) => {}
                        () = self.notify.notified() => continue,
                    }
                }
                // The cache is empty; waits for the next alert to be created.
                None => {
                    self.notify.notified().await;

                    continue;
                }
                // The next alert is already due.
                Some(_) => {}
            }

            if self.tick().await.is_err() {
                debug!("retrying scheduler tick shortly");

                tokio::time::sleep(self.retry_delay).await;
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

    /// Skips the test if a test database has not been configured.
    async fn test_service() -> Option<(AlertService, mpsc::UnboundedReceiver<Alert>, Database)> {
        let url = std::env::var("ZETA_TEST_DATABASE_URL").ok()?;

        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("could not connect to the test database");

        let mut service = AlertService::new(db.clone(), &Settings::default());
        let receiver = service.take_receiver();

        Some((service, receiver.unwrap(), db))
    }

    fn new_alert(message: &str, time: DateTime<Utc>) -> NewAlert {
        NewAlert {
            nickname: "smoke".into(),
            username: "smoke".into(),
            hostname: "smoke".into(),
            channel: "#smoke".into(),
            message: message.into(),
            time,
        }
    }

    #[tokio::test]
    async fn scheduler_delivers_created_alerts() {
        let Some((service, mut receiver, db)) = test_service().await else {
            return;
        };

        // Removes alerts left behind by earlier runs, so the cache starts clean.
        sqlx::query("DELETE FROM alerts WHERE nickname = 'smoke'")
            .execute(&db)
            .await
            .unwrap();

        service.load().await.unwrap();
        service.start_scheduler();

        // The scheduler wakes for an alert created while its cache is empty.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let now = Utc::now();
        let first = service
            .create(new_alert("first", now + TimeDelta::try_seconds(1).unwrap()))
            .await
            .unwrap();
        let second = service
            .create(new_alert(
                "second",
                now + TimeDelta::try_seconds(3).unwrap(),
            ))
            .await
            .unwrap();

        // The alerts are delivered in order, roughly when they are due.
        for expected in [&first, &second] {
            let delivered = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
                .await
                .unwrap()
                .unwrap();

            assert_eq!(delivered.id, expected.id);

            let latency = Utc::now()
                .signed_duration_since(expected.time)
                .to_std()
                .unwrap();
            assert!(latency < Duration::from_secs(2), "latency {latency:?}");
        }

        // The alerts are deleted from the database once delivered.
        tokio::time::sleep(Duration::from_millis(250)).await;

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE id = ANY($1)")
            .bind([first.id, second.id])
            .fetch_one(&db)
            .await
            .unwrap();

        assert_eq!(remaining, 0);
    }

    /// A pending alert fixture.
    fn pending_alert(id: i32, channel: &str, nickname: &str, time: DateTime<Utc>) -> Alert {
        Alert {
            id,
            nickname: nickname.to_owned(),
            username: nickname.to_owned(),
            hostname: nickname.to_owned(),
            channel: channel.to_owned(),
            message: format!("{nickname} in {channel}"),
            time,
            created_at: time,
        }
    }

    #[tokio::test]
    async fn pending_for_filters_by_channel_and_nickname() {
        // The pool is lazy, so the test runs without a database.
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgresql://localhost/zeta")
            .expect("could not create a lazy database pool");

        let service = AlertService::new(db, &Settings::default());
        let now = Utc::now();

        let sooner = pending_alert(1, "#smoke", "smoke", now + TimeDelta::try_minutes(5).unwrap());
        let later = pending_alert(2, "#smoke", "smoke", now + TimeDelta::try_minutes(10).unwrap());
        let other = pending_alert(3, "#smoke", "ash", now + TimeDelta::try_minutes(1).unwrap());
        let elsewhere =
            pending_alert(4, "#other", "smoke", now + TimeDelta::try_minutes(1).unwrap());

        service.scheduler.cache.lock().await.extend([
            Reverse(Scheduled(later.clone())),
            Reverse(Scheduled(other)),
            Reverse(Scheduled(elsewhere)),
            Reverse(Scheduled(sooner.clone())),
        ]);

        let pending = service.pending_for("#smoke", "smoke").await;

        assert_eq!(pending, vec![sooner, later]);
    }
}
