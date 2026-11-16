//! Unwalls paywalled news links through unwall.app and replies with the reader link.
//!
//! Links whose host is covered — the sites an admin added with `.unwall add` — are mirrored
//! through unwall.app and answered with the unwall.app reader link, e.g.
//! `https://unwall.app/www.bloomberg.com/news/articles/...` for a Bloomberg article. The
//! reader links are deterministic: the unwall reader derives the article from its own URL
//! path, so the plugin only has to submit the article once to warm the mirror.
//!
//! The first post of an article is submitted to the unwall API — which fetches and mirrors it
//! server-side — and the link is only sent once that submission succeeds; every later post of
//! the same URL is answered from the postgres cache without touching the API. Every initial
//! submission is attributed to the user who posted the link (nickname, username, hostname,
//! channel, and network) in the `unwall_fetches` table, which also feeds the `.unwall stats`
//! and `.unwall info <host>` numbers.
//!
//! Coverage is managed with the `.unwall add` and `.unwall rm`/`del`/`delete` commands,
//! restricted to admins: a sender is an admin when their `nick!user@host` matches one of the
//! hostmasks configured in `[irc] admin_hostmasks` (the same [`Hostmasks`] matcher the
//! `.filter` plugin uses). With no hostmasks configured, nobody is an admin.
//! `.unwall list`, `.unwall stats`, and `.unwall info <host>` are open to everyone.
//!
//! Coverage is exactly what the admins allowed with `.unwall add`, whatever unwall.app itself
//! has tested: nothing is covered until it is added, and `.unwall rm` takes a site out of the
//! set again. The set lives in the `unwall_sites` table, so it survives restarts. URL events
//! arrive already filtered by the host dispatcher, so database filters apply to this plugin
//! without it consulting them. Because the covered set is dynamic, the plugin subscribes to
//! every URL and the `titles` plugin still announces page titles for covered links —
//! deliberately, as a title next to a reader link.

mod args;
mod client;
mod commands;
mod error;
mod media;
mod model;
mod repository;
mod service;
mod sites;

#[cfg(test)]
mod tests;

pub use error::Error;
pub use model::{CachedUrl, HostStatistics, Site, Statistics};
pub use service::UnwallService;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use zeta_plugin::irc::{labeled_reply, parse_or_usage};
use zeta_plugin::{
    ArgsMode, Client, CommandEvent, Context, Hostmasks, Subscriptions, UrlEvent, ZetaError, plugin,
    plugin_err,
};

use self::args::{Info, Opts, Remove, Subcommand, UNWALL};
use client::UnwallClient;
use media::SUBMIT_CONCURRENCY;

/// The name this plugin replies under.
const NAME: &str = "UnWall";

/// Settings for the unwall plugin, from its `[plugins.unwall]` configuration section.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Settings {
    /// The base URL of the unwall API.
    pub api_base: String,
    /// The base URL of the unwall reader, linked in the replies.
    pub reader_base: String,
    /// How long an article submission may take.
    ///
    /// Only the first submission of an article hits the API — the mirror is built
    /// server-side, which can take tens of seconds — while every later post is answered from
    /// the cache without a request.
    #[serde(with = "humantime_serde")]
    pub submit_timeout: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_base: client::DEFAULT_API_BASE.to_string(),
            reader_base: "https://unwall.app/".to_string(),
            submit_timeout: Duration::from_mins(5),
        }
    }
}

/// Unwall plugin.
///
/// Unwalls covered paywalled news links posted in a channel and manages the covered sites.
pub struct Unwall {
    /// The unwall service.
    service: Arc<UnwallService>,
    /// The compiled admin hostmasks; senders whose `nick!user@host` matches one of them are
    /// authorized to manage unwall sites.
    admins: Hostmasks,
    /// Bounds the in-flight article submissions per message burst.
    submits: Arc<tokio::sync::Semaphore>,
}

// Manual implementation: the service is summarized by its own redacted view, and the submission
// permits show how much of the burst budget is available.
impl fmt::Debug for Unwall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Unwall")
            .field("service", &self.service)
            .field("admins", &self.admins)
            .field("submits_available", &self.submits.available_permits())
            .finish_non_exhaustive()
    }
}

#[plugin(name = "unwall", settings = Settings, urls(any))]
impl Unwall {
    fn new(
        ctx: &Context,
        settings: &Settings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, ZetaError> {
        subscriptions.command(UNWALL);

        let api_base = client::base_url(&settings.api_base).map_err(plugin_err)?;
        let reader_base = client::base_url(&settings.reader_base).map_err(plugin_err)?;
        let client = UnwallClient::new(ctx.http().clone(), settings.submit_timeout, api_base);

        let admins = ctx.config().admins().clone();

        Ok(Self {
            service: Arc::new(UnwallService::new(ctx.db().clone(), client, reader_base)),
            admins,
            submits: Arc::new(tokio::sync::Semaphore::new(SUBMIT_CONCURRENCY)),
        })
    }

    #[handler(loaded)]
    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        // A transient database failure must not leave the covered set empty for the run, which
        // would silently stop unwalling links until the next restart.
        zeta_plugin::database::load_with_retry(|| self.service.load())
            .await
            .map_err(plugin_err)?;

        Ok(())
    }

    #[handler(command)]
    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let Some(sender) = command.sender() else {
            return Ok(());
        };

        let Some(opts) = parse_or_usage::<Opts>(client, command, ArgsMode::Words, |line| {
            labeled_reply(NAME, line)
        })?
        else {
            return Ok(());
        };

        match opts.command {
            Subcommand::Add(add) => {
                if !self.authorized(client, channel, sender)? {
                    return Ok(());
                }

                self.add(client, channel, sender.nick, add).await
            }
            Subcommand::Remove(Remove { pattern }) => {
                if !self.authorized(client, channel, sender)? {
                    return Ok(());
                }

                self.remove(client, channel, &pattern).await
            }
            Subcommand::List => self.list(client, channel),
            Subcommand::Stats => self.stats(client, channel).await,
            Subcommand::Info(Info { host }) => self.info(client, channel, &host).await,
        }
    }

    #[handler(url)]
    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &Client,
        event: &UrlEvent,
    ) -> Result<(), ZetaError> {
        self.submit(client, event).await;

        Ok(())
    }
}
