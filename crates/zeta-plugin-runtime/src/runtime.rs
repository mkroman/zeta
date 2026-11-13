//! The runtime that owns the running plugins.
//!
//! [`Registry`](crate::Registry) constructs plugins; [`PluginRuntime`] takes the constructed
//! ones, spawns each into its own long-lived task, routes events to them, and drains them on
//! shutdown. Keeping the index and the task handles together makes the shutdown protocol — drop
//! the index to close every mailbox, then await the tasks — an implementation detail instead of
//! a sequence the host has to reproduce.

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use irc::client::Client;
use irc::proto::Message;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{Span, error, warn};

use zeta_plugin::{Context, FilterHandle, LoadedPlugin};

use crate::dispatch::EventIndex;
use crate::task::PluginTask;

/// The number of events a plugin may have queued before new ones are shed.
///
/// A plugin that stops draining its mailbox — a stalled database pool, a blocked fetch, a
/// handler that never returns — would otherwise grow with every event it subscribes to. The
/// dispatcher never blocks on the mailbox: a full one drops the event and warns at most once per
/// warning interval, and the plugin is not evicted.
pub const MAILBOX_CAPACITY: usize = 4_096;

/// The running plugins: their dispatch index and their tasks.
pub struct PluginRuntime<P> {
    /// Routes incoming messages to the plugins that registered interest.
    index: EventIndex,
    /// The long-lived task of every running plugin.
    tasks: Vec<PluginTask<P>>,
    /// The shared filters, resolved once after every plugin was constructed.
    ///
    /// The filter plugin publishes its handle from its constructor, so the handle exists by the
    /// time the runtime starts. The runtime keeps this handle for its lifetime; a later publish
    /// would not reach the dispatcher. Keeping it out of the per-message path avoids a
    /// shared-state lookup on every IRC line.
    filters: FilterHandle,
    /// The startup barrier: one receiver per plugin, resolved when its `loaded` hook returns.
    ///
    /// Each task holds the matching sender and sends after the hook ran, whether it succeeded or
    /// failed. A task that panics before sending drops its sender, which resolves the receiver
    /// immediately instead of holding up dispatch for the full grace. [`Self::wait_loaded`] waits
    /// for all of them.
    startup: Vec<oneshot::Receiver<()>>,
    /// The runtime's `zeta.shutdown` span, used as the parent of each task's
    /// `zeta.plugin.shutdown` span. The hooks run in the plugin tasks, so the parent is set
    /// explicitly; it keeps the shutdown in one trace.
    shutdown_span: Arc<OnceLock<Span>>,
}

