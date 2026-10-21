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
//! hostmasks configured in `[irc] admin_hostmasks` (the same crate-private `hostmask` module
//! the `.filter` plugin uses). With no hostmasks configured, nobody is an admin.
//! `.unwall list`, `.unwall stats`, and `.unwall info <host>` are open to everyone.
//!
//! Coverage is exactly what the admins allowed with `.unwall add`, whatever unwall.app itself
//! has tested: nothing is covered until it is added, and `.unwall rm` takes a site out of the
//! set again. The set lives in the `unwall_sites` table, so it survives restarts. URL events
//! arrive already filtered by the host dispatcher, so database filters apply to this plugin
//! without it consulting them. Because the covered set is dynamic, the plugin subscribes to
//! every URL and the `titles` plugin still announces page titles for covered links —
//! deliberately, as a title next to a reader link.

mod client;
mod command;
mod error;
mod model;
mod reply;
mod repository;
mod service;
mod sites;

// The module types are re-exported as part of the module's API surface, even though the plugin
// itself only handles them by value.
#[allow(unused_imports)]
pub use error::Error;
pub use model::{CachedUrl, HostStatistics, Site, Statistics};
pub use service::UnwallService;

use std::sync::Arc;
use std::time::Duration;

use tracing::{Instrument, debug, warn};

use self::command::{Add, Del, Delete, Info, List, Options, Rm, Stats, Subcommand, UNWALL};
use self::reply::{info_reply, join_and, list_reply, stats_reply};
use crate::hostmask::AdminHostmasks;
use crate::prelude::*;
use zeta_http as http;
use zeta_url::redact_url;

use client::UnwallClient;
use model::NewFetchRecord;

/// The name this plugin replies under.
const NAME: &str = "UnWall";

/// The reply sent to senders that are not authorized to manage unwall sites.
const UNAUTHORIZED: &str = "You are not authorized to manage unwall sites.";

/// The identifier of the network recorded with every fetch.
// TODO: support multiple networks dynamically (as `.ofn` does).
const NETWORK_ID: &str = "irc.rwx.im:6697";

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
    admins: AdminHostmasks,
}

impl Unwall {
    /// Whether `sender` is authorized to manage unwall sites.
    fn is_admin(&self, sender: Sender<'_>) -> bool {
        self.admins.matches(sender)
    }

    /// Replies with the unauthorized message and yields `false` when `sender` is not an admin.
    fn authorized(
        &self,
        client: &IrcClient,
        channel: &str,
        sender: Sender<'_>,
    ) -> Result<bool, PluginError> {
        if self.is_admin(sender) {
            return Ok(true);
        }

        client.send_privmsg(channel, reply(NAME, UNAUTHORIZED))?;

        Ok(false)
    }

    /// Adds the sites of the `add` subcommand to the covered set.
    async fn add(
        &self,
        client: &IrcClient,
        channel: &str,
        nickname: &str,
        opts: Add,
    ) -> Result<(), PluginError> {
        let mut hosts = Vec::new();

        for input in &opts.hosts {
            match sites::normalize_site(input) {
                Some(host) if !hosts.contains(&host) => hosts.push(host),
                Some(_) => {}
                None => {
                    client.send_privmsg(
                        channel,
                        reply(
                            NAME,
                            format!("{RESET}{input}{REPLY_COLOR} does not look like a hostname."),
                        ),
                    )?;

                    return Ok(());
                }
            }
        }

        if hosts.is_empty() {
            client.send_privmsg(
                channel,
                reply(NAME, "specify at least one hostname to unwall."),
            )?;

            return Ok(());
        }

        if let Err(error) = self.service.add_sites(&hosts, nickname).await {
            client.send_privmsg(
                channel,
                reply(NAME, format!("could not add the sites:{RESET} {error}")),
            )?;

            return Ok(());
        }

        debug!(count = hosts.len(), "added unwall sites");

        client.send_privmsg(
            channel,
            reply(
                NAME,
                format!("Now unwalling {}{REPLY_COLOR}.", join_and(&hosts)),
            ),
        )?;

        Ok(())
    }

    /// Removes the covered sites matching the `rm`/`del`/`delete` subcommand pattern.
    async fn remove(
        &self,
        client: &IrcClient,
        channel: &str,
        pattern: &str,
    ) -> Result<(), PluginError> {
        let Some(pattern) = sites::normalize_site(pattern) else {
            client.send_privmsg(
                channel,
                reply(
                    NAME,
                    format!(
                        "{RESET}{pattern}{REPLY_COLOR} does not look like a hostname or wildcard."
                    ),
                ),
            )?;

            return Ok(());
        };

        let removed = match self.service.remove_matching(&pattern).await {
            Ok(removed) => removed,
            Err(error) => {
                client.send_privmsg(
                    channel,
                    reply(NAME, format!("could not remove the sites:{RESET} {error}")),
                )?;

                return Ok(());
            }
        };

        let response = if removed.is_empty() {
            format!("No unwalled sites match{RESET} {pattern}{REPLY_COLOR}.")
        } else {
            format!(
                "Removed {}{REPLY_COLOR} from unwalled sites.",
                join_and(&removed)
            )
        };

        client.send_privmsg(channel, reply(NAME, response))?;

        Ok(())
    }

    /// Summarizes the covered sites in a single line.
    fn list(&self, client: &IrcClient, channel: &str) -> Result<(), PluginError> {
        client.send_privmsg(channel, list_reply(&self.service.sites()))?;

        Ok(())
    }

