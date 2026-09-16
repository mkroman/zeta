use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};

use crate::command::{PluginCommand, Prefix};
use crate::{Error, Metadata};

/// The base trait that all plugins must implement.
///
/// Plugins declare the prefix commands they handle through [`Plugin::commands`] and implement
/// [`Plugin::handle_command`] for each matched command. A plugin's configuration is declared
/// through [`Plugin::Settings`] and deserialized from its `[plugins.<name>]` section by the host,
/// which passes it to [`Plugin::new`].
///
/// The default [`Plugin::handle_message`] implementation filters incoming `PRIVMSG` messages
/// against the declared commands and dispatches them — plugins that also need to observe
/// non-command messages (e.g. URLs) may override `handle_message` and call
/// [`Plugin::dispatch_command`] themselves.
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
/// use zeta_plugin::{Error, Prefix, prelude::*};
///
/// struct MyPlugin;
///
/// const HELLO: PluginCommand = PluginCommand::new(Prefix::new(".hello"), "Greet someone");
/// const COMMANDS: &[PluginCommand] = &[HELLO];
///
///#[async_trait]
/// impl Plugin for MyPlugin {
///     type Settings = NoSettings;
///
///     fn new(_: &(), _: &NoSettings) -> Result<MyPlugin, Error> {
///         Ok(MyPlugin)
///     }
///
///     fn metadata() -> Metadata {
///         Metadata {
///             name: "my_plugin".into(),
///             authors: vec!["John Doe <john.doe@example.com>".into()]
///        }
///     }
///
///     fn commands(&self) -> &'static [PluginCommand] {
///         COMMANDS
///     }
///
///     async fn handle_command(
///         &self,
///         _ctx: &(),
///         client: &Client,
///         channel: &str,
///         _command: &Prefix,
///         args: &str,
///     ) -> Result<(), Error> {
///         let nick = args.split_whitespace().next().unwrap_or("world");
///         client.send_privmsg(channel, format!("hello, {nick}!"))?;
///         Ok(())
///     }
/// }
/// ```
#[async_trait]
pub trait Plugin<C: Sync = ()>: Send + Sync {
    /// This plugin's deserialized `[plugins.<name>]` settings.
    ///
    /// The host hands only the plugin's own settings to [`Plugin::new`]; plugins cannot access
    /// other plugins' configuration.
    type Settings;

    /// The constructor for a new plugin.
    ///
    /// `settings` holds the plugin's own configuration section, deserialized from
    /// `[plugins.<name>]` at startup. Returns `Err` if initialization fails (e.g., missing
    /// environment variables, failed HTTP client creation). The registry will log the error and
    /// skip loading the plugin.
    ///
    /// # Errors
    ///
    /// Returns an error if the plugin cannot be initialized.
    fn new(_ctx: &C, _settings: &Self::Settings) -> Result<Self, Error>
    where
        Self: Sized;

    /// Metadata describing the plugin and its authorship.
    fn metadata() -> Metadata
    where
        Self: Sized;

