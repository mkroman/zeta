use std::future::Future;

use irc::client::Client;
use serde::de::DeserializeOwned;

use crate::Error;
use crate::context::Context;
use crate::event::{Event, Subscriptions};

/// Construction-time metadata and the constructor for a plugin type.
///
/// Kept separate from [`Plugin`] so the settings type and constructor are only needed once, when
/// the host builds the plugin from its configuration; the running plugin is handed to the
/// runtime as a plain value. The facade's generated registry calls it once per variant.
///
/// The `#[plugin]` attribute macro generates this implementation, including [`Registered::NAME`]
/// and the constructor that registers the plugin's subscriptions.
pub trait Registered: Plugin + Sized + 'static {
    /// The plugin's name, e.g. `alert`.
    ///
    /// The name identifies the plugin in the help system and the plugin catalog, and must match
    /// the plugin's module name. The `#[plugin]` attribute macro sets it from its `name`
    /// argument; the facade's generated registry lists the same name, so the host discovers
    /// every compiled-in plugin without a runtime registration mechanism.
    const NAME: &'static str;

    /// This plugin's deserialized `[plugins.<name>]` settings.
    ///
    /// The host hands only the plugin's own settings to [`Registered::new`]; plugins cannot
    /// access other plugins' configuration.
    type Settings: Default + DeserializeOwned;

    /// The constructor for a new plugin.
    ///
    /// `settings` holds the plugin's own configuration section, deserialized from
    /// `[plugins.<name>]` at startup. Register the events the plugin handles on
    /// `subscriptions` — the host routes only those events to the plugin, and publishes the
    /// registered commands and URL hosts through the plugin catalog for other plugins to see.
    ///
    /// The subscriptions are only committed when this function returns `Ok`; a plugin that
    /// fails to initialize is skipped entirely.
    ///
    /// # Errors
    ///
    /// Returns an error if the plugin cannot be initialized (e.g. missing environment
    /// variables, failed HTTP client creation). The registry will log the error and skip
    /// loading the plugin.
    fn new(
        ctx: &Context,
        settings: &Self::Settings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, Error>;
}

