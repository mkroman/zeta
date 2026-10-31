//! The main process for communicating over IRC and managing state.
use std::sync::Arc;

use irc::client::prelude::Client;
use tracing::{Instrument, error, info, trace, warn};

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
        tracing::warn_span!("zeta.signal.install")
            .in_scope(|| warn!("failed to install the SIGINT listener; shutting down anyway"));
    }

    ShutdownSignal::Interrupt
}

/// Drives the client's outgoing sink independently of the dispatch loop.
///
/// The sink is normally driven by the `ClientStream`'s polling, which would leave a queued
/// `QUIT` unflushed after the dispatch loop stops. Driving it separately writes sent messages
/// to the socket as they are queued.
///
/// Returns the task's handle so the caller can supervise the write path and wait for the final
/// flush after the last client handle is dropped.
///
/// # Panics
///
/// Panics if the outgoing future is unavailable — only possible if `stream()` had already been
/// called on the client.
fn drive_outgoing(client: &mut Client) -> tokio::task::JoinHandle<()> {
    let outgoing = client
        .outgoing()
        .expect("the outgoing future is only available before the stream is taken");

    // The failure gets a short span of its own: a task-lifetime span would never be exported
    // until shutdown, and the OpenTelemetry layer needs a current span for the message.
    tokio::spawn(async move {
        if let Err(error) = outgoing.await {
            let span = tracing::error_span!("irc.outgoing.error");
            span.in_scope(
                || error!(exception.message = %error, "the IRC connection write path failed"),
            );
        }
    })
}

/// Returns the IRC command name for telemetry.
fn command_name(command: &irc::proto::Command) -> &str {
    match command {
        irc::proto::Command::PRIVMSG(..) => "PRIVMSG",
        irc::proto::Command::NOTICE(..) => "NOTICE",
        irc::proto::Command::JOIN(..) => "JOIN",
        irc::proto::Command::PART(..) => "PART",
        irc::proto::Command::QUIT(..) => "QUIT",
        irc::proto::Command::NICK(..) => "NICK",
        irc::proto::Command::KICK(..) => "KICK",
        irc::proto::Command::ERROR(..) => "ERROR",
        irc::proto::Command::Raw(name, _) => name,
        _ => "OTHER",
    }
}

/// Returns the channel a message targets, when it has one.
fn message_channel(command: &irc::proto::Command) -> Option<&str> {
    match command {
        irc::proto::Command::PRIVMSG(target, _)
        | irc::proto::Command::NOTICE(target, _)
        | irc::proto::Command::JOIN(target, ..)
        | irc::proto::Command::PART(target, ..)
        | irc::proto::Command::KICK(target, ..) => Some(target),
        _ => None,
    }
}

/// Dispatches incoming IRC messages as events to the plugins that registered interest, until
/// the stream ends.
///
/// # Errors
///
/// Returns any IRC protocol error that ends the stream, and [`Error::IrcDisconnected`] when the
/// server sends `ERROR` or closes the stream. A closed connection is reported as an error so the
/// process exits non-zero rather than looking like a successful run to its supervisor.
async fn dispatch_messages(
    stream: &mut irc::client::ClientStream,
    runtime: &mut PluginRuntime,
) -> Result<(), Error> {
    use futures::stream::StreamExt;

    while let Some(message) = stream.next().await.transpose()? {
        // Everything logged while this message is routed runs inside a span: the
        // OpenTelemetry layer drops events that are not in the context of a span. Only the
        // commands `EventIndex` routes get one — PING, PONG and numerics would otherwise
        // produce a trace with no children. `ERROR` is included because the disconnect reason
        // is logged below. The span is at info level, not debug, because the loop below logs
        // warnings: an `info` filter must not be what keeps them out of the backend.
        let routed = matches!(
            &message.command,
            irc::proto::Command::PRIVMSG(..)
                | irc::proto::Command::NOTICE(..)
                | irc::proto::Command::JOIN(..)
                | irc::proto::Command::PART(..)
                | irc::proto::Command::QUIT(..)
                | irc::proto::Command::NICK(..)
                | irc::proto::Command::KICK(..)
                | irc::proto::Command::Raw(..)
                | irc::proto::Command::ERROR(..)
        );
        let dispatch = routed.then(|| {
            let span = tracing::info_span!(
                "irc.dispatch",
                irc.command = command_name(&message.command),
                irc.channel = tracing::field::Empty,
            );

            if let Some(channel) = message_channel(&message.command) {
                span.record("irc.channel", channel);
            }

            span
        });
        let _dispatch = dispatch.as_ref().map(tracing::Span::enter);

        // `ERROR` carries the server's reason for closing the connection. It is not a `Raw`
        // command, so without this check the reason would be dropped entirely.
        if let irc::proto::Command::ERROR(reason) = &message.command {
            warn!(reason = %reason, "the IRC server closed the connection");

            return Err(Error::IrcDisconnected {
                reason: reason.clone(),
            });
        }

        trace!(irc.raw = %message, "processing irc message");

        runtime.dispatch(message);
    }

    tracing::warn_span!("irc.stream.ended").in_scope(|| warn!("the IRC message stream ended"));

    Err(Error::IrcDisconnected {
        reason: "the server closed the connection".to_owned(),
    })
}