    /// The commands handled by this plugin.
    ///
    /// Every incoming `PRIVMSG` is matched against these prefixes; the first matching command is
    /// dispatched to [`Plugin::handle_command`].
    ///
    /// Each command carries a short description shown by the host's help command. Commands that
    /// accept arguments should associate their [`argh`] argument type with
    /// [`PluginCommand::with_args`], so the host can derive usage and argument information from it.
    ///
    /// Commands may overlap as long as no prefix is a word-prefix of another (e.g. `.y` and `.yt`).
    fn commands(&self) -> &'static [PluginCommand] {
        &[]
    }

    /// The URL hosts whose links this plugin handles itself.
    ///
    /// Plugins that react to URLs posted in a channel — e.g. by looking up details about the
    /// linked resource — declare the exact host names they handle here, so generic URL plugins
    /// (such as the titles plugin) can leave those URLs alone. The hosts are matched against the
    /// URL host after ASCII lowercasing both sides, so list every variant that can appear in a
    /// posted URL (e.g. `imdb.com` as well as `www.imdb.com`).
    ///
    /// The host list is advertised through the plugin catalog; keeping it in sync with the hosts
    /// the plugin actually handles is up to the plugin.
    fn url_hosts(&self) -> &'static [&'static str] {
        &[]
    }

    /// Handles a command invocation.
    ///
    /// Called when a `PRIVMSG` in `channel` matches one of the prefixes returned by
    /// [`Plugin::commands`]. `command` is the matched prefix and `args` is the remainder of the
    /// message with leading whitespace stripped (empty when the command was invoked without
    /// arguments).
    ///
    /// Plugins handling multiple commands should dispatch on the identity of their declared command
    /// constants (see [`Prefix`]'s documentation on matching by identity), for example:
    ///
    /// ```
    /// # use irc::client::Client;
    /// # use irc::proto::{Command, Message};
    /// # use zeta_plugin::{Error, Prefix, prelude::*};
    /// # const FOO: Prefix = Prefix::new(".foo");
    /// # const BAR: Prefix = Prefix::new(".bar");
    /// # const COMMANDS: &[PluginCommand] = &[
    /// #     PluginCommand::new(FOO, "Handle `.foo`"),
    /// #     PluginCommand::new(BAR, "Handle `.bar`"),
    /// # ];
    /// # struct MyPlugin;
    /// # #[async_trait]
    /// # impl Plugin for MyPlugin {
    /// #     type Settings = NoSettings;
    /// #     fn new(_: &(), _: &NoSettings) -> Result<Self, Error> { Ok(MyPlugin) }
    /// #     fn metadata() -> Metadata { unimplemented!() }
    /// #     fn commands(&self) -> &'static [PluginCommand] { COMMANDS }
    /// async fn handle_command(
    ///     &self,
    ///     _ctx: &(),
    ///     client: &Client,
    ///     channel: &str,
    ///     command: &Prefix,
    ///     args: &str,
    /// ) -> Result<(), Error> {
    ///     match *command {
    ///         FOO => { /* handle `.foo` */ }
    ///         BAR => { /* handle `.bar` */ }
    ///         _ => {}
    ///     }
    ///     Ok(())
    /// }
    /// # }
    /// ```
    async fn handle_command(
        &self,
        _ctx: &C,
        _client: &Client,
        _channel: &str,
        _command: &Prefix,
        _args: &str,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Called when all plugins are loaded and the client has connected to the network.
    ///
    /// This is useful for setting up plugins with async state.
    async fn loaded(&mut self, _ctx: &C, _client: &Client) -> Result<(), Error> {
        Ok(())
    }

    /// Dispatches `message` to [`Plugin::handle_command`] if it matches one of
    /// [`Plugin::commands`].
    ///
    /// Plugins overriding [`Plugin::handle_message`] (e.g. to observe all messages) can call this
    /// to retain the standard command dispatching.
    async fn dispatch_command(
        &self,
        ctx: &C,
        client: &Client,
        message: &Message,
    ) -> Result<(), Error> {
        let Command::PRIVMSG(ref channel, ref text) = message.command else {
            return Ok(());
        };

        let Some((command, args)) = self
            .commands()
            .iter()
            .find_map(|command| command.parse(text).map(|args| (command, args)))
        else {
            return Ok(());
        };

        let prefix = command.prefix();

        self.handle_command(ctx, client, channel, &prefix, args)
            .await
    }

    /// Handles IRC protocol messages.
    ///
    /// The default implementation only dispatches commands; override it to observe every message,
    /// calling [`Plugin::dispatch_command`] to keep command handling.
    async fn handle_message(
        &self,
        ctx: &C,
        client: &Client,
        message: &Message,
    ) -> Result<(), Error> {
        self.dispatch_command(ctx, client, message).await
    }
}
