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

mod command;
mod error;
mod model;
mod reply;
mod repository;
mod service;
mod time;

// The module types are re-exported as part of the module's API surface, even though the plugin
// itself only handles them by value.
pub use time::ParseTimeError;
#[allow(unused_imports)]
pub use {
    error::Error,
    model::{Alert, NewAlert},
    service::AlertService,
};

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::types::chrono::Local;
use tokio::sync::mpsc;
use tracing::{Instrument, debug, error, trace};

use self::command::{ALERT, Opts, USAGE};
use self::reply::{format_pending, success_message};
use self::time::{parse_time, split_args};
use crate::prelude::*;

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
/// [`loaded`]: Plugin::loaded
pub struct AlertPlugin {
    /// The alert service, also published for other plugins to use.
    service: Arc<AlertService>,
    /// The receiver of due alerts, moved into the delivery task on load.
    receiver: Option<mpsc::UnboundedReceiver<Alert>>,
}

impl AlertPlugin {
    /// Spawns the delivery task that sends due alerts over IRC as they are delivered by the
    /// scheduler.
    fn start_delivery(client: &Client, mut receiver: mpsc::UnboundedReceiver<Alert>) {
        let sender = client.sender();

        tokio::spawn(
            async move {
                debug!("starting alert delivery task");

                while let Some(alert) = receiver.recv().await {
                    trace!(?alert, "delivering alert");

                    if let Err(err) = sender.send_privmsg(
                        &alert.channel,
                        format!("{}: {}", alert.nickname, alert.message),
                    ) {
                        error!(?err, alert_id = alert.id, "could not deliver alert");
                    }
                }

                debug!("alert delivery channel is closed");
            }
            .instrument(tracing::info_span!("alert_delivery")),
        );
    }
}

#[async_trait]
impl Plugin<Context> for AlertPlugin {
    type Settings = Settings;

    fn new(
        ctx: &Context,
        settings: &Settings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, ZetaError> {
        subscriptions.command(ALERT);

        let mut service = AlertService::new(ctx.db.clone(), settings);
        let receiver = service.take_receiver();
        let service = Arc::new(service);

        ctx.shared.publish(Arc::clone(&service));

        Ok(Self { service, receiver })
    }

    async fn loaded(&mut self, _ctx: &Context, client: &Client) -> Result<(), ZetaError> {
        self.service.load().await.map_err(plugin_err)?;
        self.service.start_scheduler();

        if let Some(receiver) = self.receiver.take() {
            Self::start_delivery(client, receiver);
        }

        Ok(())
    }

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

        let (nickname, username, hostname) = (sender.nick, sender.username, sender.hostname);

        let Some(opts) =
            parse_words_or_usage::<Opts>(client, command, |line| reply("Alert", line))?
        else {
            return Ok(());
        };

        if opts.list {
            if !opts.args.is_empty() {
                client.send_privmsg(channel, reply("Alert", USAGE))?;

                return Ok(());
            }

            let pending = match self.service.pending_for(channel, nickname).await {
                Ok(pending) => pending,
                Err(err) => {
                    error!(?err, "could not list pending alerts");

                    client.send_privmsg(
                        channel,
                        reply("Alert", "could not list your pending alerts"),
                    )?;

                    return Ok(());
                }
            };

            client.send_privmsg(channel, format_pending(&pending))?;

            return Ok(());
        }

        let input = opts.input();

        let Some((message, time_spec)) = split_args(&input) else {
            client.send_privmsg(channel, reply("Alert", USAGE))?;

            return Ok(());
        };

        let time = match parse_time(time_spec, Local::now()) {
            Ok(time) => time,
            Err(err) => {
                client.send_privmsg(channel, reply("Alert", err.to_string()))?;

                return Ok(());
            }
        };

        let alert = NewAlert {
            nickname: nickname.to_owned(),
            username: username.to_owned(),
            hostname: hostname.to_owned(),
            channel: channel.to_owned(),
            message: message.to_owned(),
            time,
        };

        match self.service.create(alert).await {
            Ok(_) => {
                let local = time.with_timezone(&Local).format("%d/%m/%Y %H:%M:%S");

                client.send_privmsg(
                    channel,
                    reply(
                        "Alert",
                        format!("{} Alert stored for\x0f {local}.", success_message()),
                    ),
                )?;
            }
            Err(err) => {
                error!(?err, "could not store alert");
                client.send_privmsg(channel, reply("Alert", "could not store the alert"))?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert_eq!(settings.retry_delay, Duration::from_secs(30));
            assert_eq!(settings.sync_interval, Duration::from_mins(5));
            assert_eq!(settings.window, Duration::from_mins(15));
        }
        deserialize: {
            "retry_delay": "1m",
            "sync_interval": "2m",
            "window": "30m",
        } assert: {
            assert_eq!(settings.retry_delay, Duration::from_mins(1));
            assert_eq!(settings.sync_interval, Duration::from_mins(2));
            assert_eq!(settings.window, Duration::from_mins(30));
        }
    }
}
