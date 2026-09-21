//! The main process for communicating over IRC and managing state.
use std::sync::Arc;
use std::time::Duration;

use futures::stream::StreamExt;
use irc::client::prelude::Client;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::Error;
use crate::Registry;
use crate::config::Config;
use crate::consts::SHUTDOWN_GRACE;
use crate::plugin::dispatch::EventIndex;
use crate::plugin::filtering::Filters;
use crate::plugin::{Context, PluginTask};

/// Returns the name of the first shutdown signal received by the process.
///
/// `SIGINT` (`Ctrl-C`) and, on unix, `SIGTERM` — e.g. sent by a container orchestrator deleting
/// the pod — both trigger shutdown. The `SIGTERM` listener is best-effort: when it cannot be
/// installed, only `SIGINT` triggers shutdown.
async fn shutdown_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        // Installation failure is ignored; shutdown still works through `SIGINT`.
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            let sigterm = async move { sigterm.recv().await };
            tokio::pin!(sigterm);

            tokio::select! {
                _ = tokio::signal::ctrl_c() => return "SIGINT",
                _ = &mut sigterm => return "SIGTERM",
            }
        }
    }

    if tokio::signal::ctrl_c().await.is_err() {
        warn!("failed to install the SIGINT listener; shutting down anyway");
    }

    "SIGINT"
}

/// Waits for the plugin tasks to drain their queued messages and run their shutdown hooks.
///
/// The tasks share a deadline `grace` seconds from now; any task still running when the deadline
/// passes is aborted. The tasks are awaited concurrently, so a hanging task cannot consume the
/// grace that is owed to the other plugins.
async fn drain_plugin_handles(handles: Vec<(String, JoinHandle<()>)>, grace: Duration) {
    let deadline = tokio::time::Instant::now() + grace;

    let mut handles = handles;

    let outcomes = futures::future::join_all(
        handles
            .iter_mut()
            .map(|(_, handle)| async move { tokio::time::timeout_at(deadline, handle).await })
            .collect::<Vec<_>>(),
    )
    .await;

    for ((name, handle), outcome) in handles.into_iter().zip(outcomes) {
        match outcome {
            Ok(Ok(())) => {}
            // The task was cancelled by its own runtime (or this function, below).
            Ok(Err(error)) if error.is_cancelled() => {}
            Ok(Err(error)) => warn!(plugin = %name, %error, "plugin task panicked while shutting down"),
            Err(_) => {
                warn!(plugin = %name, "plugin task did not shut down in time; aborting");

                handle.abort();
            }
        }
    }
}

/// Drives the client's outgoing sink independently of the dispatch loop.
///
/// The sink is normally driven by the `ClientStream`'s polling, which would leave a queued
/// `QUIT` unflushed after the dispatch loop stops. Driving it separately writes sent messages
/// to the socket as they are queued.
///
/// # Panics
///
/// Panics if the outgoing future is unavailable — only possible if `stream()` had already been
/// called on the client.
fn drive_outgoing(client: &mut Client) {
    let outgoing = client
        .outgoing()
        .expect("the outgoing future is only available before the stream is taken");

    tokio::spawn(async move {
        if let Err(error) = outgoing.await {
            warn!(%error, "the IRC connection write path failed");
        }
    });
}

/// Dispatches incoming IRC messages as events to the plugins that registered interest, until
/// the stream ends.
///
/// # Errors
///
/// Returns any IRC protocol error that ends the stream.
async fn dispatch_messages(
    context: &Context,
    stream: &mut irc::client::ClientStream,
    index: &mut EventIndex,
) -> Result<(), Error> {
    while let Some(message) = stream.next().await.transpose()? {
        debug!(payload = %message, "processing irc message");

        let filters = Filters::from_context(context);

        for stopped in index.dispatch(&filters, message) {
            warn!(plugin = %stopped, "plugin task has stopped");

            index.evict(&stopped);
        }
    }

    Ok(())
}

/// Sends the configured `QUIT` message on shutdown and waits briefly for the outgoing task to
/// flush it to the server.
///
/// `send_quit` only queues the message on an unbounded channel, written to the socket by the
/// task spawned by [`drive_outgoing`]; the stream has already ended, so nothing is read back.
async fn flush_quit(client: &Client, quitting: bool, quit_message: &str) {
    if !quitting {
        return;
    }

    if let Err(error) = client.send_quit(quit_message) {
        warn!(%error, "failed to send the QUIT message");
    }

    tokio::time::sleep(Duration::from_millis(500)).await;
}

/// Exits the process immediately when a second shutdown signal arrives.
///
/// The signal handlers stay installed after the first signal is received, so a second one
/// resolves here instead of terminating the process with the default handler.
fn exit_on_second_signal() {
    tokio::spawn(async move {
        let signal = shutdown_signal().await;
        let code = if signal == "SIGTERM" { 143 } else { 130 };

        warn!(signal = %signal, "received a second shutdown signal; exiting now");

        std::process::exit(code);
    });
}

/// The main IRC bot struct that manages connection state and message handling.
pub struct Zeta {
    /// The complete configuration loaded from file or environment
    config: Config,
    /// The plugin containing all loaded plugins
    registry: Registry,
    /// The shared context for plugins
    context: Arc<Context>,
}

