//! The main process for communicating over IRC and managing state.
use std::sync::Arc;

use irc::client::prelude::Client;
use tracing::{Instrument, debug, info, warn};

use crate::Error;
use crate::config::Config;
use crate::consts::{QUIT_FLUSH_DELAY, SHUTDOWN_GRACE};
use zeta_plugin::Context;
use zeta_plugin_runtime::{PluginRuntime, Registry};

/// The signal that triggered a graceful shutdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShutdownSignal {
    /// `SIGINT`, sent by `Ctrl-C`.
    Interrupt,
    /// `SIGTERM`, sent by a service manager or container orchestrator.
    Terminate,
}

impl ShutdownSignal {
    /// Returns the signal's conventional name, for logs.
    const fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
        }
    }

    /// Returns the process exit code conventionally associated with the signal.
    const fn exit_code(self) -> i32 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
        }
    }
}

/// Returns the first shutdown signal received by the process.
///
/// `SIGINT` (`Ctrl-C`) and, on unix, `SIGTERM` — e.g. sent by a container orchestrator deleting
/// the pod — both trigger shutdown. The `SIGTERM` listener is best-effort: when it cannot be
/// installed, only `SIGINT` triggers shutdown.
async fn shutdown_signal() -> ShutdownSignal {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        // Installation failure is ignored; shutdown still works through `SIGINT`.
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            let sigterm = async move { sigterm.recv().await };
            tokio::pin!(sigterm);

            tokio::select! {
                _ = tokio::signal::ctrl_c() => return ShutdownSignal::Interrupt,
                _ = &mut sigterm => return ShutdownSignal::Terminate,
            }
        }
    }

    if tokio::signal::ctrl_c().await.is_err() {
        warn!("failed to install the SIGINT listener; shutting down anyway");
    }

    ShutdownSignal::Interrupt
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

    // The span keeps the write-path failure visible: events outside a span are dropped by the
    // OpenTelemetry layer.
    tokio::spawn(
        async move {
            if let Err(error) = outgoing.await {
                warn!(%error, "the IRC connection write path failed");
            }
        }
        .instrument(tracing::info_span!("irc_outgoing")),
    );
}

/// Dispatches incoming IRC messages as events to the plugins that registered interest, until
/// the stream ends.
///
/// # Errors
///
/// Returns any IRC protocol error that ends the stream.
async fn dispatch_messages(
    stream: &mut irc::client::ClientStream,
    runtime: &mut PluginRuntime,
) -> Result<(), Error> {
    use futures::stream::StreamExt;

    while let Some(message) = stream.next().await.transpose()? {
        // Everything logged while this message is routed runs inside a span: the
        // OpenTelemetry layer drops events that are not in the context of a span, so without
        // it the routing logs would only ever reach stdout. The span is at info level, not
        // debug, because the loop below logs warnings: an `info` filter must not be what
        // keeps them out of the backend.
        let _dispatch = tracing::info_span!("dispatch").entered();

        debug!(payload = %message, "processing irc message");

        runtime.dispatch(message);
    }

    Ok(())
}

/// Sends the configured `QUIT` message on shutdown and waits briefly for the outgoing task to
/// flush it to the server.
///
/// `send_quit` only queues the message on an unbounded channel, written to the socket by the
/// task spawned by [`drive_outgoing`]; the stream has already ended, so nothing is read back.
async fn flush_quit(client: &Client, quit_message: &str) {
    if let Err(error) = client.send_quit(quit_message) {
        warn!(%error, "failed to send the QUIT message");
    }

    tokio::time::sleep(QUIT_FLUSH_DELAY).await;
}

