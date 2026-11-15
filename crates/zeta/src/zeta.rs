//! The host process for communicating over IRC and managing state.
//!
//! [`Zeta`] owns the long-lived host state: it constructs the plugin registry from the
//! configuration and runs the IRC connection loop. The lifecycle helpers live in sibling
//! modules: `connection` for the connection setup and the incoming and outgoing paths, `signal`
//! for the signal-driven shutdown, and `mirror` for the optional shared mirror's download
//! manager. Cross-cutting lifecycle tests belong in `tests`.

mod connection;
#[cfg(feature = "mirror")]
mod mirror;
mod signal;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use irc::client::prelude::{Capability, Client};
use tracing::{Instrument, debug, info, warn};

use crate::Error;
use crate::config::Config;
use crate::consts::{QUIT_FLUSH_DELAY, SHUTDOWN_GRACE, STARTUP_GRACE};
use zeta_plugin::Context;
use zeta_plugin_runtime::{PluginRuntime, Registry};

use plugins::BundledPlugin;

use self::connection::{connect_span, dispatch_messages, drive_outgoing};
#[cfg(feature = "mirror")]
use self::mirror::start_mirror;
use self::signal::{exit_on_second_signal, note_signal, shutdown_signal};

/// The main IRC bot struct that manages connection state and message handling.
pub struct Zeta {
    /// The complete configuration loaded from file or environment
    config: Config,
    /// The plugin registry holding every loaded plugin
    registry: Registry<BundledPlugin>,
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
                context.shared().publish(Arc::new(mirror));
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
            info!(zeta.plugin.count = registry.len(), "plugins registered");
        } else {
            warn!(
                zeta.plugin.count = registry.len(),
                zeta.plugin.failed.names = ?failed,
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
    /// Each plugin is spawned into its own long-lived task. Incoming messages are not dispatched
    /// until every plugin's `loaded` hook has finished or the startup grace (`STARTUP_GRACE`)
    /// expires. Messages are dispatched to every plugin through a bounded mailbox, so a slow or
    /// failing plugin cannot block the IRC connection or the other plugins: once its mailbox is
    /// full, further events for that plugin are shed. Plugin errors, including failures during
    /// loading, are logged but never propagated.
    ///
    /// # Shutdown behavior
    ///
    /// The bot shuts down gracefully on `SIGINT` (`Ctrl-C`) and, on unix, `SIGTERM` — e.g. when a
    /// container orchestrator deletes the pod running the bot. On a signal, the plugin tasks
    /// drain their queued messages and run [`Plugin::shutdown`] first, then a `QUIT` with the
    /// configured message ([`IrcConfig::quit_message`]) is sent to the server. A signal received
    /// during the startup wait for the `loaded` hooks takes the same path and skips dispatch. The
    /// plugin tasks share a grace period of `SHUTDOWN_GRACE`; stragglers are aborted. A second
    /// signal during the grace period exits the process immediately, whether the shutdown was
    /// triggered by a signal or by the connection ending. The same drain runs when the IRC stream
    /// ends, or an error propagates; a connection that ends without a shutdown signal is reported
    /// as an error so the process exits non-zero.
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
        // The shared mirror's download manager is started here, before the IRC connection is
        // established: building it sweeps stale download directories and creates the download
        // directory, synchronous filesystem work that would otherwise block a runtime worker
        // while the first mirrored URL is handled. Nothing is dispatched yet either, because the
        // plugin tasks only start below.
        #[cfg(feature = "mirror")]
        start_mirror(self.context.shared());

        // The connection setup — DNS, TCP/TLS, capability negotiation and registration — gets one
        // bounded `irc.connect` span. The connection itself has no span; the dispatcher and every
        // reply open their own short spans, so nothing hangs from a never-closing root.
        let (client, mut stream, mut outgoing) = async {
            let mut client = Client::from_config((&self.config.irc).into())
                .await
                .map_err(Error::IrcClient)?;

            // Request the IRCv3 message tags so servers attach tags such as `msgid` to delivered
            // messages. A server that does not support `message-tags` answers with a `NAK`, which
            // is harmless; registration continues without tags.
            client
                .send_cap_req(&[Capability::Custom("message-tags")])
                .map_err(Error::IrcRegistration)?;

            client.identify().map_err(Error::IrcRegistration)?;

            let outgoing = drive_outgoing(&mut client);

            let stream = client.stream()?;

            Ok::<_, Error>((client, stream, outgoing))
        }
        .instrument(connect_span(&self.config.irc))
        .await?;

        let client = Arc::new(client);

        // Each plugin gets its own long-lived task with a bounded mailbox, so a slow or failing
        // plugin cannot block the IRC connection or the other plugins; a full mailbox sheds
        // events instead of growing.
        let mut runtime =
            PluginRuntime::start(self.registry.take_plugins(), &self.context, &client);

        // The plugin tasks start their `loaded` hooks as soon as they are spawned. Wait for them
        // — filters loaded, caches warmed — before dispatching, so the first messages do not run
        // against empty state. The grace bounds a stalled hook; the runtime then dispatches with
        // whatever loaded. The short span makes the timeout warning exportable: `run` has no
        // process-lifetime span, and events outside a span are dropped.
        //
        // The shutdown future is created and pinned before the wait: `shutdown_signal` installs
        // the signal listeners on its first poll, so it must be selected alongside the wait. A
        // signal received during that window would otherwise use the default disposition and
        // terminate the process without draining the plugins or sending `QUIT`.
        let mut shutdown = Box::pin(shutdown_signal());

        let mut shutting_down = false;
        let mut dispatch_error = None;
        let mut outgoing_done = false;

        tokio::select! {
            () = runtime
                .wait_loaded(STARTUP_GRACE)
                .instrument(tracing::info_span!("zeta.startup")) => {}
            signal = &mut shutdown => {
                note_signal(signal);
                shutting_down = true;
            }
        }

        // Dispatching incoming IRC messages as events to the plugins that registered interest,
        // until the stream ends, the write path stops, or a shutdown signal arrives. A signal
        // during the startup wait skips dispatch and goes straight to the shutdown below.
        if !shutting_down {
            let dispatch = dispatch_messages(&mut stream, &mut runtime);

            tokio::select! {
                result = dispatch => {
                    if let Err(error) = result {
                        dispatch_error = Some(error);
                    }
                }
                signal = &mut shutdown => {
                    note_signal(signal);
                    shutting_down = true;
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
        }

        // The signal branches already armed the forced-exit path; arm it for the branches that
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

        // Dropping the stream closes the incoming half. The outgoing task cannot finish from the
        // sender side: the `irc` crate's transport (and its pinger) holds a clone of the outgoing
        // sender, so the receiver never observes a closed channel. The wait below is therefore a
        // flush grace for the queued QUIT, not a completion wait; abort the task afterwards.
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
                Err(_) => {
                    // The QUIT has had the full grace to reach the socket. Abort the write task
                    // so it does not outlive the shutdown; the process is about to exit.
                    outgoing.abort();
                    let _ = outgoing.await;
                    debug!("outgoing task still running after the QUIT flush grace; aborted");
                }
            }
        }

        dispatch_error.map_or_else(|| Ok(()), Err)
    }
}
