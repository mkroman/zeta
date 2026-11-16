//! Alert service, persisting alerts in the database and delivering due alerts to a subscriber.
//!
//! The module is split by concern: `scheduler` holds the queue of scheduled alerts, the
//! scheduler task and its delivery loop, and `tests` covers the service; this module holds the
//! public [`AlertService`] and the methods the plugin calls on it.

mod scheduler;
#[cfg(test)]
mod tests;

use std::fmt;

use tokio::sync::mpsc;
use tracing::instrument;

use self::scheduler::Scheduler;
use super::{
    Settings,
    error::Error,
    model::{Alert, NewAlert},
};
use zeta_plugin::database::Database;

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

// Manual implementation: the repository holds the database pool and the cache its queue of due
// alerts, so only the scheduling configuration and the receiver state are shown.
impl fmt::Debug for AlertService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlertService")
            .field("receiver", &self.receiver.is_some())
            .field("retry_delay", &self.scheduler.retry_delay)
            .field("sync_interval", &self.scheduler.sync_interval)
            .field("window", &self.scheduler.window)
            .finish_non_exhaustive()
    }
}

impl AlertService {
    /// Creates a new alert service backed by the given database pool.
    #[must_use]
    pub fn new(db: Database, settings: &Settings) -> Self {
        let (scheduler, receiver) = Scheduler::new(db, settings);

        Self {
            scheduler,
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
    #[instrument(name = "alert.load", skip_all, err)]
    pub async fn load(&self) -> Result<(), Error> {
        self.scheduler.sync().await.map(|_| ())
    }

    /// Creates a new alert, persisting it in the database and waking the scheduler to sync it
    /// into the cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the alert could not be inserted into the database.
    #[instrument(name = "alert.create", skip_all, err)]
    pub async fn create(&self, alert: NewAlert) -> Result<Alert, Error> {
        self.scheduler.create(alert).await
    }

    /// Returns the pending alerts of `nickname` in `channel`, ordered by the time they are due.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the alerts could not be fetched from the database.
    #[instrument(name = "alert.pending", skip_all, err)]
    pub async fn pending_for(&self, channel: &str, nickname: &str) -> Result<Vec<Alert>, Error> {
        self.scheduler.repo.list_for(channel, nickname).await
    }

    /// Spawns the scheduler task, delivering due alerts until the runtime shuts down.
    #[must_use]
    pub fn start_scheduler(&self) -> tokio::task::JoinHandle<()> {
        let scheduler = self.scheduler.clone();

        tokio::spawn(async move {
            scheduler.run().await;
        })
    }

    /// Closes the delivery channel: the delivery task drains what is queued, then exits and
    /// drops its IRC sender.
    pub(crate) fn close(&self) {
        self.scheduler.close();
    }
}
