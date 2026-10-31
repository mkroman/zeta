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
use tokio::task::JoinHandle;
use tracing::{Span, error, warn};

use zeta_plugin::{Context, Filters, LoadedPlugin};

use crate::dispatch::EventIndex;
use crate::task::PluginTask;

/// The running plugins: their dispatch index and their tasks.
pub struct PluginRuntime {
    /// Routes incoming messages to the plugins that registered interest.
    index: EventIndex,
    /// The long-lived task of every running plugin.
    tasks: Vec<PluginTask>,
    /// The shared filters, resolved once after every plugin was constructed.
    ///
    /// The filter plugin publishes its handle from its constructor, so the handle exists by the
    /// time the runtime starts. The runtime keeps this handle for its lifetime; a later publish
    /// would not reach the dispatcher. Keeping it out of the per-message path avoids a
    /// shared-state lookup on every IRC line.
    filters: Filters,
    /// The runtime's `zeta.shutdown` span, used as the parent of each task's
    /// `zeta.plugin.shutdown` span. The hooks run in the plugin tasks, so the parent is set
    /// explicitly; it keeps the shutdown in one trace.
    shutdown_span: Arc<OnceLock<Span>>,
}

impl PluginRuntime {
    /// Spawns every plugin into its own long-lived task and indexes its subscriptions.
    ///
    /// Each task gets an unbounded mailbox, so a slow or failing plugin cannot block the IRC
    /// connection or the other plugins.
    ///
    /// The shared filters are resolved before the tasks are spawned: every plugin is already
    /// constructed, and the filter plugin publishes its handle from its constructor.
    #[must_use]
    pub fn start(plugins: Vec<LoadedPlugin>, ctx: &Arc<Context>, client: &Arc<Client>) -> Self {
        // The plugins are already constructed: the filter plugin published its handle from its
        // constructor, and nothing replaces it later. Resolve it once, before any task can
        // dispatch a message.
        let filters = Filters::from_context(ctx);

        let mut index = EventIndex::default();
        let mut tasks = Vec::with_capacity(plugins.len());
        let shutdown_span = Arc::new(OnceLock::new());

        for LoadedPlugin {
            name,
            plugin,
            subscriptions,
        } in plugins
        {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

            tasks.push(PluginTask::spawn(
                name,
                plugin,
                Arc::clone(ctx),
                Arc::clone(client),
                receiver,
                Arc::clone(&shutdown_span),
            ));
            index.add(name, &subscriptions, sender);
        }

        Self {
            index,
            tasks,
            filters,
            shutdown_span,
        }
    }

    /// Routes `message` to the plugins that subscribed to the events it matches.
    ///
    /// A plugin whose task has stopped is logged and removed from the index.
    pub fn dispatch(&mut self, message: Message) {
        for stopped in self.index.dispatch(&self.filters, message) {
            warn!(zeta.plugin.name = %stopped, "plugin task has stopped");
        }
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
    use url::Url;
    use zeta_plugin::{Sender, UrlFilter};

    use super::*;

    /// A filter that ignores messages from one nick and never filters a URL.
    struct IgnoresNick(&'static str);

    impl UrlFilter for IgnoresNick {
        fn is_filtered(&self, _channel: &str, _sender: Option<Sender<'_>>, _url: &Url) -> bool {
            false
        }

        fn ignores(&self, _channel: &str, sender: Option<Sender<'_>>) -> bool {
            sender.is_some_and(|sender| sender.nick == self.0)
        }
    }

    /// Builds a client on the `irc` crate's mock connection, so the test needs no server.
    async fn mock_client() -> Arc<Client> {
        let config = irc::client::prelude::Config {
            use_mock_connection: true,
            ..irc::client::prelude::Config::default()
        };

        Arc::new(
            Client::from_config(config)
                .await
                .expect("a mock connection should be created"),
        )
    }

    /// The runtime resolves the handle the filter plugin published from its constructor, and
    /// keeps it: filtering changes go through the service, not through another `publish`.
    #[tokio::test]
    async fn start_resolves_the_filters_published_by_the_constructors() {
        let ctx = Arc::new(zeta_test_support::test_context());
        ctx.shared
            .publish(Arc::new(Filters::new(Arc::new(IgnoresNick("ignored")))));

        let client = mock_client().await;
        let runtime = PluginRuntime::start(Vec::new(), &ctx, &client);
        let sender = |nick| Some(Sender::new(nick, "user", "host.example"));

        assert!(runtime.filters.ignores("#test", sender("ignored")));

        // A later publish does not replace the handle the dispatcher already holds.
        ctx.shared
            .publish(Arc::new(Filters::new(Arc::new(IgnoresNick("later")))));

        assert!(runtime.filters.ignores("#test", sender("ignored")));
        assert!(!runtime.filters.ignores("#test", sender("later")));
    }

    #[tokio::test]
    async fn drain_tasks_aborts_stragglers() {
        let finished = Arc::new(tokio::sync::Mutex::new(false));
        let flag = Arc::clone(&finished);

        let fast = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;

            *flag.lock().await = true;
        });
        let hung = tokio::spawn(futures::future::pending::<()>());

        drain_tasks(
            vec![("fast", fast), ("hung", hung)],
            Duration::from_millis(200),
        )
        .await;

        assert!(*finished.lock().await);
    }
}
