use async_trait::async_trait;
use irc::client::Client;
use serde::de::DeserializeOwned;

use crate::Error;
use crate::context::Context;
use crate::event::{
    CommandEvent, CtcpEvent, JoinEvent, KickEvent, MessageEvent, NickEvent, PartEvent, QuitEvent,
    RawEvent, Subscriptions, UrlEvent,
};

/// The base trait that all plugins must implement.
///
/// A plugin declares the events it wants to receive in [`Plugin::new`] by registering them on
/// the [`Subscriptions`] set the host passes in — the commands it handles, the URL hosts it
/// reacts to, and the presence, message, CTCP or raw traffic it observes. The host routes only
/// the matching events to the plugin, where each kind is handled by overloading the
/// corresponding handler method: [`Plugin::handle_command`], [`Plugin::handle_url`],
/// [`Plugin::handle_join`], and so on. Handlers the plugin did not register are never called;
/// their default implementations do nothing.
///
/// A plugin's configuration is declared through [`Plugin::Settings`] and deserialized from its
/// `[plugins.<name>]` section by the host, which passes it to [`Plugin::new`].
///
/// Most plugins are written with the `#[plugin]` attribute macro instead of implementing this
/// trait by hand: it reads the container attributes and the `#[command]`/`#[handler]` method
/// annotations off an inherent impl block and generates the implementation, including the
/// [`Plugin::NAME`] constant and the command dispatch.
///
/// Each plugin runs in its own long-lived task, and the calls to a plugin's handlers are
/// serialized. Handlers therefore must not block the task; spawn a task for work that outlives
/// the call. Plugins that need to share state with other plugins can publish it through their
/// context.
///
///# Examples
///
/// ```
/// use argh::{ArgsInfo, FromArgs};
/// use irc::client::Client;
/// use zeta_plugin::{Error, prelude::*};
///
/// /// Greets the named person.
/// #[derive(Default)]
/// struct Greet;
///
/// /// The greeting options.
/// #[derive(FromArgs, ArgsInfo)]
/// struct Opts {
///     /// the name of the person to greet
///     #[argh(positional)]
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
/// A plugin reacts to more than commands by registering the matching event kinds:
///
/// ```
/// use irc::client::Client;
/// use zeta_plugin::{Error, prelude::*};
///
/// /// Watches the channel traffic.
/// struct Watcher;
///
/// #[plugin(name = "watcher", urls(any), messages)]
/// impl Watcher {
///     fn new(
///         _: &Context,
///         _: &NoSettings,
///         subscriptions: &mut Subscriptions,
///     ) -> Result<Self, Error> {
///         // `urls` and `messages` cover their kinds; join events need an explicit registration.
///         subscriptions.receive_join();
///         Ok(Self)
///     }
///
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
#[async_trait]
pub trait Plugin: Send + Sync {
    /// The plugin's name, e.g. `alert`.
    ///
    /// The name identifies the plugin in the help system and the plugin catalog, and must match
    /// the plugin's module name. The `#[plugin]` attribute macro sets it from its `name`
    /// argument and submits a [`PluginRegistration`](crate::PluginRegistration) under it, so the
    /// host discovers every compiled-in plugin without a central list.
    const NAME: &'static str;

