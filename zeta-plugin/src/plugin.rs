use async_trait::async_trait;
use irc::client::Client;
use irc::proto::Message;

use crate::{BoxError, Error, Metadata};

/// The base trait that all plugins must implement.
///
///# Examples
///
/// ```
/// use irc::client::Client;
/// use irc::proto::{Command, Message};
/// use zeta_plugin::{Error, prelude::*};
///
/// struct MyPlugin;
///
///#[async_trait]
/// impl Plugin for MyPlugin {
///     fn new(_: &()) -> Result<MyPlugin, BoxError> {
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
///     async fn handle_message(
///         &self,
///         _ctx: &(),
///         client: &Client,
///         message: &Message,
///     ) -> Result<(), Error> {
///         if let Command::PRIVMSG(ref target, _) = message.command {
///             let nick = message.source_nickname().unwrap_or("unknown");
///             client.send_privmsg(target, format!("hello, {nick}!"))?;
///         }
///         Ok(())
///     }
/// }
/// ```
#[async_trait]
pub trait Plugin<C = ()>: Send + Sync {
    /// The constructor for a new plugin.
    ///
    /// Returns `Err` if initialization fails (e.g., missing environment
    /// variables, failed HTTP client creation). The registry will log
    /// the error and skip loading the plugin.
    fn new(_ctx: &C) -> Result<Self, BoxError>
    where
        Self: Sized;

    /// Metadata describing the plugin and its authorship.
    fn metadata() -> Metadata
    where
        Self: Sized;

    /// Handles IRC protocol messages.
    async fn handle_message(
        &self,
        _ctx: &C,
        _client: &Client,
        _message: &Message,
    ) -> Result<(), Error> {
        Ok(())
    }
}
