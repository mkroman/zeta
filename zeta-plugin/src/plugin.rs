use async_trait::async_trait;
use irc::client::Client;

use crate::event::{
    CommandEvent, CtcpEvent, JoinEvent, KickEvent, MessageEvent, NickEvent, PartEvent, QuitEvent,
    RawEvent, Subscriptions, UrlEvent,
};
use crate::Error;

/// Supplies the name of a plugin.
///
/// The name identifies the plugin in the help system and the plugin catalog, and must match the
/// plugin's module name.
pub trait PluginName {
    /// The plugin's name, e.g. `alert`.
    const NAME: &'static str;
}

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
/// Each plugin runs in its own long-lived task, and the calls to a plugin's handlers are
/// serialized. Handlers therefore must not block the task; spawn a task for work that outlives
/// the call. Plugins that need to share state with other plugins can publish it through their
/// context.
///
///# Examples
///
/// ```
/// use irc::client::Client;
/// use zeta_plugin::{CommandSpec, Error, Subscriptions, prelude::*};
///
/// struct Greet;
///
/// impl PluginName for Greet {
///     const NAME: &'static str = "greet";
/// }
///
/// const HELLO: CommandSpec = CommandSpec::new(".hello", "Greet someone");
///
///#[async_trait]
/// impl Plugin for Greet {
///     type Settings = NoSettings;
///
///     fn new(_: &(), _: &NoSettings, subscriptions: &mut Subscriptions) -> Result<Greet, Error> {
///         subscriptions.command(HELLO);
///
///         Ok(Greet)
///     }
///
///     async fn handle_command(
///         &self,
///         _ctx: &(),
///         client: &Client,
///         command: &CommandEvent,
///     ) -> Result<(), Error> {
///         let nick = command.args().split_whitespace().next().unwrap_or("world");
///         client.send_privmsg(command.channel(), format!("hello, {nick}!"))?;
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
/// use zeta_plugin::{Error, Subscriptions, prelude::*};
///
/// struct Watcher;
///
/// impl PluginName for Watcher {
///     const NAME: &'static str = "watcher";
/// }
///
///#[async_trait]
/// impl Plugin for Watcher {
///     type Settings = NoSettings;
///
///     fn new(_: &(), _: &NoSettings, subscriptions: &mut Subscriptions) -> Result<Watcher, Error> {
///         subscriptions.urls(UrlScope::Any).receive_join().receive_message();
///
///         Ok(Watcher)
///     }
///
///     async fn handle_url(&self, _: &(), _: &Client, url: &UrlEvent) -> Result<(), Error> {
///         // Every URL posted in any channel — already filtered, deduplicated, and repaired.
///         Ok(())
///     }
///
///     async fn handle_join(&self, _: &(), _: &Client, join: &JoinEvent) -> Result<(), Error> {
///         // Someone joined `join.channel()`.
///         Ok(())
///     }
///
///     async fn handle_message(&self, _: &(), _: &Client, message: &MessageEvent) -> Result<(), Error> {
///         // Every channel message, whether or not it matches a command.
///         Ok(())
///     }
/// }
/// ```
#[async_trait]
pub trait Plugin<C: Sync = ()>: PluginName + Send + Sync {
    /// This plugin's deserialized `[plugins.<name>]` settings.
    ///
    /// The host hands only the plugin's own settings to [`Plugin::new`]; plugins cannot access
    /// other plugins' configuration.
    type Settings;

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
        _ctx: &C,
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
    /// Plugins handling multiple commands should dispatch on the identity of their declared
    /// command constants (see [`CommandSpec`](super::CommandSpec)'s documentation on matching
    /// by identity), for example:
    ///
    /// ```
    /// # use irc::client::Client;
    /// # use zeta_plugin::{CommandEvent, CommandSpec, Error, Subscriptions, prelude::*};
    /// # const FOO: CommandSpec = CommandSpec::new(".foo", "Handle `.foo`");
    /// # const BAR: CommandSpec = CommandSpec::new(".bar", "Handle `.bar`");
    /// # struct MyPlugin;
    /// # impl PluginName for MyPlugin {
    /// #     const NAME: &'static str = "my_plugin";
    /// # }
    /// # #[async_trait]
    /// # impl Plugin for MyPlugin {
    /// #     type Settings = NoSettings;
    /// #     fn new(_: &(), _: &NoSettings, subscriptions: &mut Subscriptions) -> Result<Self, Error> {
    /// #         subscriptions.command(FOO).command(BAR);
    /// #         Ok(MyPlugin)
    /// #     }
    /// async fn handle_command(
    ///     &self,
    ///     _ctx: &(),
    ///     client: &Client,
    ///     command: &CommandEvent,
    /// ) -> Result<(), Error> {
    ///     match command.spec {
    ///         FOO => { /* handle `.foo` */ }
    ///         BAR => { /* handle `.bar` */ }
    ///         _ => {}
    ///     }
    ///
    ///     Ok(())
    /// }
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if handling the command failed; the error is logged and never
    /// propagated to the other plugins.
    async fn handle_command(
        &self,
        _ctx: &C,
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
    async fn handle_url(&self, _ctx: &C, _client: &Client, _url: &UrlEvent) -> Result<(), Error> {
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
        _ctx: &C,
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
    async fn handle_join(&self, _ctx: &C, _client: &Client, _join: &JoinEvent) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a user leaving a channel.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_part(&self, _ctx: &C, _client: &Client, _part: &PartEvent) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a user quitting the network.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_quit(&self, _ctx: &C, _client: &Client, _quit: &QuitEvent) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a user changing their nickname.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_nick(&self, _ctx: &C, _client: &Client, _nick: &NickEvent) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a user being kicked from a channel.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_kick(&self, _ctx: &C, _client: &Client, _kick: &KickEvent) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a CTCP request or reply.
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_ctcp(&self, _ctx: &C, _client: &Client, _ctcp: &CtcpEvent) -> Result<(), Error> {
        Ok(())
    }

    /// Handles a raw, unmodeled IRC command.
    ///
    /// Only delivered to plugins that registered with [`Subscriptions::receive_raw`].
    ///
    /// # Errors
    ///
    /// Returns an error if handling the event failed; the error is logged and never propagated.
    async fn handle_raw(&self, _ctx: &C, _client: &Client, _raw: &RawEvent) -> Result<(), Error> {
        Ok(())
    }

    /// Called when all plugins are loaded and the client has connected to the network.
    ///
    /// This is useful for setting up plugins with async state.
    ///
    /// # Errors
    ///
    /// Returns an error if the plugin failed to load; the error is logged and the plugin task
    /// exits without processing any events.
    async fn loaded(&mut self, _ctx: &C, _client: &Client) -> Result<(), Error> {
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
    /// use zeta_plugin::{Error, Subscriptions, prelude::*};
    ///
    /// struct FlushPlugin;
    ///
    /// impl PluginName for FlushPlugin {
    ///     const NAME: &'static str = "flush";
    /// }
    ///
    ///#[async_trait]
    /// impl Plugin for FlushPlugin {
    ///     type Settings = NoSettings;
    ///
    ///     fn new(_: &(), _: &NoSettings, _: &mut Subscriptions) -> Result<Self, Error> {
    ///         Ok(FlushPlugin)
    ///     }
    ///
    ///     async fn shutdown(&mut self, _: &(), _client: &Client) -> Result<(), Error> {
    ///         // Flush any pending state to durable storage here.
    ///         Ok(())
    ///     }
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if flushing failed; the error is logged and the shutdown continues.
    async fn shutdown(&mut self, _ctx: &C, _client: &Client) -> Result<(), Error> {
        Ok(())
    }
}
