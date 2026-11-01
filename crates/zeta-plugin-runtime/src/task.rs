//! The long-lived task that runs one plugin.

use std::panic::{AssertUnwindSafe, resume_unwind};
use std::sync::Arc;
use std::sync::OnceLock;

use futures::FutureExt;
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
/// drain its queued events, run the shutdown hook and exit. A panicking handler still runs the
/// shutdown hook; the panic is re-raised afterwards so the task keeps its panic status.
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
    /// closes, the plugin's shutdown hook runs before the task exits. If a handler panics, the
    /// shutdown hook still runs and the panic is resumed afterwards, so the task's join error
    /// remains a panic rather than a clean exit.
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
            // above closed before this one fires. The exported name carries the plugin name so
            // one span per plugin stays readable in the trace view.
            let load_span = tracing::info_span!(
                "zeta.plugin.load",
                otel.name = %format!("zeta.plugin.load {name}"),
                zeta.plugin.name = %name
            );

            if let Err(error) = plugin
                .loaded(&ctx, &client)
                .instrument(load_span.clone())
                .await
            {
                load_span.in_scope(|| warn!(exception.message = %error, "plugin failed to load"));

                return;
            }

            // A panic in a handler must not skip the shutdown hook: background tasks the plugin
            // owns would otherwise keep running without their owner. The panic is held until the
            // hook has run and then resumed, so the task still ends as a panic.
            let panic = AssertUnwindSafe(async {
                while let Some(envelope) = receiver.recv().await {
                    let Envelope { event, span } = envelope;

                    // The span was created by the dispatcher while the IRC line's dispatch span
                    // was current, so the handler joins the message's trace. It also covers the
                    // time the event waited in the mailbox.
                    async {
                        if let Err(error) =
                            handle_event(plugin.as_ref(), &ctx, &client, &event).await
                        {
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
            })
            .catch_unwind()
            .await
            .err();

            // The event loop has ended: the mailbox is closed because the bot is shutting down,
            // or a handler panicked. The warning goes inside the span, for the same reason as
            // the load one above. The span is a child of the runtime's `zeta.shutdown` span when
            // the shutdown came from there: the hook runs in the plugin's own task, but the
            // explicit parent keeps the shutdown in one trace.
            let span = shutdown_span.get().map_or_else(
                || {
                    tracing::info_span!(
                        "zeta.plugin.shutdown",
                        otel.name = %format!("zeta.plugin.shutdown {name}"),
                        zeta.plugin.name = %name
                    )
                },
                |shutdown| {
                    tracing::info_span!(
                        parent: shutdown,
                        "zeta.plugin.shutdown",
                        otel.name = %format!("zeta.plugin.shutdown {name}"),
                        zeta.plugin.name = %name
                    )
                },
            );

            if let Err(error) = plugin
                .shutdown(&ctx, &client)
                .instrument(span.clone())
                .await
            {
                span.in_scope(|| warn!(exception.message = %error, "plugin error during shutdown"));
            }

            if let Some(payload) = panic {
                resume_unwind(payload);
            }

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
    use zeta_test_support::{Recording, RecordingPlugin, publish_recording, test_context};

    /// A plugin whose message handler panics, recording that the shutdown hook still ran.
    struct PanickingPlugin {
        /// The log the shutdown hook appends to.
        recording: Recording,
    }

    #[zeta_plugin::async_trait::async_trait]
    impl Plugin for PanickingPlugin {
        async fn handle_message(
            &self,
            _: &Context,
            _: &Client,
            _: &MessageEvent,
        ) -> Result<(), zeta_plugin::Error> {
            panic!("handler panic");
        }

        async fn shutdown(&mut self, _: &Context, _: &Client) -> Result<(), zeta_plugin::Error> {
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

    #[tokio::test]
    async fn plugin_task_runs_shutdown_after_a_panicking_handler() {
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
            "panicking",
            Box::new(PanickingPlugin {
                recording: Arc::clone(&recording),
            }),
            Arc::clone(&ctx),
            Arc::clone(&client),
            receiver,
            Arc::new(OnceLock::new()),
        );

        let message = irc::proto::Message::new(None, "PRIVMSG", vec!["#test", "hello"])
            .expect("message should parse");

        sender
            .send(Envelope {
                event: zeta_plugin::Event::Message(MessageEvent::new(Arc::new(message))),
                span: Span::none(),
            })
            .expect("task should be running");

        drop(sender);

        let (_, handle) = task.into_handle();
        let error = handle
            .await
            .expect_err("the panicking handler should still panic the task");

        assert!(error.is_panic(), "expected a panic join error: {error:?}");
        assert_eq!(*zeta_utils::sync::lock(&recording), ["shutdown"]);
    }
}