    /// This plugin's deserialized `[plugins.<name>]` settings.
    ///
    /// The host hands only the plugin's own settings to [`Plugin::new`]; plugins cannot access
    /// other plugins' configuration.
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
        _ctx: &Context,
        _settings: &Self::Settings,
        _subscriptions: &mut Subscriptions,
    ) -> Result<Self, Error>
    where
        Self: Sized;

    /// Handles a command invocation.
    ///
    /// Called when a channel message's first word matches the trigger of one of the commands
    /// registered in [`Plugin::new`]. The event carries the
    /// [`CommandSpec`](super::CommandSpec) that matched and the trailing arguments of the
    /// invocation.
    ///
    /// For plugins written with the `#[plugin]` macro, the generated implementation matches the
    /// event's trigger against each `#[command]`'s trigger, parses the arguments, and calls the
    /// annotated method — one method per command. Hand-written plugins that register several
    /// commands can instead dispatch on the identity of their declared constants (see
    /// [`CommandSpec`](super::CommandSpec)'s documentation on matching by identity):
    ///
    /// ```
    /// use irc::client::Client;
    /// use zeta_plugin::{Error, prelude::*};
    ///
    /// # #[derive(Default)]
    /// # struct MyPlugin;
    /// #[plugin(name = "my_plugin")]
    /// impl MyPlugin {
    ///     #[command(".foo", "Handle `.foo`")]
    ///     async fn foo(&self, _: &Context, _: &Client, _channel: &str) -> Result<(), Error> {
    ///         // Handle `.foo`.
    ///         Ok(())
    ///     }
    ///
    ///     #[command(".bar", "Handle `.bar`")]
    ///     async fn bar(&self, _: &Context, _: &Client, _channel: &str) -> Result<(), Error> {
    ///         // Handle `.bar`.
    ///         Ok(())
    ///     }
    /// }
    /// ```
    ///
    /// Overriding `handle_command` by hand — for example with `#[handler(command)]` — serves
    /// every registered command from one method instead.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the command failed; the error is logged and never
    /// propagated to the other plugins.
    async fn handle_command(
        &self,
        _ctx: &Context,
        _client: &Client,
        _command: &CommandEvent,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a URL posted in a channel.
    ///
    /// Called once per extracted URL whose host the plugin registered (or for every URL, for
    /// plugins that registered with [`Subscriptions::urls`]). The URL has already been
    /// deduplicated and checked against the shared URL filters.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the URL failed; the error is logged and never propagated.
    async fn handle_url(
        &self,
        _ctx: &Context,
        _client: &Client,
        _url: &UrlEvent,
    ) -> Result<(), Error> {
        Ok(())
    }

    // The per-kind `handle_message`..`handle_raw` defaults below stay hand-written: they
    // cannot be generated from the event kind table, because `#[async_trait]` expands before
    // the trait body's macro invocations — the generated `async fn`s would never be desugared
    // and would mismatch every `#[async_trait]`-generated impl (lifetime error E0195).
    /// Handles a channel message.
    ///
    /// Called for every channel `PRIVMSG` that is not a CTCP message, whether or not it
    /// matches a registered command — for plugins that need to observe all traffic (e.g.
    /// message history or free-form text parsing).
    ///
    /// # Errors
    ///
    /// Returns an error if handling the message failed; the error is logged and never
    /// propagated.
    async fn handle_message(
        &self,
        _ctx: &Context,
        _client: &Client,
        _message: &MessageEvent,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a user joining a channel.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_join(
        &self,
        _ctx: &Context,
        _client: &Client,
        _join: &JoinEvent,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a user leaving a channel.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_part(
        &self,
        _ctx: &Context,
        _client: &Client,
        _part: &PartEvent,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a user quitting the network.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_quit(
        &self,
        _ctx: &Context,
        _client: &Client,
        _quit: &QuitEvent,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a user changing their nickname.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_nick(
        &self,
        _ctx: &Context,
        _client: &Client,
        _nick: &NickEvent,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a user being kicked from a channel.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_kick(
        &self,
        _ctx: &Context,
        _client: &Client,
        _kick: &KickEvent,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a CTCP request or reply.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_ctcp(
        &self,
        _ctx: &Context,
        _client: &Client,
        _ctcp: &CtcpEvent,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a raw, unmodeled IRC command.
    ///
    /// Only delivered to plugins that registered with [`Subscriptions::receive_raw`].
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_raw(
        &self,
        _ctx: &Context,
        _client: &Client,
        _raw: &RawEvent,
    ) -> Result<(), Error> {
        Ok(())
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
    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), Error> {
        Ok(())
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
    ///# Examples
    ///
    /// ```
    /// use irc::client::Client;
    /// use zeta_plugin::{Error, prelude::*};
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
    async fn shutdown(&mut self, _ctx: &Context, _client: &Client) -> Result<(), Error> {
        Ok(())
    }
}