    /// Replies with the overall statistics of the unwalled links.
    async fn stats(&self, client: &IrcClient, channel: &str) -> Result<(), PluginError> {
        let response = match self.service.statistics().await {
            Ok(stats) => stats_reply(&stats),
            Err(error) => reply(
                NAME,
                format!("could not load the statistics:{RESET} {error}"),
            ),
        };

        client.send_privmsg(channel, response)?;

        Ok(())
    }

    /// Replies with the statistics of one host.
    async fn info(&self, client: &IrcClient, channel: &str, host: &str) -> Result<(), PluginError> {
        let host = host.trim().to_lowercase();

        let response = match self.service.host_statistics(&host).await {
            Ok(stats) => info_reply(&host, stats.as_ref()),
            Err(error) => reply(
                NAME,
                format!("could not load the statistics:{RESET} {error}"),
            ),
        };

        client.send_privmsg(channel, response)?;

        Ok(())
    }

    /// Submits a covered article to unwall.app in a task of its own, replying with the reader
    /// link once the mirror is warm — or from the cache, when the article is known.
    fn submit(&self, client: &IrcClient, event: &UrlEvent) {
        let url = event.url();

        if !matches!(url.scheme(), "http" | "https") {
            return;
        }

        let Some(host) = url.host_str() else {
            return;
        };

        if !self.service.covers(host) {
            return;
        }

        let Some(sender) = event.sender() else {
            return;
        };

        // The fragment is not part of what unwall mirrors, so it is not part of the cache key
        // either.
        let mut article = url.clone();
        article.set_fragment(None);

        let fetch = NewFetchRecord {
            url: article.as_str().to_owned(),
            host: host.to_owned(),
            nickname: sender.nick.to_owned(),
            username: sender.username.to_owned(),
            hostname: sender.hostname.to_owned(),
            channel: event.channel().to_owned(),
            network_id: NETWORK_ID.to_owned(),
        };

        let service = Arc::clone(&self.service);
        let irc = client.sender();
        let channel = event.channel().to_owned();

        // The span carries the redacted URL: without it, this task's logs would only ever
        // reach stdout.
        let span = tracing::info_span!("unwall_article", url.full = %redact_url(&article));

        tokio::spawn(
            async move {
                match service.resolve(&article, fetch).await {
                    Ok(cached) => {
                        // The link rides on the plain `>` reply marker, which colors the
                        // whole line cyan.
                        if let Err(error) =
                            irc.send_privmsg(&channel, plain_reply(&cached.unwall_url))
                        {
                            warn!(%error, "could not send the unwall link");
                        }
                    }
                    Err(error) => {
                        warn!(
                            url.full = %redact_url(&article),
                            %error,
                            "could not unwall the article"
                        );

                        if let Err(error) = irc.send_privmsg(
                            &channel,
                            plain_reply(format!("could not unwall the article:{RESET} {error}")),
                        ) {
                            warn!(%error, "could not send the unwall failure");
                        }
                    }
                }
            }
            .instrument(span),
        );
    }
}

#[async_trait]
impl Plugin<Context> for Unwall {
    type Settings = Settings;

    fn new(
        ctx: &Context,
        settings: &Settings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, PluginError> {
        subscriptions
            .subscribe_command(UNWALL)
            .subscribe_urls(UrlScope::Any);

        let api_base = client::base_url(&settings.api_base).map_err(plugin_err)?;
        let reader_base = client::base_url(&settings.reader_base).map_err(plugin_err)?;
        let client = UnwallClient::new(
            http::build_client(&ctx.config.http),
            settings.submit_timeout,
            api_base,
        );

        let admins = AdminHostmasks::compile(&ctx.config.admin_hostmasks, "manage unwall sites");

        Ok(Self {
            service: Arc::new(UnwallService::new(ctx.db.clone(), client, reader_base)),
            admins,
        })
    }

    async fn start(&mut self, _ctx: &Context, _client: &IrcClient) -> Result<(), PluginError> {
        self.service.load().await.map_err(plugin_err)?;

        Ok(())
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &IrcClient,
        command: &CommandEvent,
    ) -> Result<(), PluginError> {
        let channel = command.channel();
        let Some(sender) = command.sender() else {
            return Ok(());
        };

        let Some(opts) =
            parse_words_or_usage::<Options>(client, command, |line| reply(NAME, line))?
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
            Subcommand::Rm(Rm { pattern })
            | Subcommand::Del(Del { pattern })
            | Subcommand::Delete(Delete { pattern }) => {
                if !self.authorized(client, channel, sender)? {
                    return Ok(());
                }

                self.remove(client, channel, &pattern).await
            }
            Subcommand::List(List {}) => self.list(client, channel),
            Subcommand::Stats(Stats {}) => self.stats(client, channel).await,
            Subcommand::Info(Info { host }) => self.info(client, channel, &host).await,
        }
    }

    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &IrcClient,
        event: &UrlEvent,
    ) -> Result<(), PluginError> {
        self.submit(client, event);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert_eq!(settings.api_base, "https://api.unwall.app/");
            assert_eq!(settings.reader_base, "https://unwall.app/");
            assert_eq!(settings.submit_timeout, Duration::from_mins(5));
        }
        deserialize: {
            "api_base": "https://unwall.example/api",
            "reader_base": "https://unwall.example/",
            "submit_timeout": "90s",
        } assert: {
            assert_eq!(settings.api_base, "https://unwall.example/api");
            assert_eq!(settings.reader_base, "https://unwall.example/");
            assert_eq!(settings.submit_timeout, Duration::from_secs(90));
        }
    }
}
