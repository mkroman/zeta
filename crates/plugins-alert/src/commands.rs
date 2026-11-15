//! The `.alert` listing handler and the task that delivers due alerts.

use tokio::sync::mpsc;
use tracing::{Instrument, debug, error, trace, warn};

use zeta_plugin::irc::{labeled_reply, send_reply, send_reply_via, unstyle_text};
use zeta_plugin::{Chain, Client, ZetaError};

use super::args::{Opts, USAGE};
use super::format::format_pending;
use super::{Alert, AlertPlugin};

impl AlertPlugin {
    /// Replies to `.alert -l` with the sender's pending alerts in `channel`.
    pub(super) async fn list_alerts(
        &self,
        client: &Client,
        channel: &str,
        nickname: &str,
        opts: &Opts,
    ) -> Result<(), ZetaError> {
        if !opts.args.is_empty() {
            send_reply(client, channel, labeled_reply("Alert", USAGE))?;

            return Ok(());
        }

        let pending = match self.service.pending_for(channel, nickname).await {
            Ok(pending) => pending,
            Err(err) => {
                warn!(exception.message = %Chain(&err), "could not list pending alerts");

                send_reply(
                    client,
                    channel,
                    labeled_reply("Alert", "could not list your pending alerts"),
                )?;

                return Ok(());
            }
        };

        send_reply(client, channel, format_pending(&pending))?;

        Ok(())
    }

    /// Spawns the delivery task that sends due alerts over IRC as they are delivered by the
    /// scheduler.
    pub(super) fn start_delivery(
        client: &Client,
        mut receiver: mpsc::UnboundedReceiver<Alert>,
    ) -> tokio::task::JoinHandle<()> {
        let sender = client.sender();

        tokio::spawn(async move {
            tracing::info_span!("alert.delivery.start").in_scope(|| {
                debug!("starting alert delivery task");
            });

            while let Some(alert) = receiver.recv().await {
                let span = tracing::info_span!(
                    "alert.deliver",
                    alert.id = alert.id,
                    irc.channel = %alert.channel,
                );

                async {
                    trace!(?alert, "delivering alert");

                    let message = unstyle_text(&alert.message);

                    if let Err(err) = send_reply_via(
                        &sender,
                        &alert.channel,
                        format!("{}: {message}", alert.nickname),
                    ) {
                        error!(exception.message = %Chain(&err), "could not deliver alert");
                    }
                }
                .instrument(span)
                .await;
            }

            tracing::info_span!("alert.delivery.stop").in_scope(|| {
                debug!("alert delivery channel is closed");
            });
        })
    }
}
