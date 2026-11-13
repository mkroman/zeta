//! The main process for communicating over IRC and managing state.
use std::sync::Arc;

use irc::client::prelude::Client;
use tracing::{Instrument, debug, error, info, trace, warn};

use crate::Error;
use crate::config::Config;
use crate::consts::{QUIT_FLUSH_DELAY, SHUTDOWN_GRACE, STARTUP_GRACE};
use zeta_plugin::Context;
use zeta_plugin::event::MessagingOperation;
use zeta_plugin_runtime::{PluginRuntime, Registry};

use plugins::BundledPlugin;

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

/// Returns the IRC command name when the event index routes it, for telemetry.
///
/// The classification is the runtime's, so this span decision cannot drift from dispatch;
/// `ERROR` is the one addition because its disconnect reason is logged inside the span even
/// though it is not routed.
fn routed_command_name(command: &irc::proto::Command) -> Option<&str> {
    if matches!(command, irc::proto::Command::ERROR(..)) {
        return Some("ERROR");
    }

    zeta_plugin_runtime::routed_command_name(command)
}

/// Returns the `OTel` messaging operation when the event index routes the command, for telemetry.
///
/// Like [`routed_command_name`], the classification is the runtime's; `ERROR` is the one
/// addition because its disconnect reason is logged inside the span even though it is not
/// routed.
fn messaging_operation(command: &irc::proto::Command) -> Option<MessagingOperation<'_>> {
    if matches!(command, irc::proto::Command::ERROR(..)) {
        return Some(MessagingOperation::Known("error"));
    }

    zeta_plugin_runtime::routed_messaging_operation(command)
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

/// The telemetry fields of one IRC line, computed once before its dispatch span is created.
///
/// The route classification is shared with the dispatcher, so the span records exactly the
/// operations the runtime routes; the channel and IRCv3 message id are read from the message
/// once. The span carries the OpenTelemetry messaging attributes alongside the transitional
/// `irc.*` keys.
struct DispatchMeta<'a> {
    /// The IRC command, recorded as `irc.command`.
    command: &'a str,
    /// The `OTel` messaging operation, recorded as `messaging.operation.name`.
    operation: MessagingOperation<'a>,
    /// The channel the message targets, recorded as `irc.channel` and
    /// `messaging.destination.name`.
    channel: Option<&'a str>,
    /// The IRCv3 message identifier, recorded as `messaging.message.id`.
    message_id: Option<&'a str>,
}

impl<'a> DispatchMeta<'a> {
    /// Classifies `message` for telemetry, or returns [`None`] when it is not dispatched.
    fn of(message: &'a irc::proto::Message) -> Option<Self> {
        let command = routed_command_name(&message.command)?;
        let operation = messaging_operation(&message.command)?;

        Some(Self {
            command,
            operation,
            channel: message_channel(&message.command),
            message_id: zeta_plugin::event::message_id(message),
        })
    }

