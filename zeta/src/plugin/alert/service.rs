//! Alert service, persisting alerts in the database and broadcasting due alerts to a subscriber.

use std::time::Duration;

use tokio::sync::broadcast;
use tracing::{debug, error, instrument, warn};

use super::{
    error::Error,
    model::{Alert, NewAlert},
    repository::AlertRepository,
};
use crate::database::Database;

/// How often the scheduler checks the database for due alerts.
const SCHEDULER_INTERVAL: Duration = Duration::from_secs(20);

/// The capacity of the broadcast channel used to deliver due alerts.
const BROADCAST_CAPACITY: usize = 256;

/// Stores alerts in the database and broadcasts due alerts on a [`tokio::sync::broadcast`]
/// channel.
///
/// The scheduler task owns the channel's sender and broadcasts every alert as it becomes due,
/// deleting it from the database once broadcast. The plugin holds a receiver and delivers the
/// alerts it receives over IRC.
pub struct AlertService {
    /// The alert repository.
    repo: AlertRepository,
    /// The sender half of the broadcast channel.
    tx: broadcast::Sender<Alert>,
}

impl AlertService {
    /// Creates a new alert service backed by the given database pool.
    #[must_use]
    pub fn new(db: Database) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);

        Self {
            repo: AlertRepository::new(db),
            tx,
        }
    }

    /// Returns a new receiver for due alerts.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Alert> {
        self.tx.subscribe()
    }

    /// Creates a new alert, persisting it in the database.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Insert`] if the alert could not be inserted into the database.
    #[instrument(skip_all, err)]
    pub async fn create(&self, alert: NewAlert) -> Result<Alert, Error> {
        self.repo.insert(alert).await
    }

    /// Spawns the scheduler task, broadcasting due alerts until the runtime shuts down.
    pub fn start_scheduler(&self) {
        let repo = self.repo.clone();
        let tx = self.tx.clone();

        tokio::spawn(async move {
            debug!("starting alert scheduler");

            let mut ticker = tokio::time::interval(SCHEDULER_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                ticker.tick().await;

                if let Err(err) = tick(&repo, &tx).await {
                    error!(?err, "scheduler tick failed");
                }
            }
        });
    }
}

/// Broadcasts all due alerts on `tx` and deletes them from the database once sent.
///
/// # Errors
///
/// Returns the repository error if the due alerts could not be fetched or deleted.
async fn tick(repo: &AlertRepository, tx: &broadcast::Sender<Alert>) -> Result<(), Error> {
    let alerts = repo.list_due().await?;

    if alerts.is_empty() {
        return Ok(());
    }

    let mut sent_ids = Vec::with_capacity(alerts.len());

    for alert in alerts {
        debug!(?alert, "broadcasting alert");

        let id = alert.id;

        match tx.send(alert) {
            Ok(_) => sent_ids.push(id),
            Err(broadcast::error::SendError(alert)) => {
                warn!(alert_id = alert.id, "no active subscribers, dropping alert");
            }
        }
    }

    if !sent_ids.is_empty() {
        repo.delete_all(&sent_ids).await?;
    }

    Ok(())
}
