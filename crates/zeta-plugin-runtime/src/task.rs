//! The long-lived task that runs one plugin.

use std::sync::Arc;

use irc::client::Client;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, error, warn};

use zeta_plugin::{Context, ErasedPlugin, Event};

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
        mut plugin: Box<dyn ErasedPlugin>,
        ctx: Arc<Context>,
        client: Arc<Client>,
        mut receiver: mpsc::UnboundedReceiver<Event>,
    ) -> Self {
        let handle = tokio::spawn(async move {
            debug!(plugin = %name, "plugin task started");

            // The load and shutdown hooks run inside spans for the same reason the handler
            // does: without a span, their warnings only ever reach stdout — the `loaded` span
            // above closed before this one fires.
            let failed_to_load = async {
                if let Err(error) = plugin.loaded(&ctx, &client).await {
                    warn!(plugin = %name, %error, "plugin failed to load");

                    true
                } else {
                    false
                }
            }
            .instrument(tracing::info_span!("loaded", plugin = %name))
            .await;

            if failed_to_load {
                return;
            }

            while let Some(event) = receiver.recv().await {
                // The handler runs inside a span: the OpenTelemetry layer drops events that
                // are not in the context of a span, so without it the plugin's logs would
                // only ever reach stdout.
                async {
                    if let Err(error) = plugin.handle_event(&ctx, &client, &event).await {
                        error!(plugin = %name, %error, "plugin error during event handling");
                    }
                }
                .instrument(tracing::info_span!("handle_event", plugin = %name))
                .await;
            }

            // The mailbox is closed: the bot is shutting down. The warning goes inside the
            // span, for the same reason as the load one above.
            async {
                if let Err(error) = plugin.shutdown(&ctx, &client).await {
                    warn!(plugin = %name, %error, "plugin error during shutdown");
                }
            }
            .instrument(tracing::info_span!("shutdown", plugin = %name))
            .await;

            debug!(plugin = %name, "plugin task stopped");
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
        );

        for _ in 0..2 {
            let message = irc::proto::Message::new(None, "PRIVMSG", vec!["#test", "hello"])
                .expect("message should parse");

            sender
                .send(Event::Message(MessageEvent::new(Arc::new(message))))
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
