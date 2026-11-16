//! Schedule alerts to be delivered in a channel at a given time.
//!
//! Alerts are created with the `.alert <message> <in|at> <datetime>` command, e.g.
//! `.alert hello world at 4:20` or `.alert another message in 10 minutes`, and support human
//! datetime expressions ("tomorrow 8pm", "next friday 10:30", "10 minutes", ..). The pending
//! alerts of a user in the current channel are listed with `.alert -l`.
//!
//! Alerts are persisted in the database, and a window of upcoming alerts is cached in memory by
//! the alert service, syncing the window from the database every few minutes. A scheduler task
//! delivers each alert to a delivery task as it becomes due, which sends it in the channel it
//! was created in; alerts are deleted from the database once sent.

mod args;
mod commands;
mod error;
mod format;
mod model;
mod repository;
mod service;
mod time;

#[cfg(test)]
mod tests;

pub use error::Error;
pub use model::{Alert, NewAlert};
pub use service::AlertService;
pub use time::ParseTimeError;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::types::chrono::Local;
use tokio::sync::mpsc;
use tracing::warn;

use zeta_plugin::database::Attribution;
use zeta_plugin::irc::{labeled_reply, parse_or_usage, send_reply};
use zeta_plugin::{
    ArgsMode, Chain, Client, CommandEvent, Context, Subscriptions, ZetaError, plugin, plugin_err,
};

use self::args::{ALERT, Opts, USAGE};
use self::format::success_message;
use self::time::{parse_time, split_args};

/// Settings for the alert plugin, from its `[plugins.alert]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// How long the scheduler waits before retrying after a failed tick.
    #[serde(with = "humantime_serde")]
    pub retry_delay: Duration,
    /// How often the window of upcoming alerts is synced from the database.
    ///
    /// Alerts are only delivered on time if the window is at least as large as this interval,
    /// as the last sync before an alert is due can be a full interval earlier.
    #[serde(with = "humantime_serde")]
    pub sync_interval: Duration,
    /// How far ahead of their due time alerts are kept in memory.
    ///
    /// Should be at least [`Settings::sync_interval`], so every alert is cached before it is
    /// due.
    #[serde(with = "humantime_serde")]
    pub window: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            retry_delay: Duration::from_secs(30),
            sync_interval: Duration::from_mins(5),
            window: Duration::from_mins(15),
        }
    }
}

/// Alert plugin.
///
/// Lets users schedule alerts for themselves with the `.alert <message> <in|at> <datetime>`
/// command; a scheduler task delivers due alerts to a delivery task, started in [`loaded`],
/// which sends them in the channel the alert was created in.
///
/// The alert service is published to [`Context::shared`], so other plugins can schedule alerts
/// through [`AlertService::create`].
///
/// [`loaded`]: zeta_plugin::Plugin::loaded
pub struct AlertPlugin {
    /// The alert service, also published for other plugins to use.
    service: Arc<AlertService>,
    /// The receiver of due alerts, moved into the delivery task on load.
    receiver: Option<mpsc::UnboundedReceiver<Alert>>,
    /// The scheduler task, stopped before the delivery channel is closed on shutdown.
    scheduler: Option<tokio::task::JoinHandle<()>>,
    /// The delivery task, drained on shutdown so it drops its IRC sender.
    delivery: Option<tokio::task::JoinHandle<()>>,
}

// Manual implementation: the service is summarized by its own redacted view, and the tasks are
// shown by whether they are running.
impl fmt::Debug for AlertPlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlertPlugin")
            .field("service", &self.service)
            .field("receiver", &self.receiver.is_some())
            .field("scheduler", &self.scheduler.is_some())
            .field("delivery", &self.delivery.is_some())
            .finish_non_exhaustive()
    }
}

#[plugin(name = "alert", settings = Settings)]
impl AlertPlugin {
    fn new(
        ctx: &Context,
        settings: &Settings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, ZetaError> {
        subscriptions.command(ALERT);

        let mut service = AlertService::new(ctx.db().clone(), settings);
        let receiver = service.take_receiver();
        let service = Arc::new(service);

        // Intentional: other plugins register alerts through the shared service.
        ctx.shared().publish(Arc::clone(&service));

        Ok(Self {
            service,
            receiver,
            scheduler: None,
            delivery: None,
        })
    }

    #[handler(loaded)]
    async fn loaded(&mut self, _ctx: &Context, client: &Client) -> Result<(), ZetaError> {
        // A transient database failure must not leave the cache empty for the run: without the
        // retry, every alert due within the window is skipped until the next restart.
        zeta_plugin::database::load_with_retry(|| self.service.load())
            .await
            .map_err(plugin_err)?;

        self.scheduler = Some(self.service.start_scheduler());

        if let Some(receiver) = self.receiver.take() {
            self.delivery = Some(Self::start_delivery(client, receiver));
        }

        Ok(())
    }

    #[handler(shutdown)]
    async fn shutdown(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        // Stop the producer first, then close the channel: the delivery task drains queued
        // alerts and exits, dropping its IRC sender before the host drops the client.
        if let Some(scheduler) = self.scheduler.take() {
            scheduler.abort();
            let _ = scheduler.await;
        }

        self.service.close();

        if let Some(delivery) = self.delivery.take() {
            let _ = delivery.await;
        }

        Ok(())
    }

    #[handler(command)]
    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();

        let Some(sender) = command.sender() else {
            return Ok(());
        };

        let nickname = sender.nick;

        let Some(opts) = parse_or_usage::<Opts>(client, command, ArgsMode::Words, |line| {
            labeled_reply("Alert", line)
        })?
        else {
            return Ok(());
        };

        if opts.list {
            return self.list_alerts(client, channel, nickname, &opts).await;
        }

        let input = opts.input();

        let Some((message, time_spec)) = split_args(&input) else {
            send_reply(client, channel, labeled_reply("Alert", USAGE))?;

            return Ok(());
        };

        let time = match parse_time(time_spec, Local::now()) {
            Ok(time) => time,
            Err(err) => {
                send_reply(client, channel, labeled_reply("Alert", err.to_string()))?;

                return Ok(());
            }
        };

        let alert = NewAlert {
            attribution: Attribution::from_sender(sender, channel),
            message: message.to_owned(),
            time,
        };

        match self.service.create(alert).await {
            Ok(_) => {
                let local = time.with_timezone(&Local).format("%d/%m/%Y %H:%M:%S");

                send_reply(
                    client,
                    channel,
                    labeled_reply(
                        "Alert",
                        format_args!("{} Alert stored for\x0f {local}.", success_message()),
                    ),
                )?;
            }
            Err(err) => {
                warn!(exception.message = %Chain(&err), "could not store alert");
                send_reply(
                    client,
                    channel,
                    labeled_reply("Alert", "could not store the alert"),
                )?;
            }
        }

        Ok(())
    }
}
