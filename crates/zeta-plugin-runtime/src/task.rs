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
    pub name: String,
    /// The handle of the spawned plugin task.
    handle: JoinHandle<()>,
}

impl PluginTask {
    /// Spawns `plugin` into a task that loads it and processes events until the channel closes.
    ///
    /// If the plugin fails to load, the error is logged and the task exits. When the channel
    /// closes, the plugin's shutdown hook runs before the task exits.
    pub fn spawn(
        name: String,
        mut plugin: Box<dyn ErasedPlugin>,
        ctx: Arc<Context>,
        client: Arc<Client>,
        mut receiver: mpsc::UnboundedReceiver<Event>,
    ) -> Self {
        let task_name = name.clone();

        let handle = tokio::spawn(async move {
            debug!(plugin = %task_name, "plugin task started");

            // The load and shutdown hooks run inside spans for the same reason the handler
            // does: without a span, their warnings only ever reach stdout — the `loaded` span
            // above closed before this one fires.
            let failed_to_load = async {
                if let Err(error) = plugin.loaded(&ctx, &client).await {
                    warn!(plugin = %task_name, %error, "plugin failed to load");

                    true
                } else {
                    false
                }
            }
            .instrument(tracing::info_span!("loaded", plugin = %task_name))
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
                        error!(plugin = %task_name, %error, "plugin error during event handling");
                    }
                }
                .instrument(tracing::info_span!("handle_event", plugin = %task_name))
                .await;
            }

            // The mailbox is closed: the bot is shutting down. The warning goes inside the
            // span, for the same reason as the load one above.
            async {
                if let Err(error) = plugin.shutdown(&ctx, &client).await {
                    warn!(plugin = %task_name, %error, "plugin error during shutdown");
                }
            }
            .instrument(tracing::info_span!("shutdown", plugin = %task_name))
            .await;

            debug!(plugin = %task_name, "plugin task stopped");
        });

        Self { name, handle }
    }

    /// Consumes the plugin task and returns its name with the task's handle.
    ///
    /// The mailbox is closed by the dispatcher, which holds the sending end; awaiting the
    /// returned handle — bounded by a deadline — waits for the plugin to drain its queue and
    /// run its shutdown hook.
    #[must_use]
    pub fn into_handle(self) -> (String, JoinHandle<()>) {
        (self.name, self.handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_plugin::event::MessageEvent;
    use zeta_plugin::{Error, NoSettings, Plugin};
    use zeta_test_support::test_context;

    /// The lifecycle log shared between a test and its plugin through the context.
    type Recording = Arc<std::sync::Mutex<Vec<&'static str>>>;

    /// Publishes the recording log through the context's shared state.
    fn publish_recording(ctx: &Context) -> Recording {
        let recording: Recording = Arc::new(std::sync::Mutex::new(Vec::new()));

        assert!(ctx.shared.publish(Arc::clone(&recording)).is_none());

        recording
    }

    /// Test plugin recording the lifecycle events it observes.
    struct RecordingPlugin {
        recording: Recording,
    }

    #[async_trait::async_trait]
    impl Plugin<Context> for RecordingPlugin {
        const NAME: &'static str = "recording";

        type Settings = NoSettings;

        fn new(
            ctx: &Context,
            _: &NoSettings,
            _: &mut zeta_plugin::Subscriptions,
        ) -> Result<Self, Error> {
            let recording = ctx
                .shared
                .get::<std::sync::Mutex<Vec<&'static str>>>()
                .expect("recording log should be published");

            Ok(Self { recording })
        }

        async fn handle_message(
            &self,
            _: &Context,
            _: &Client,
            _: &MessageEvent,
        ) -> Result<(), Error> {
            zeta_utils::sync::lock(&self.recording).push("message");

            Ok(())
        }

        async fn shutdown(&mut self, _: &Context, _: &Client) -> Result<(), Error> {
            zeta_utils::sync::lock(&self.recording).push("shutdown");

            Ok(())
        }
    }

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
            "recording".to_string(),
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
