//! Plugin registry.
//!
//! The registry constructs plugins, keeps the loaded ones with the subscriptions they declared,
//! and tracks the ones that failed to initialize. The host drives registration through
//! [`Registry::register`] (or a bundled-plugin helper such as the `plugins` crate's
//! `register_bundled_plugins`), spawns each [`RegisteredPlugin`] into its own [`PluginTask`],
//! and routes events to them through the dispatch index.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use irc::client::Client;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, error, warn};

use crate::command::CommandSpec;
use crate::context::Context;
use crate::event::{Event, Subscriptions, UrlScope};
use crate::{Error, Plugin};

/// Object-safe view of [`Plugin`], implemented automatically for every `Plugin<Context>`.
///
/// [`Plugin`] declares an associated [`Settings`](Plugin::Settings) type and therefore cannot be
/// used directly as a trait object. The registry stores plugins with different settings types
/// behind this trait, which erases the settings type and exposes only the runtime methods the
/// host needs. Plugin authors never implement this trait themselves.
///
/// The methods return the plugin's already-boxed futures directly instead of being declared
/// `async`; forwarding an `async fn` into another `async fn` would allocate a second boxed future
/// that only awaits the first.
pub trait ErasedPlugin: Send + Sync {
    /// Routes an event to the handler registered for its kind.
    ///
    /// Events are only routed to handlers the plugin registered the matching kind for, so an
    /// unknown variant is unreachable in practice; it is logged when a future event kind is
    /// added without a routing arm.
    fn handle_event<'a>(
        &'a self,
        ctx: &'a Context,
        client: &'a Client,
        event: &'a Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;

    /// Called when all plugins are loaded and the client has connected to the network.
    fn loaded<'a>(
        &'a mut self,
        ctx: &'a Context,
        client: &'a Client,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;

    /// Called once while the bot is shutting down, after the plugin's event queue has been
    /// drained.
    fn shutdown<'a>(
        &'a mut self,
        ctx: &'a Context,
        client: &'a Client,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;
}

impl<P: Plugin<Context>> ErasedPlugin for P {
    fn handle_event<'a>(
        &'a self,
        ctx: &'a Context,
        client: &'a Client,
        event: &'a Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            match event {
                Event::Command(command) => Plugin::handle_command(self, ctx, client, command).await,
                Event::Url(url) => Plugin::handle_url(self, ctx, client, url).await,
                Event::Message(message) => Plugin::handle_message(self, ctx, client, message).await,
                Event::Join(join) => Plugin::handle_join(self, ctx, client, join).await,
                Event::Part(part) => Plugin::handle_part(self, ctx, client, part).await,
                Event::Quit(quit) => Plugin::handle_quit(self, ctx, client, quit).await,
                Event::Nick(nick) => Plugin::handle_nick(self, ctx, client, nick).await,
                Event::Kick(kick) => Plugin::handle_kick(self, ctx, client, kick).await,
                Event::Ctcp(ctcp) => Plugin::handle_ctcp(self, ctx, client, ctcp).await,
                Event::Raw(raw) => Plugin::handle_raw(self, ctx, client, raw).await,
            }
        })
    }

    fn loaded<'a>(
        &'a mut self,
        ctx: &'a Context,
        client: &'a Client,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Plugin::loaded(self, ctx, client)
    }

    fn shutdown<'a>(
        &'a mut self,
        ctx: &'a Context,
        client: &'a Client,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Plugin::shutdown(self, ctx, client)
    }
}

/// A plugin registered with the bot, together with the subscriptions it declared.
pub struct RegisteredPlugin {
    /// The name of the plugin.
    pub name: String,
    /// The plugin itself.
    pub plugin: Box<dyn ErasedPlugin>,
    /// The events the plugin registered interest in during initialization.
    pub subscriptions: Subscriptions,
}

/// Metadata about a registered plugin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEntry {
    /// The name of the plugin.
    pub name: String,
    /// The commands handled by the plugin.
    pub commands: Vec<CommandSpec>,
    /// The URL hosts whose links the plugin handles itself.
    pub url_hosts: Vec<&'static str>,
}