impl<P> PluginRuntime<P>
where
    P: zeta_plugin::Plugin + 'static,
{
    /// Spawns every plugin into its own long-lived task and indexes its subscriptions.
    ///
    /// Each task gets a bounded mailbox, so a slow or failing plugin cannot block the IRC
    /// connection or the other plugins. A plugin that stops draining its mailbox has further
    /// events shed instead of retained.
    ///
    /// The shared filters are resolved before the tasks are spawned: every plugin is already
    /// constructed, and the filter plugin publishes its handle from its constructor.
    ///
    /// Every task also reports on the startup barrier once its `loaded` hook has finished;
    /// [`Self::wait_loaded`] waits for all of them.
    #[must_use]
    pub fn start(plugins: Vec<LoadedPlugin<P>>, ctx: &Arc<Context>, client: &Arc<Client>) -> Self {
        // The plugins are already constructed: the filter plugin published its handle from its
        // constructor, and nothing replaces it later. Resolve it once, before any task can
        // dispatch a message.
        let filters = FilterHandle::from_context(ctx);

        let mut startup = Vec::with_capacity(plugins.len());
        let mut index = EventIndex::default();
        let mut tasks = Vec::with_capacity(plugins.len());
        let shutdown_span = Arc::new(OnceLock::new());

        for LoadedPlugin {
            name,
            plugin,
            subscriptions,
        } in plugins
        {
            let (sender, receiver) = tokio::sync::mpsc::channel(MAILBOX_CAPACITY);
            let (started, startup_signal) = oneshot::channel();
            startup.push(startup_signal);

            tasks.push(PluginTask::spawn(
                name,
                plugin,
                Arc::clone(ctx),
                Arc::clone(client),
                receiver,
                Arc::clone(&shutdown_span),
                started,
            ));
            index.add(name, &subscriptions, sender);
        }

        Self {
            index,
            tasks,
            filters,
            startup,
            shutdown_span,
        }
    }

    /// Waits until every plugin's `loaded` hook has finished, or `grace` elapses.
    ///
    /// A hook that failed still reports in: its task exits without handling events, so waiting
    /// for it any longer would only burn the grace every other plugin may need. A task that
    /// panicked drops its sender without reporting, which resolves its receiver immediately; a
    /// hook that stalls is bounded by `grace`.
    pub async fn wait_loaded(&mut self, grace: Duration) {
        let startup = std::mem::take(&mut self.startup);
        let plugin_count = startup.len();

        if plugin_count == 0 {
            return;
        }

        if let Ok(results) = tokio::time::timeout(grace, futures::future::join_all(startup)).await {
            let failed = results.iter().filter(|result| result.is_err()).count();

            if failed > 0 {
                warn!(
                    zeta.plugin.count = plugin_count,
                    zeta.plugin.failed.count = failed,
                    "plugin task stopped before its loaded hook finished"
                );
            }
        } else {
            warn!(
                zeta.plugin.count = plugin_count,
                "timed out waiting for plugins to load; dispatching with whatever loaded"
            );
        }
    }

    /// Routes `message` to the plugins that subscribed to the events it matches.
    ///
    /// The dispatcher owns the line's `irc.dispatch` span and logs a plugin whose task has
    /// stopped from inside it; the runtime only hands the message over.
    pub fn dispatch(&mut self, message: Message) {
        let _ = self.index.dispatch(&self.filters, message);
    }

    /// Closes every plugin mailbox and waits for the tasks to drain and stop.
    ///
    /// The tasks drain their queued events and run their shutdown hooks, sharing a deadline
    /// `grace` from now; any task still running when the deadline passes is aborted.
    pub async fn shutdown(self, grace: Duration) {
        // Set before the mailboxes close: each task reads it when it opens its
        // `zeta.plugin.shutdown` span, and parents that span to this one. Outside a span
        // (tests), leave it unset so the hook span is a root.
        let current = Span::current();

        if current.id().is_some() {
            let _ = self.shutdown_span.set(current);
        }

        let Self {
            index,
            tasks,
            filters: _,
            startup: _,
            shutdown_span: _,
        } = self;

        // Dropping the index closes every plugin mailbox: the tasks drain their queued events
        // and run `Plugin::shutdown`.
        drop(index);

        let handles = tasks
            .into_iter()
            .map(PluginTask::into_handle)
            .collect::<Vec<_>>();

        drain_tasks(handles, grace).await;
    }
}

