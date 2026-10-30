//! The long-lived task that runs one plugin.

use std::sync::Arc;
use std::sync::OnceLock;

use irc::client::Client;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{Instrument, Span, debug, error, warn};

use zeta_plugin::{Context, Plugin, handle_event};

use crate::dispatch::Envelope;

/// A plugin running in its own long-lived task.
///
/// The task loads the plugin and then waits for events on an unbounded channel, handling them
/// one at a time. State that other plugins should be able to access must be published to
/// [`Context::shared`] when the plugin is constructed or loaded.
///
/// Closing the mailbox — by dropping its senders, which the dispatcher holds — makes the plugin
/// drain its queued events, run the shutdown hook and exit.
pub struct PluginTask {
    /// The name of the plugin, used for logging.
    pub name: &'static str,
    /// The handle of the spawned plugin task.
    handle: JoinHandle<()>,
}

impl PluginTask {
    /// Spawns `plugin` into a task that loads it and processes events until the channel closes.
    ///
    /// If the plugin fails to load, the error is logged and the task exits. When the channel
    /// closes, the plugin's shutdown hook runs before the task exits.
    pub fn spawn(
        name: &'static str,
        mut plugin: Box<dyn Plugin>,
        ctx: Arc<Context>,
        client: Arc<Client>,
        mut receiver: mpsc::UnboundedReceiver<Envelope>,
        shutdown_span: Arc<OnceLock<Span>>,
    ) -> Self {
        let handle = tokio::spawn(async move {
            debug!(zeta.plugin.name = %name, "plugin task started");

            // The load and shutdown hooks run inside spans for the same reason the handler
            // does: without a span, their warnings only ever reach stdout — the `loaded` span
            // above closed before this one fires.
            let failed_to_load = async {
                if let Err(error) = plugin.loaded(&ctx, &client).await {
                    warn!(exception.message = %error, "plugin failed to load");

                    true
                } else {
                    false
                }
            }
            .instrument(tracing::info_span!(
                "zeta.plugin.load",
                zeta.plugin.name = %name
            ))
            .await;

            if failed_to_load {
                return;
            }

            while let Some(envelope) = receiver.recv().await {
                let Envelope { event, span } = envelope;

                // The span was created by the dispatcher while the IRC line's dispatch span was
                // current, so the handler joins the message's trace. It also covers the time the
                // event waited in the mailbox.
                async {
                    if let Err(error) = handle_event(plugin.as_ref(), &ctx, &client, &event).await {
                        // The handler span is `.or_current()`: when it is filtered out, the
                        // fallback `irc.dispatch` span does not carry the plugin name, so the
                        // error event keeps it.
                        error!(
                            zeta.plugin.name = %name,
                            exception.message = %error,
                            "plugin error during event handling"
                        );
                    }
                }
                .instrument(span)
                .await;
            }

            // The mailbox is closed: the bot is shutting down. The warning goes inside the
            // span, for the same reason as the load one above. The span is a child of the
            // runtime's `zeta.shutdown` span when the shutdown came from there: the hook runs
            // in the plugin's own task, but the explicit parent keeps the shutdown in one
            // trace.
            let span = shutdown_span.get().map_or_else(
                || tracing::info_span!("zeta.plugin.shutdown", zeta.plugin.name = %name),
                |shutdown| {
                    tracing::info_span!(
                        parent: shutdown,
                        "zeta.plugin.shutdown",
                        zeta.plugin.name = %name
                    )
                },
            );

            async {
                if let Err(error) = plugin.shutdown(&ctx, &client).await {
                    warn!(exception.message = %error, "plugin error during shutdown");
                }
            }
            .instrument(span)
            .await;

            debug!(zeta.plugin.name = %name, "plugin task stopped");
        });

        Self { name, handle }
    }

    /// Consumes the plugin task and returns its name with the task's handle.
    ///
    /// The mailbox is closed by the dispatcher, which holds the sending end; awaiting the
    /// returned handle — bounded by a deadline — waits for the plugin to drain its queue and
    /// run its shutdown hook.
    #[must_use]
    pub fn into_handle(self) -> (&'static str, JoinHandle<()>) {
        (self.name, self.handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_plugin::event::MessageEvent;
    use zeta_test_support::{RecordingPlugin, publish_recording, test_context};

    #[tokio::test]
    async fn plugin_task_drains_messages_before_shutdown() {
        let ctx = Arc::new(test_context());
        let recording = publish_recording(&ctx);

        let client = Arc::new(
            Client::from_config(irc::client::data::Config {
                nickname: Some("zeta-test".to_owned()),
                server: Some("mock".to_owned()),
                use_mock_connection: true,
                mock_initial_value: Some(String::new()),
                ..Default::default()
            })
            .await
            .expect("mock irc client"),
        );

        let (sender, receiver) = mpsc::unbounded_channel();
        let task = PluginTask::spawn(
            "recording",
            Box::new(RecordingPlugin {
                recording: Arc::clone(&recording),
            }),
            Arc::clone(&ctx),
            Arc::clone(&client),
            receiver,
            Arc::new(OnceLock::new()),
        );

        for _ in 0..2 {
            let message = irc::proto::Message::new(None, "PRIVMSG", vec!["#test", "hello"])
                .expect("message should parse");

            sender
                .send(Envelope {
                    event: zeta_plugin::Event::Message(MessageEvent::new(Arc::new(message))),
                    span: Span::none(),
                })
                .expect("task should be running");
        }

        // Closing the mailbox makes the task drain its queue, run the shutdown hook and exit.
        drop(sender);

        let (_, handle) = task.into_handle();
        handle.await.expect("task should finish");

        let log = zeta_utils::sync::lock(&recording);

        assert_eq!(*log, ["message", "message", "shutdown"]);
    }
}