impl Zeta {
    /// Creates a new Zeta instance from the provided configuration.
    ///
    /// This initializes the plugin registry with preloaded plugins but doesn't
    /// establish the IRC connection yet. Call `run()` to start the bot.
    #[must_use]
    pub fn new(
        mut config: Config,
        #[cfg(feature = "database")] db: crate::database::Database,
        dns: hickory_resolver::TokioResolver,
    ) -> Self {
        // The per-plugin sections are handed to the plugins through their constructors; the
        // context's config only carries global configuration.
        let plugins = config.take_plugins();
        let context = Arc::new(Context::new(
            #[cfg(feature = "database")]
            db,
            dns,
            config.clone(),
        ));
        let registry = Registry::preloaded(&context, &plugins);

        Zeta {
            config,
            registry,
            context,
        }
    }

    /// Starts the bot and begins processing IRC messages.
    ///
    /// Each plugin is spawned into its own long-lived task. Incoming messages are dispatched to
    /// every plugin through an unbounded channel, so a slow or failing plugin cannot block the IRC
    /// connection or the other plugins. Plugin errors, including failures during loading, are
    /// logged but never propagated.
    ///
    /// # Shutdown behavior
    ///
    /// The bot shuts down gracefully on `SIGINT` (`Ctrl-C`) and, on unix, `SIGTERM` — e.g. when a
    /// container orchestrator deletes the pod running the bot. On a signal, a `QUIT` with the
    /// configured message ([`IrcConfig::quit_message`]) is sent to the server, after which the
    /// plugin tasks drain their queued messages and run [`Plugin::shutdown`]. The plugin tasks
    /// share a grace period of [`SHUTDOWN_GRACE`]; stragglers are aborted. A second signal during
    /// the grace period exits the process immediately. The same drain runs when the IRC stream
    /// ends, or an error propagates.
    ///
    /// [`IrcConfig::quit_message`]: crate::config::IrcConfig::quit_message
    /// [`Plugin::shutdown`]: zeta_plugin::Plugin::shutdown
    ///
    /// # Panics
    ///
    /// Panics if the IRC client's outgoing future is unavailable — only possible if `stream()`
    /// had already been called on the client, which `run` never does.
    ///
    /// # Errors
    ///
    /// This function will return an error in the following situations:
    ///
    /// - [`Error::IrcClient`] - if the instantiation of the IRC client fails (e.g. due to
    ///   configuration issues.)
    /// - [`Error::IrcRegistration`] - if user registration fails (e.g. if the nickname is already taken.)
    /// - [`Error::Irc`] - if a protocol or communication error occurred.
    pub async fn run(&mut self) -> Result<(), Error> {
        let mut client = Client::from_config(self.config.irc.clone().into())
            .await
            .map_err(Error::IrcClient)?;

        client.identify().map_err(Error::IrcRegistration)?;

        drive_outgoing(&mut client);

        let mut stream = client.stream()?;
        let client = Arc::new(client);
        let quit_message = self.config.irc.quit_message.clone();

        // Each plugin gets its own long-lived task with an unbounded mailbox, so a slow or
        // failing plugin cannot block the IRC connection or the other plugins.
        let (mut index, plugins) = self.spawn_plugin_tasks(&client);

        // Dispatching incoming IRC messages as events to the plugins that registered interest,
        // until the stream ends or a shutdown signal arrives.
        let dispatch = dispatch_messages(&self.context, &mut stream, &mut index);

        let mut quitting = false;
        let mut dispatch_error = None;

        tokio::select! {
            result = dispatch => {
                if let Err(error) = result {
                    dispatch_error = Some(error);
                }
            }
            signal = shutdown_signal() => {
                info!(signal = %signal, "shutting down");
                quitting = true;

                // Another signal during the grace period forces an immediate exit.
                exit_on_second_signal();
            }
        }

        flush_quit(&client, quitting, &quit_message).await;

        let handles = plugins
            .into_iter()
            .map(PluginTask::into_handle)
            .collect::<Vec<_>>();

        drain_plugin_handles(handles, SHUTDOWN_GRACE).await;

        dispatch_error.map_or_else(|| Ok(()), Err)
    }

    /// Spawns a long-lived task per registered plugin and indexes its subscriptions.
    ///
    /// Returns the dispatch index — which holds the mailboxes' sending ends, so dropping it
    /// closes every mailbox at once, making the plugins drain and run their shutdown hooks —
    /// together with the spawned tasks.
    fn spawn_plugin_tasks(&mut self, client: &Arc<Client>) -> (EventIndex, Vec<PluginTask>) {
        let mut index = EventIndex::default();
        let mut plugins = Vec::new();

        for registered in self.registry.take_plugins() {
            let name = registered.name;
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

            plugins.push(PluginTask::spawn(
                name.clone(),
                registered.plugin,
                Arc::clone(&self.context),
                Arc::clone(client),
                receiver,
            ));
            index.add(&name, &registered.subscriptions, sender);
        }

        (index, plugins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drain_plugin_handles_aborts_stragglers() {
        let finished = Arc::new(tokio::sync::Mutex::new(false));
        let flag = Arc::clone(&finished);

        let fast = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;

            *flag.lock().await = true;
        });
        let hung = tokio::spawn(futures::future::pending::<()>());

        drain_plugin_handles(
            vec![("fast".to_string(), fast), ("hung".to_string(), hung)],
            Duration::from_millis(200),
        )
        .await;

        assert!(*finished.lock().await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_signal_responds_to_sigterm() {
        let signal = tokio::spawn(shutdown_signal());

        // Give the signal task a chance to install the `SIGTERM` listener before sending.
        tokio::time::sleep(Duration::from_millis(100)).await;

        std::process::Command::new("kill")
            .args(["-s", "TERM", &std::process::id().to_string()])
            .status()
            .expect("could not send SIGTERM");

        assert_eq!(signal.await.unwrap(), "SIGTERM");
    }
}