/// The behaviour shared by every plugin: one event handler and the lifecycle hooks.
///
/// A plugin declares the events it wants to receive during construction, through the
/// [`Subscriptions`] it registers — the commands it handles, the URL hosts it reacts to, and the
/// presence, message, CTCP or raw traffic it observes. The host routes only the matching events
/// to the plugin and calls [`Plugin::handle`] with each of them; the handler matches the
/// [`Event`] to reach the plugin's own per-kind code.
///
/// Most plugins are written with the `#[plugin]` attribute macro instead of implementing this
/// trait by hand: it reads the container attributes and the `#[command]`/`#[handler]` method
/// annotations off an inherent impl block and generates both implementations, including the
/// name constant, the settings plumbing, the single [`Plugin::handle`] match and the command
/// dispatch.
///
/// Each plugin runs in its own long-lived task, and the calls to a plugin's handlers are
/// serialized. Handlers therefore must not block the task; spawn a task for work that outlives
/// the call. Plugins that need to share state with other plugins can publish it through their
/// context.
///
/// The trait is closed at three methods and is deliberately not object-safe: the host keeps one
/// statically known plugin value per variant of the facade's generated enum and awaits the
/// handler future inline, so no plugin is boxed behind `dyn` and no future is allocated. A
/// hand-written implementation therefore has to spell the `impl Future + Send` return types the
/// macro would generate.
///
/// # Examples
///
/// ```
/// use clap::Parser;
/// use zeta_plugin::{Client, Context, Error, plugin};
///
/// /// Greets the named person.
/// #[derive(Default)]
/// struct Greet;
///
/// /// The greeting options.
/// #[derive(Parser)]
/// struct Opts {
///     /// the name of the person to greet
///     name: String,
/// }
///
/// #[plugin(name = "greet")]
/// impl Greet {
///     #[command(".hello", "Greet someone", args = Opts)]
///     async fn hello(
///         &self,
///         _: &Context,
///         client: &Client,
///         channel: &str,
///         opts: Opts,
///     ) -> Result<(), Error> {
///         client.send_privmsg(channel, format!("hello, {}!", opts.name))?;
///
///         Ok(())
///     }
/// }
/// ```
///
/// A plugin reacts to more than commands by declaring a handler for the event kind:
///
/// ```
/// use zeta_plugin::{Client, Context, Error, JoinEvent, MessageEvent, UrlEvent, plugin};
///
/// /// Watches the channel traffic.
/// #[derive(Default)]
/// struct Watcher;
///
/// #[plugin(name = "watcher", urls(any))]
/// impl Watcher {
///     #[handler(url)]
///     async fn url(&self, _: &Context, _: &Client, url: &UrlEvent) -> Result<(), Error> {
///         // Every URL posted in any channel — already filtered, deduplicated, and repaired.
///         Ok(())
///     }
///
///     #[handler(join)]
///     async fn join(&self, _: &Context, _: &Client, join: &JoinEvent) -> Result<(), Error> {
///         // Someone joined `join.channel()`.
///         Ok(())
///     }
///
///     #[handler(message)]
///     async fn message(
///         &self,
///         _: &Context,
///         _: &Client,
///         message: &MessageEvent,
///     ) -> Result<(), Error> {
///         // Every channel message, whether or not it matches a command.
///         Ok(())
///     }
/// }
/// ```
///
/// A plugin that serves several commands from one method — or wants the matched
/// [`CommandSpec`](crate::CommandSpec) itself — takes the event with `#[handler(command)]`:
///
/// ```
/// use zeta_plugin::{Client, CommandEvent, Context, Error, plugin, send_reply};
///
/// /// Handles `.foo` and `.bar` from one method.
/// #[derive(Default)]
/// struct Multi;
///
/// #[plugin(name = "multi")]
/// impl Multi {
///     #[handler(command)]
///     async fn command(
///         &self,
///         _: &Context,
///         client: &Client,
///         command: &CommandEvent,
///     ) -> Result<(), Error> {
///         send_reply(client, command.channel(), format!("got {}", command.spec.trigger()))?;
///
///         Ok(())
///     }
/// }
/// ```
pub trait Plugin: Send + Sync {
    /// Handles one event the plugin subscribed to.
    ///
    /// The dispatcher only delivers events whose kind the plugin registered, so a handler that
    /// matches every event kind may still assume the others never arrive. Commands carry the
    /// [`CommandSpec`](crate::CommandSpec) that matched and the trailing arguments of the
    /// invocation; URLs are already deduplicated and checked against the shared URL filters.
    ///
    /// For plugins written with the `#[plugin]` macro, the generated implementation matches the
    /// event's kind, routes commands by trigger, parses their arguments, and calls the annotated
    /// method — one method per command or event kind.
    ///
    /// The default implementation ignores every event. It exists for hand-written plugins that
    /// only need the lifecycle hooks; the `#[plugin]` macro always generates an implementation.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated
    /// to the other plugins.
    fn handle<'a>(
        &'a self,
        _ctx: &'a Context,
        _client: &'a Client,
        _event: &'a Event,
    ) -> impl Future<Output = Result<(), Error>> + Send + 'a {
        async { Ok(()) }
    }

    /// Called once when the plugin's task starts, after the client has connected to the network.
    ///
    /// Each plugin's hook runs in its own task, so it is a per-plugin startup hook rather than a
    /// barrier that waits for every plugin to load. This is useful for setting up plugins with
    /// async state.
    ///
    /// # Errors
    ///
    /// Returns an error if the plugin failed to load; the error is logged and the plugin task
    /// exits without processing any events.
    fn loaded<'a>(
        &'a mut self,
        _ctx: &'a Context,
        _client: &'a Client,
    ) -> impl Future<Output = Result<(), Error>> + Send + 'a {
        async { Ok(()) }
    }

    /// Called once while the bot is shutting down, after the plugin's event queue has been
    /// drained and before its task is stopped.
    ///
    /// Shutdown begins when the host receives a `SIGTERM` (e.g. from a container orchestrator
    /// deleting the pod) or `SIGINT` (e.g. `Ctrl-C`). Use this hook to flush state to durable
    /// storage or to finish work that must not be cut short. The host enforces a shutdown grace
    /// period, so the hook should not block indefinitely.
    ///
    /// The default implementation does nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// use zeta_plugin::{Client, Context, Error, plugin};
    ///
    /// /// Flushes pending state on shutdown.
    /// #[derive(Default)]
    /// struct FlushPlugin;
    ///
    /// #[plugin(name = "flush")]
    /// impl FlushPlugin {
    ///     #[handler(shutdown)]
    ///     async fn flush(&mut self, _: &Context, _client: &Client) -> Result<(), Error> {
    ///         // Flush any pending state to durable storage here.
    ///         Ok(())
    ///     }
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if flushing failed; the error is logged and the shutdown continues.
    fn shutdown<'a>(
        &'a mut self,
        _ctx: &'a Context,
        _client: &'a Client,
    ) -> impl Future<Output = Result<(), Error>> + Send + 'a {
        async { Ok(()) }
    }
}