/// Snapshot of the plugins registered with the bot and the events they handle.
///
/// The catalog is published to the context's shared state once registration is finished, so
/// plugins can look it up with [`SharedState::get`](crate::context::SharedState::get) to
/// discover other plugins.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PluginCatalog {
    /// The registered plugins, in registration order.
    pub entries: Vec<CatalogEntry>,
}

/// Plugin registry.
#[derive(Default)]
pub struct Registry {
    /// List of loaded plugins.
    pub plugins: Vec<RegisteredPlugin>,
    /// Catalog of the registered plugins, published to the context's shared state.
    catalog: PluginCatalog,
    /// List of plugins that failed to initialize.
    pub failed: Vec<(String, Error)>,
}

impl Registry {
    /// Constructs and returns a new, empty plugin registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            plugins: vec![],
            catalog: PluginCatalog::default(),
            failed: vec![],
        }
    }

    /// Publishes the catalog of registered plugins to the context's shared state.
    ///
    /// Called once after all plugins have been registered, so plugins can discover each other
    /// through the catalog.
    pub fn publish_catalog(&self, ctx: &Context) {
        ctx.shared.publish(Arc::new(self.catalog.clone()));
    }

    /// Registers a new plugin based on its type.
    ///
    /// A fresh [`Subscriptions`] set is passed to the plugin's constructor, which registers the
    /// events it handles on it. The subscriptions are committed — to the catalog and to the
    /// returned plugin — only when construction succeeds.
    ///
    /// Returns `true` if the plugin was successfully initialized and registered, `false` if
    /// initialization failed. Failed plugins are tracked in `self.failed` and logged with their
    /// name and error.
    pub fn register<P: Plugin<Context> + 'static>(
        &mut self,
        ctx: &Context,
        settings: &P::Settings,
    ) -> bool {
        let name = P::NAME.to_string();
        let mut subscriptions = Subscriptions::new();

        match P::new(ctx, settings, &mut subscriptions) {
            Ok(plugin) => {
                debug!(plugin = %name, "registered plugin");

                let url_hosts = match subscriptions.url_scope() {
                    UrlScope::Hosts(hosts) => hosts.to_vec(),
                    _ => Vec::new(),
                };

                self.catalog.entries.push(CatalogEntry {
                    name: name.clone(),
                    commands: subscriptions.commands().to_vec(),
                    url_hosts,
                });
                self.plugins.push(RegisteredPlugin {
                    name,
                    plugin: Box::new(plugin),
                    subscriptions,
                });

                true
            }
            Err(e) => {
                tracing::warn!(plugin = %name, error = %e, "failed to initialize plugin");
                self.failed.push((name, e));

                false
            }
        }
    }

    /// Removes and returns all loaded plugins, leaving the registry empty.
    ///
    /// The caller is expected to move each plugin into its own task.
    #[must_use]
    pub fn take_plugins(&mut self) -> Vec<RegisteredPlugin> {
        std::mem::take(&mut self.plugins)
    }
}

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
    use crate::event::MessageEvent;
    use crate::{PluginName, types::NoSettings};
    use async_trait::async_trait;

    /// Constructs a context for the tests: a default configuration and an unreachable (lazily
    /// connected) database pool.
    ///
    /// Local on purpose: `zeta-test-support` depends on this crate, so the helper cannot live
    /// there without a dependency cycle.
    fn test_context() -> Context {
        Context::new(
            #[cfg(feature = "database")]
            {
                use sqlx::postgres::PgPoolOptions;

                PgPoolOptions::new()
                    .connect_lazy("postgresql://invalid/zeta_test")
                    .expect("lazy database pool")
            },
            crate::config::HostConfig::default(),
        )
    }

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

    impl PluginName for RecordingPlugin {
        const NAME: &'static str = "recording";
    }

    #[async_trait]
    impl Plugin<Context> for RecordingPlugin {
        type Settings = NoSettings;

        fn new(ctx: &Context, _: &NoSettings, _: &mut Subscriptions) -> Result<Self, Error> {
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

    #[cfg(feature = "database")]
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