/// Exits the process immediately when a second shutdown signal arrives.
///
/// The signal handlers stay installed after the first signal is received, so a second one
/// resolves here instead of terminating the process with the default handler.
fn exit_on_second_signal() {
    tokio::spawn(
        async move {
            let signal = shutdown_signal().await;

            warn!(
                signal = signal.name(),
                "received a second shutdown signal; exiting now"
            );

            std::process::exit(signal.exit_code());
        }
        .instrument(tracing::info_span!("second_signal")),
    );
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
    ) -> Self {
        // The shared media mirror is published before any plugin is constructed, so plugins can
        // pick it up through their constructors.
        // The per-plugin sections are handed to the plugins through their constructors; the
        // context's config only carries global configuration.
        let plugins = config.take_plugins();
        let context = Arc::new(Context::new(
            #[cfg(feature = "database")]
            db,
            zeta_plugin::config::HostConfig::from(&config),
        ));

        // The shared mirror is published before any plugin is registered, so the plugins that
        // mirror media can pick it up through their constructors. Mirroring stays disabled when
        // the configuration is incomplete or invalid.
        #[cfg(feature = "mirror")]
        match zeta_mirror::Mirror::from_config(&config.mirror) {
            Ok(mirror) => {
                context.shared.publish(Arc::new(mirror));
            }
            Err(error) => {
                warn!(%error, "mirroring is disabled");
            }
        }

        let mut registry = Registry::new();
        plugins::register_bundled_plugins(&mut registry, &context, &plugins);
        registry.publish_catalog(&context);

        Self {
            config,
            registry,
            context,
        }
    }

    /// Starts the bot and begins processing IRC messages.
    ///
    /// Consumes the bot: the plugin registry is drained into the running tasks, so the instance
    /// cannot be run twice.
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
    /// share a grace period of `SHUTDOWN_GRACE`; stragglers are aborted. A second signal during
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
    #[tracing::instrument(name = "run", skip_all)]
    pub async fn run(mut self) -> Result<(), Error> {
        let mut client = Client::from_config((&self.config.irc).into())
            .await
            .map_err(Error::IrcClient)?;

        client.identify().map_err(Error::IrcRegistration)?;

        drive_outgoing(&mut client);

        let mut stream = client.stream()?;
        let client = Arc::new(client);

        // Each plugin gets its own long-lived task with an unbounded mailbox, so a slow or
        // failing plugin cannot block the IRC connection or the other plugins.
        let mut runtime = PluginRuntime::start(
            self.registry.take_plugins(),
            Arc::clone(&self.context),
            &client,
        );

        // Dispatching incoming IRC messages as events to the plugins that registered interest,
        // until the stream ends or a shutdown signal arrives.
        let dispatch = dispatch_messages(&mut stream, &mut runtime);

        let mut shutting_down = false;
        let mut dispatch_error = None;

        tokio::select! {
            result = dispatch => {
                if let Err(error) = result {
                    dispatch_error = Some(error);
                }
            }
            signal = shutdown_signal() => {
                info!(signal = signal.name(), "shutting down");
                shutting_down = true;

                // Another signal during the grace period forces an immediate exit.
                exit_on_second_signal();
            }
        }

        // Closing the plugin mailboxes makes the tasks drain their queued events and run
        // `Plugin::shutdown`; awaiting the shutdown joins them within the grace period. The
        // dispatch future only borrows the runtime, so ending the `select!` above does not
        // close the mailboxes on its own.
        let shutdown = runtime.shutdown(SHUTDOWN_GRACE);

        if shutting_down {
            // The QUIT is written while the plugins drain, so it does not delay their grace.
            tokio::join!(flush_quit(&client, &self.config.irc.quit_message), shutdown);
        } else {
            shutdown.await;
        }

        dispatch_error.map_or_else(|| Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_signal_responds_to_sigterm() {
        use futures::task::noop_waker;
        use std::task::Poll;

        let mut signal = Box::pin(shutdown_signal());

        // Poll once with a no-op waker: the first poll installs the `SIGINT` and `SIGTERM`
        // listeners, so the signal below can never arrive while the default disposition is
        // still active — which would terminate the whole test process.
        let waker = noop_waker();
        let mut context = std::task::Context::from_waker(&waker);

        assert_eq!(signal.as_mut().poll(&mut context), Poll::Pending);

        std::process::Command::new("kill")
            .args(["-s", "TERM", &std::process::id().to_string()])
            .status()
            .expect("could not send SIGTERM");

        let received = signal.await;

        assert_eq!(received, ShutdownSignal::Terminate);
    }
}
