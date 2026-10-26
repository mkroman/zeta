//! Object-safe view of [`Plugin`], with the settings type erased.

use std::future::Future;
use std::pin::Pin;

use irc::client::Client;

use crate::context::Context;
use crate::error::Error;
use crate::event::Event;
use crate::plugin::Plugin;

/// Object-safe view of [`Plugin`], implemented automatically for every `Plugin<Context>`.
///
/// [`Plugin`] declares an associated [`Settings`](Plugin::Settings) type and therefore cannot be
/// used directly as a trait object. The registry stores plugins with different settings types
/// behind this trait, which erases the settings type and exposes only the runtime methods the
/// host needs. Plugin authors never implement this trait themselves.
///
/// The methods return the plugin's already-boxed futures directly instead of being declared
/// `async`; forwarding an `async fn` into another `async fn` would allocate a second boxed future
/// that only awaits the first.
pub trait ErasedPlugin: Send + Sync {
    /// Routes an event to the handler registered for its kind.
    ///
    /// Events are only routed to handlers the plugin registered the matching kind for, so an
    /// unknown variant is unreachable in practice; it is logged when a future event kind is
    /// added without a routing arm.
    fn handle_event<'a>(
        &'a self,
        ctx: &'a Context,
        client: &'a Client,
        event: &'a Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;

    /// Called when all plugins are loaded and the client has connected to the network.
    fn loaded<'a>(
        &'a mut self,
        ctx: &'a Context,
        client: &'a Client,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;

    /// Called once while the bot is shutting down, after the plugin's event queue has been
    /// drained.
    fn shutdown<'a>(
        &'a mut self,
        ctx: &'a Context,
        client: &'a Client,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;
}

impl<P: Plugin<Context>> ErasedPlugin for P {
    fn handle_event<'a>(
        &'a self,
        ctx: &'a Context,
        client: &'a Client,
        event: &'a Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            match event {
                Event::Command(command) => Plugin::handle_command(self, ctx, client, command).await,
                Event::Url(url) => Plugin::handle_url(self, ctx, client, url).await,
                Event::Message(message) => Plugin::handle_message(self, ctx, client, message).await,
                Event::Join(join) => Plugin::handle_join(self, ctx, client, join).await,
                Event::Part(part) => Plugin::handle_part(self, ctx, client, part).await,
                Event::Quit(quit) => Plugin::handle_quit(self, ctx, client, quit).await,
                Event::Nick(nick) => Plugin::handle_nick(self, ctx, client, nick).await,
                Event::Kick(kick) => Plugin::handle_kick(self, ctx, client, kick).await,
                Event::Ctcp(ctcp) => Plugin::handle_ctcp(self, ctx, client, ctcp).await,
                Event::Raw(raw) => Plugin::handle_raw(self, ctx, client, raw).await,
            }
        })
    }

    fn loaded<'a>(
        &'a mut self,
        ctx: &'a Context,
        client: &'a Client,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Plugin::loaded(self, ctx, client)
    }

    fn shutdown<'a>(
        &'a mut self,
        ctx: &'a Context,
        client: &'a Client,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Plugin::shutdown(self, ctx, client)
    }
}