/// Exits the process immediately when a second shutdown signal arrives.
///
/// The signal handlers stay installed after the first signal is received, so a second one
/// resolves here instead of terminating the process with the default handler.
fn exit_on_second_signal() {
    tokio::spawn(async move {
        let signal = shutdown_signal().await;

        // The span closes as soon as the message is logged; the task then exits the process.
        let span = tracing::warn_span!("zeta.signal.second", signal = signal.name());
        span.in_scope(|| warn!("received a second shutdown signal; exiting now"));

        std::process::exit(signal.exit_code());
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
                warn!(exception.message = %error, "mirroring is disabled");
            }
        }

        let mut registry = Registry::new();
        plugins::register_bundled_plugins(&mut registry, &context, &plugins);
        registry.publish_catalog(&context);

        let failed = registry.failed_plugin_names().collect::<Vec<_>>();

        if failed.is_empty() {
            info!(plugins.loaded = registry.len(), "plugins registered");
        } else {
            warn!(
                plugins.loaded = registry.len(),
                plugins.failed = ?failed,
                "some plugins failed to initialize"
            );
        }

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
    /// container orchestrator deletes the pod running the bot. On a signal, the plugin tasks
    /// drain their queued messages and run [`Plugin::shutdown`] first, then a `QUIT` with the
    /// configured message ([`IrcConfig::quit_message`]) is sent to the server. The plugin tasks
    /// share a grace period of `SHUTDOWN_GRACE`; stragglers are aborted. A second signal during
    /// the grace period exits the process immediately, whether the shutdown was triggered by a
    /// signal or by the connection ending. The same drain runs when the IRC stream ends, or an
    /// error propagates; a connection that ends without a shutdown signal is reported as an
    /// error so the process exits non-zero.
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
    /// - [`Error::IrcDisconnected`] - if the server closed the connection or sent `ERROR`.
    pub async fn run(mut self) -> Result<(), Error> {
        let mut client = Client::from_config((&self.config.irc).into())
            .await
            .map_err(Error::IrcClient)?;

        client.identify().map_err(Error::IrcRegistration)?;

        let mut outgoing = drive_outgoing(&mut client);

        let mut stream = client.stream()?;
        let client = Arc::new(client);

        // Each plugin gets its own long-lived task with an unbounded mailbox, so a slow or
        // failing plugin cannot block the IRC connection or the other plugins.
        let mut runtime =
            PluginRuntime::start(self.registry.take_plugins(), &self.context, &client);

        // Dispatching incoming IRC messages as events to the plugins that registered interest,
        // until the stream ends, the write path stops, or a shutdown signal arrives.
        let dispatch = dispatch_messages(&mut stream, &mut runtime);

        let mut shutting_down = false;
        let mut dispatch_error = None;
        let mut outgoing_done = false;

        tokio::select! {
            result = dispatch => {
                if let Err(error) = result {
                    dispatch_error = Some(error);
                }
            }
            signal = shutdown_signal() => {
                // `run` has no process-lifetime span of its own, and the OpenTelemetry layer
                // drops events that are not in a span, so each one-off diagnostic gets a short
                // span here.
                tracing::info_span!("zeta.signal", signal = signal.name())
                    .in_scope(|| info!("shutting down"));
                shutting_down = true;

                // Another signal during the grace period forces an immediate exit.
                exit_on_second_signal();
            }
            result = &mut outgoing => {
                outgoing_done = true;

                tracing::warn_span!("irc.outgoing.stopped").in_scope(|| match result {
                    Ok(()) => warn!("the IRC connection write path stopped"),
                    Err(error) => warn!(exception.message = %error, "the IRC connection write path stopped"),
                });

                if dispatch_error.is_none() {
                    dispatch_error = Some(Error::IrcDisconnected {
                        reason: "the IRC connection write path stopped".to_owned(),
                    });
                }
            }
        }

        // The signal branch already armed the forced-exit path; arm it for the branches that
        // ended the connection without a signal. The signal handlers stay installed after the
        // first signal, so a second one must not be delivered to nobody.
        if !shutting_down {
            exit_on_second_signal();
        }

        // Closing the plugin mailboxes makes the tasks drain their queued events and run
        // `Plugin::shutdown`; awaiting the shutdown joins them within the grace period. The
        // dispatch future only borrows the runtime, so ending the `select!` above does not
        // close the mailboxes on its own. Draining before the QUIT lets those final replies
        // reach the server while the connection is still open.
        // A short span for the drain; the per-plugin shutdown spans are children of this span
        // (set explicitly from the plugin tasks), so the drain and every hook end up in one
        // trace.
        let shutdown = tracing::info_span!("zeta.shutdown");
        runtime.shutdown(SHUTDOWN_GRACE).instrument(shutdown).await;

        if shutting_down && let Err(error) = client.send_quit(&self.config.irc.quit_message) {
            tracing::warn_span!("irc.quit").in_scope(|| {
                warn!(exception.message = %error, "failed to send the QUIT message");
            });
        }

        // Dropping the stream closes the incoming half and its sender; dropping the last client
        // handle closes the outgoing channel, so the write task can flush the queued QUIT and
        // finish.
        drop(stream);
        drop(client);

        if shutting_down && !outgoing_done {
            let flush = tracing::info_span!("irc.quit.flush");

            match tokio::time::timeout(QUIT_FLUSH_DELAY, &mut outgoing)
                .instrument(flush)
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    warn!(exception.message = %error, "the IRC connection write path failed while flushing the QUIT message");
                }
                Err(_) => warn!("timed out flushing the QUIT message"),
            }
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