/// Waits for the plugin tasks to drain their queued messages and run their shutdown hooks.
///
/// The tasks share a deadline `grace` from now; any task still running when the deadline passes
/// is aborted. The tasks are awaited concurrently, so a hanging task cannot consume the grace
/// that is owed to the other plugins.
async fn drain_tasks(mut handles: Vec<(&'static str, JoinHandle<()>)>, grace: Duration) {
    let deadline = tokio::time::Instant::now() + grace;

    let outcomes = futures::future::join_all(
        handles
            .iter_mut()
            .map(|(_, handle)| async move { tokio::time::timeout_at(deadline, handle).await }),
    )
    .await;

    for ((name, handle), outcome) in handles.into_iter().zip(outcomes) {
        match outcome {
            Ok(Ok(())) => {}
            // The task was cancelled by its own runtime (or this function, below).
            Ok(Err(error)) if error.is_cancelled() => {}
            Ok(Err(error)) => {
                error!(
                    zeta.plugin.name = %name,
                    exception.message = %error,
                    "plugin task panicked while shutting down"
                );
            }
            Err(_) => {
                warn!(zeta.plugin.name = %name, "plugin task did not shut down in time; aborting");

                handle.abort();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures::future;
    use irc::proto::Message;
    use url::Url;
    use zeta_plugin::{
        CommandSpec, Error, Event, EventFilter, LoadedPlugin, Plugin, Sender, Subscriptions,
    };

    use super::*;

    /// A filter that ignores messages from one nick and never filters a URL.
    struct IgnoresNick(&'static str);

    impl EventFilter for IgnoresNick {
        fn is_filtered(&self, _channel: &str, _sender: Option<Sender<'_>>, _url: &Url) -> bool {
            false
        }

        fn ignores(&self, _channel: &str, sender: Option<Sender<'_>>) -> bool {
            sender.is_some_and(|sender| sender.nick == self.0)
        }
    }

    /// A plugin whose `loaded` hook waits `delay` and then records that it ran.
    struct Loader {
        loaded: Arc<AtomicBool>,
        delay: Duration,
    }

    impl Plugin for Loader {
        async fn loaded<'a>(&'a mut self, _: &'a Context, _: &'a Client) -> Result<(), Error> {
            tokio::time::sleep(self.delay).await;
            self.loaded.store(true, Ordering::SeqCst);

            Ok(())
        }
    }

    /// A plugin whose `loaded` hook never finishes, modelling a stalled hook.
    struct PendingLoader(Arc<AtomicBool>);

    impl Plugin for PendingLoader {
        async fn loaded<'a>(&'a mut self, _: &'a Context, _: &'a Client) -> Result<(), Error> {
            future::pending::<()>().await;
            self.0.store(true, Ordering::SeqCst);

            Ok(())
        }
    }

    /// A plugin whose `loaded` hook panics before it can report on the startup barrier.
    struct PanicLoader;

    impl Plugin for PanicLoader {
        async fn loaded<'a>(&'a mut self, _: &'a Context, _: &'a Client) -> Result<(), Error> {
            panic!("loaded hook panic");
        }
    }

    /// Wraps `plugin` into a loaded plugin with no subscriptions.
    fn loaded_plugin<P>(name: &'static str, plugin: P) -> LoadedPlugin<P> {
        LoadedPlugin {
            name,
            plugin,
            subscriptions: Subscriptions::new(),
        }
    }

    /// The runtime resolves the handle the filter plugin published from its constructor, and
    /// keeps it: filtering changes go through the service, not through another `publish`.
    #[tokio::test]
    async fn start_resolves_the_filters_published_by_the_constructors() {
        let ctx = Arc::new(zeta_test_support::test_context());
        ctx.shared
            .publish(Arc::new(FilterHandle::new(Arc::new(IgnoresNick(
                "ignored",
            )))));

        let client = Arc::new(zeta_test_support::mock_client().await);
        let runtime = PluginRuntime::<Loader>::start(Vec::new(), &ctx, &client);
        let sender = |nick| Some(Sender::new(nick, "user", "host.example"));

        assert!(runtime.filters.ignores("#test", sender("ignored")));

        // A later publish does not replace the handle the dispatcher already holds.
        ctx.shared
            .publish(Arc::new(FilterHandle::new(Arc::new(IgnoresNick("later")))));

        assert!(runtime.filters.ignores("#test", sender("ignored")));
        assert!(!runtime.filters.ignores("#test", sender("later")));
    }

    /// A plugin subscribed to both a command and the message kind observes one IRC line as two
    /// events, in dispatcher order: the command first, then the message.
    ///
    /// This pins the interaction the mixed subscriptions rely on; a changed ordering would make
    /// a command handler and a message handler see the same line in a different order.
    #[tokio::test]
    async fn command_and_message_subscriptions_observe_one_line_in_order() {
        struct Mixed {
            seen: Arc<std::sync::Mutex<Vec<&'static str>>>,
        }

        #[allow(clippy::unused_async_trait_impl)]
        impl Plugin for Mixed {
            async fn handle(
                &self,
                _ctx: &Context,
                _client: &Client,
                event: &Event,
            ) -> Result<(), Error> {
                match event {
                    Event::Command(_) => {
                        self.seen.lock().expect("seen lock").push("command");
                    }
                    Event::Message(_) => {
                        self.seen.lock().expect("seen lock").push("message");
                    }
                    _ => {}
                }

                Ok(())
            }
        }

        let ctx = Arc::new(zeta_test_support::test_context());
        let client = Arc::new(zeta_test_support::mock_client().await);
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut subscriptions = Subscriptions::new();
        subscriptions.command(CommandSpec::new(".mixed", "mixed"));
        subscriptions.receive_message();

        let mut runtime = PluginRuntime::start(
            vec![LoadedPlugin {
                name: "mixed",
                plugin: Mixed {
                    seen: Arc::clone(&seen),
                },
                subscriptions,
            }],
            &ctx,
            &client,
        );
        runtime.wait_loaded(Duration::from_secs(5)).await;

        runtime.dispatch(
            Message::new(
                Some("nick!user@host.example"),
                "PRIVMSG",
                vec!["#test", ".mixed hello"],
            )
            .expect("message should parse"),
        );
        runtime.shutdown(Duration::from_secs(5)).await;

        assert_eq!(
            *seen.lock().expect("seen lock"),
            ["command", "message"],
            "the command event must be delivered before the message event"
        );
    }

    /// Dispatching must not start while a `loaded` hook is still building the state the handlers
    /// read; `wait_loaded` returns only after every hook has reported in.
    #[tokio::test]
    async fn wait_loaded_waits_for_the_loaded_hooks() {
        let ctx = Arc::new(zeta_test_support::test_context());
        let client = Arc::new(zeta_test_support::mock_client().await);
        let loaded = Arc::new(AtomicBool::new(false));
        let delay = Duration::from_millis(50);

        let mut runtime = PluginRuntime::start(
            vec![loaded_plugin(
                "loader",
                Loader {
                    loaded: Arc::clone(&loaded),
                    delay,
                },
            )],
            &ctx,
            &client,
        );

        assert!(!loaded.load(Ordering::SeqCst), "the hook has not finished");

        let started = tokio::time::Instant::now();
        runtime.wait_loaded(Duration::from_secs(5)).await;
        let elapsed = started.elapsed();

        println!("startup_barrier: loaded hook {delay:?}, wait_loaded returned after {elapsed:?}");

        assert!(loaded.load(Ordering::SeqCst), "the hook should have run");
        assert!(
            elapsed >= delay,
            "wait_loaded returned before the hook finished: {elapsed:?}"
        );

        runtime.shutdown(Duration::from_secs(1)).await;
    }

    /// A hook that never finishes must not stall startup forever: the grace bounds the wait and
    /// dispatch proceeds with whatever loaded.
    #[tokio::test]
    async fn wait_loaded_gives_up_after_the_grace() {
        let ctx = Arc::new(zeta_test_support::test_context());
        let client = Arc::new(zeta_test_support::mock_client().await);
        let loaded = Arc::new(AtomicBool::new(false));

        // A hook that stalls models the case the grace exists for; a panicking hook drops its
        // sender instead and is covered by its own test below.
        let mut runtime = PluginRuntime::start(
            vec![loaded_plugin("stalled", PendingLoader(Arc::clone(&loaded)))],
            &ctx,
            &client,
        );

        let started = tokio::time::Instant::now();
        runtime.wait_loaded(Duration::from_millis(50)).await;

        println!(
            "startup_barrier: 50 ms grace, stalled hook, wait_loaded returned after {:?}",
            started.elapsed()
        );

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the grace should bound the wait"
        );
        assert!(
            !loaded.load(Ordering::SeqCst),
            "the stalled hook never finished"
        );

        runtime.shutdown(Duration::ZERO).await;
    }

    /// A task that panics before reporting drops its sender, so its receiver resolves
    /// immediately: the startup wait is not held up for the full grace by a dead plugin.
    #[tokio::test]
    async fn wait_loaded_returns_when_a_loaded_hook_panics() {
        let ctx = Arc::new(zeta_test_support::test_context());
        let client = Arc::new(zeta_test_support::mock_client().await);

        let mut runtime =
            PluginRuntime::start(vec![loaded_plugin("panicking", PanicLoader)], &ctx, &client);

        let started = tokio::time::Instant::now();
        runtime.wait_loaded(Duration::from_secs(30)).await;

        println!(
            "startup_barrier: 30 s grace, panicking hook, wait_loaded returned after {:?}",
            started.elapsed()
        );

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a panicking task should resolve its receiver, not wait out the grace"
        );

        runtime.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn drain_tasks_aborts_stragglers() {
        let finished = Arc::new(tokio::sync::Mutex::new(false));
        let flag = Arc::clone(&finished);

        let fast = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;

            *flag.lock().await = true;
        });
        let hung = tokio::spawn(future::pending::<()>());

        drain_tasks(
            vec![("fast", fast), ("hung", hung)],
            Duration::from_millis(200),
        )
        .await;

        assert!(*finished.lock().await);
    }

    /// A plugin that records every event it receives, so dispatch is observable.
    struct Recorder(Arc<std::sync::Mutex<Vec<&'static str>>>);

    #[allow(clippy::unused_async_trait_impl)]
    impl Plugin for Recorder {
        async fn handle<'a>(
            &'a self,
            _: &'a Context,
            _: &'a Client,
            event: &'a Event,
        ) -> Result<(), Error> {
            self.0
                .lock()
                .expect("the recording lock is not poisoned")
                .push(event.kind_name());

            Ok(())
        }
    }

    /// Dispatching a message reaches the plugin's handler through its mailbox.
    #[tokio::test]
    async fn dispatch_reaches_the_plugin_handler() {
        let ctx = Arc::new(zeta_test_support::test_context());
        let client = Arc::new(zeta_test_support::mock_client().await);
        let recording = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut subscriptions = Subscriptions::new();
        subscriptions.receive_message();

        let mut runtime = PluginRuntime::start(
            vec![LoadedPlugin {
                name: "recorder",
                plugin: Recorder(Arc::clone(&recording)),
                subscriptions,
            }],
            &ctx,
            &client,
        );

        runtime.dispatch(
            Message::new(Some("nick!user@host"), "PRIVMSG", vec!["#test", "hello"])
                .expect("message should parse"),
        );
        runtime.shutdown(Duration::from_secs(1)).await;

        assert_eq!(
            *recording
                .lock()
                .expect("the recording lock is not poisoned"),
            ["message"]
        );
    }
}