    /// Builds the per-message dispatch span.
    fn span(self) -> tracing::Span {
        let span = tracing::info_span!(
            "irc.dispatch",
            irc.command = self.command,
            irc.channel = tracing::field::Empty,
            otel.kind = "consumer",
            messaging.system.name = "irc",
            messaging.operation.name = %self.operation,
            messaging.operation.type = "process",
            messaging.destination.name = tracing::field::Empty,
            messaging.message.id = tracing::field::Empty,
        );

        if let Some(channel) = self.channel {
            span.record("irc.channel", channel);
            span.record("messaging.destination.name", channel);
        }

        if let Some(message_id) = self.message_id {
            span.record("messaging.message.id", message_id);
        }

        span
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
    runtime: &mut PluginRuntime<BundledPlugin>,
) -> Result<(), Error> {
    use futures::stream::StreamExt;

    while let Some(message) = stream.next().await.transpose()? {
        // Everything logged while this message is routed runs inside a span: the
        // OpenTelemetry layer drops events that are not in the context of a span. Only the
        // commands `EventIndex` routes get one — PING, PONG and numerics would otherwise
        // produce a trace with no children. `ERROR` is included because the disconnect reason
        // is logged below. The span is at info level, not debug, because the loop below logs
        // warnings: an `info` filter must not be what keeps them out of the backend.
        //
        // The span fields are derived only when the span is enabled, so a disabled subscriber
        // does not pay for the route classification, channel lookup and `msgid` scan.
        let dispatch = tracing::enabled!(tracing::Level::INFO)
            .then(|| DispatchMeta::of(&message))
            .flatten()
            .map(DispatchMeta::span);
        let _dispatch = dispatch.as_ref().map(tracing::Span::enter);

        // `ERROR` carries the server's reason for closing the connection. It is not a `Raw`
        // command, so without this check the reason would be dropped entirely.
        if let irc::proto::Command::ERROR(reason) = &message.command {
            warn!(
                zeta.shutdown.reason = %reason,
                "the IRC server closed the connection"
            );

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

/// Logs the signal that triggered the shutdown and arms the forced exit for a second one.
///
/// `run` has no process-lifetime span of its own, and the OpenTelemetry layer drops events that
/// are not in a span, so the one-off diagnostic gets a short span here.
fn note_signal(signal: ShutdownSignal) {
    tracing::info_span!("zeta.signal", signal = signal.name()).in_scope(|| info!("shutting down"));

    // Another signal during the grace period forces an immediate exit.
    exit_on_second_signal();
}

/// Starts the download manager of the shared mirror, when one is published.
///
/// The mirror is published to the context's shared state during construction; plugins pick up
/// their handles from there, and the host starts the manager so the first mirrored URL does not
/// pay for the manager's synchronous filesystem setup.
///
/// Returns whether the manager is running; this is `false` when no mirror is published or its
/// download directory could not be created, in which case mirroring stays disabled.
#[cfg(feature = "mirror")]
fn start_mirror(shared: &zeta_plugin::SharedState) -> bool {
    shared
        .get::<zeta_mirror::Mirror>()
        .is_some_and(|mirror| mirror.start())
}

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
        start_mirror(&self.context.shared);

        let mut client = Client::from_config((&self.config.irc).into())
            .await
            .map_err(Error::IrcClient)?;

        client.identify().map_err(Error::IrcRegistration)?;

        let mut outgoing = drive_outgoing(&mut client);

        let mut stream = client.stream()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routed_command_name_follows_the_runtime_classification() {
        use irc::proto::{Command, Response};

        let cases = [
            (
                Command::PRIVMSG("#zeta".to_owned(), "hello".to_owned()),
                Some("PRIVMSG"),
            ),
            (
                Command::NOTICE("#zeta".to_owned(), "hello".to_owned()),
                Some("NOTICE"),
            ),
            (Command::JOIN("#zeta".to_owned(), None, None), Some("JOIN")),
            (Command::PART("#zeta".to_owned(), None), Some("PART")),
            (Command::QUIT(None), Some("QUIT")),
            (Command::NICK("zeta".to_owned()), Some("NICK")),
            (
                Command::KICK("#zeta".to_owned(), "nick".to_owned(), None),
                Some("KICK"),
            ),
            (Command::Raw("BATCH".to_owned(), Vec::new()), Some("BATCH")),
            (Command::ERROR("closing link".to_owned()), Some("ERROR")),
            (Command::PING("server".to_owned(), None), None),
            (Command::PONG("server".to_owned(), None), None),
            (Command::Response(Response::RPL_WELCOME, Vec::new()), None),
        ];

        for (command, expected) in cases {
            assert_eq!(routed_command_name(&command), expected, "{command:?}");
            assert_eq!(
                messaging_operation(&command).map(|operation| operation.to_string()),
                expected.map(str::to_ascii_lowercase),
                "{command:?}"
            );

            // `ERROR` is the host's only addition to the dispatcher's classification: it gets
            // a span for its disconnect reason, but dispatches no event.
            if matches!(command, Command::ERROR(..)) {
                assert_eq!(
                    zeta_plugin_runtime::routed_command_name(&command),
                    None,
                    "{command:?}"
                );
                assert!(zeta_plugin_runtime::routed_messaging_operation(&command).is_none());
            } else {
                assert_eq!(
                    routed_command_name(&command),
                    zeta_plugin_runtime::routed_command_name(&command),
                    "{command:?}"
                );
                assert_eq!(
                    messaging_operation(&command).map(|operation| operation.to_string()),
                    zeta_plugin_runtime::routed_messaging_operation(&command)
                        .map(|operation| operation.to_string()),
                    "{command:?}"
                );
            }
        }
    }

    /// The dispatch span records the `OTel` messaging attributes next to the transitional `irc.*`
    /// keys: the operation is the lowercased command, a channel becomes the destination, and an
    /// IRCv3 `msgid` is recorded when the message carries one.
    #[test]
    fn dispatch_span_records_messaging_attributes() {
        use tracing::subscriber::with_default;
        use tracing_subscriber::prelude::*;

        let recorder = SpanRecorder::default();
        let subscriber = tracing_subscriber::registry().with(recorder.clone());

        with_default(subscriber, || {
            let privmsg = tagged_privmsg("hello", "m1");
            let span = DispatchMeta::of(&privmsg)
                .expect("a privmsg is dispatched")
                .span();
            drop(span);

            let quit = message("nick!u@h", "QUIT", &["gone"]);
            let span = DispatchMeta::of(&quit)
                .expect("a quit is dispatched")
                .span();
            drop(span);

            let ping = message("server.example", "PING", &["server"]);
            assert!(
                DispatchMeta::of(&ping).is_none(),
                "PING is not dispatched and must not get a span"
            );
        });

        let spans = recorder.spans();
        assert_eq!(spans.len(), 2, "recorded: {spans:?}");

        let privmsg = &spans[0];
        assert_eq!(privmsg.field("irc.command"), Some("PRIVMSG"));
        assert_eq!(privmsg.field("irc.channel"), Some("#test"));
        assert_eq!(privmsg.field("otel.kind"), Some("consumer"));
        assert_eq!(privmsg.field("messaging.system.name"), Some("irc"));
        assert_eq!(privmsg.field("messaging.operation.name"), Some("privmsg"));
        assert_eq!(privmsg.field("messaging.operation.type"), Some("process"));
        assert_eq!(privmsg.field("messaging.destination.name"), Some("#test"));
        assert_eq!(privmsg.field("messaging.message.id"), Some("m1"));

        let quit = &spans[1];
        assert_eq!(quit.field("irc.command"), Some("QUIT"));
        assert_eq!(quit.field("messaging.operation.name"), Some("quit"));
        assert_eq!(quit.field("messaging.operation.type"), Some("process"));
        assert!(
            quit.field("messaging.destination.name").is_none(),
            "a channel-less command has no destination: {quit:?}"
        );
        assert!(
            quit.field("messaging.message.id").is_none(),
            "a quit carries no msgid: {quit:?}"
        );
    }

    /// Builds a `PRIVMSG` to `#test` carrying `text` and an IRCv3 `msgid` tag.
    fn tagged_privmsg(text: &str, msgid: &str) -> irc::proto::Message {
        irc::proto::Message::with_tags(
            Some(vec![irc::proto::message::Tag(
                "msgid".to_owned(),
                Some(msgid.to_owned()),
            )]),
            Some("nick!user@host.example"),
            "PRIVMSG",
            vec!["#test", text],
        )
        .expect("message should parse")
    }

    /// Builds a message from `prefix` carrying `command` with `args`.
    fn message(prefix: &str, command: &str, args: &[&str]) -> irc::proto::Message {
        let prefix = (!prefix.is_empty()).then_some(prefix);

        irc::proto::Message::new(prefix, command, args.to_vec()).expect("message should parse")
    }

    /// One span as the recorder observed it.
    #[derive(Clone, Debug)]
    struct RecordedSpan {
        /// The span's identity, used to merge fields recorded after creation.
        id: tracing::span::Id,
        /// Every field recorded on the span, at creation or later.
        fields: std::collections::BTreeMap<String, String>,
    }

    impl RecordedSpan {
        /// Returns the string value of `field`.
        fn field(&self, field: &str) -> Option<&str> {
            self.fields.get(field).map(String::as_str)
        }
    }

    /// Records every span created while it is installed.
    #[derive(Clone, Default)]
    struct SpanRecorder(Arc<std::sync::Mutex<Vec<RecordedSpan>>>);

    impl SpanRecorder {
        /// Returns a copy of the spans recorded so far.
        fn spans(&self) -> Vec<RecordedSpan> {
            self.0
                .lock()
                .expect("span recorder should not be poisoned")
                .clone()
        }
    }

    /// Collects the fields of a span, rendered as their exported values.
    #[derive(Default)]
    struct Fields(std::collections::BTreeMap<String, String>);

    impl tracing::field::Visit for Fields {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().to_owned(), value.to_owned());
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().to_owned(), format!("{value:?}"));
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::layer::Layer<S> for SpanRecorder {
        fn on_new_span(
            &self,
            attrs: &tracing::span::Attributes<'_>,
            id: &tracing::span::Id,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut fields = Fields::default();
            attrs.record(&mut fields);

            self.0
                .lock()
                .expect("span recorder should not be poisoned")
                .push(RecordedSpan {
                    id: id.clone(),
                    fields: fields.0,
                });
        }

        fn on_record(
            &self,
            id: &tracing::span::Id,
            values: &tracing::span::Record<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut fields = Fields::default();
            values.record(&mut fields);

            self.0
                .lock()
                .expect("span recorder should not be poisoned")
                .iter_mut()
                .find(|span| span.id == *id)
                .expect("recorded spans should exist")
                .fields
                .extend(fields.0);
        }
    }

    /// A mirror whose manager can be started without network access.
    #[cfg(feature = "mirror")]
    fn test_mirror(download_dir: &std::path::Path) -> Arc<zeta_mirror::Mirror> {
        let config = zeta_mirror::MirrorConfig {
            download_dir: Some(download_dir.to_path_buf()),
            s3_access_key_id: Some("access".to_string()),
            s3_secret_access_key: Some("secret".into()),
            s3_bucket_name: Some("bucket".to_string()),
            ..zeta_mirror::MirrorConfig::default()
        };

        Arc::new(
            zeta_mirror::Mirror::from_config(&config)
                .expect("the test configuration should be valid"),
        )
    }

    #[cfg(feature = "mirror")]
    #[tokio::test]
    async fn start_mirror_starts_the_published_manager() {
        let dir = tempfile::tempdir().unwrap();
        let shared = zeta_plugin::SharedState::new();

        assert!(
            !start_mirror(&shared),
            "Nothing is published, so there is nothing to start."
        );

        shared.publish(test_mirror(dir.path()));
        assert!(
            start_mirror(&shared),
            "startup should start the published mirror's manager"
        );

        assert!(start_mirror(&shared), "A second startup call is a no-op.");
    }

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
