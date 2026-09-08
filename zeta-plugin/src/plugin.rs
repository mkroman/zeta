use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};

use crate::command::Prefix;
use crate::{Error, Metadata};

/// The base trait that all plugins must implement.
///
/// Plugins declare the prefix commands they handle through [`Plugin::commands`] and implement
/// [`Plugin::handle_command`] for each matched command.
///
/// The default [`Plugin::handle_message`] implementation filters incoming `PRIVMSG` messages
/// against the declared commands and dispatches them — plugins that also need to observe
/// non-command messages (e.g. URLs) may override `handle_message` and call
/// [`Plugin::dispatch_command`] themselves.
///
///# Examples
///
/// ```
/// use irc::client::Client;
/// use zeta_plugin::{Error, Prefix, prelude::*};
///
/// struct MyPlugin;
///
/// const HELLO: Prefix = Prefix::new(".hello");
///
///#[async_trait]
/// impl Plugin for MyPlugin {
///     fn new(_: &()) -> Result<MyPlugin, Error> {
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
///     fn commands(&self) -> &'static [Prefix] {
///         &[HELLO]
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
    /// The constructor for a new plugin.
    ///
    /// Returns `Err` if initialization fails (e.g., missing environment variables, failed HTTP
    /// client creation). The registry will log the error and skip loading the plugin.
    ///
    /// # Errors
    ///
    /// Returns an error if the plugin cannot be initialized.
    fn new(_ctx: &C) -> Result<Self, Error>
    where
        Self: Sized;

    /// Metadata describing the plugin and its authorship.
    fn metadata() -> Metadata
    where
        Self: Sized;

    /// The prefix commands handled by this plugin.
    ///
    /// Every incoming `PRIVMSG` is matched against these prefixes; the first matching command is
    /// dispatched to [`Plugin::handle_command`].
    ///
    /// Commands may overlap as long as no prefix is a word-prefix of another (e.g. `.y` and `.yt`).
    fn commands(&self) -> &'static [Prefix] {
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
    /// # const COMMANDS: &[Prefix] = &[FOO, BAR];
    /// # struct MyPlugin;
    /// # #[async_trait]
    /// # impl Plugin for MyPlugin {
    /// #     fn new(_: &()) -> Result<Self, Error> { Ok(MyPlugin) }
    /// #     fn metadata() -> Metadata { unimplemented!() }
    /// #     fn commands(&self) -> &'static [Prefix] { COMMANDS }
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

        self.handle_command(ctx, client, channel, command, args)
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
