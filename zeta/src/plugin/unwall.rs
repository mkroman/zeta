//! Unwalls paywalled news links through unwall.app and replies with the reader link.
//!
//! Links whose host is covered — the sites unwall.app has tested, plus admin additions — are
//! mirrored through unwall.app and answered with the unwall.app reader link, e.g.
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
//! hostmasks configured in `[irc] admin_hostmasks` (shared with the `.filter` plugin). With no
//! hostmasks configured, nobody is an admin. `.unwall list`, `.unwall stats`, and
//! `.unwall info <host>` are open to everyone.
//!
//! The covered set starts at the tested domains of unwall.app — a snapshot in [`sites`],
//! refreshed from the API once a day and retried with a capped exponential backoff while it
//! is unreachable. Admins add sites per deployment with `.unwall add`, and remove coverage
//! with `.unwall rm`, whose patterns also silence tested domains (a tombstone in the
//! database). URL events arrive already filtered by the host dispatcher, so database filters
//! apply to this plugin without it consulting them. Because the covered set is dynamic, the
//! plugin subscribes to every URL and the `titles` plugin still announces page titles for
//! covered links — deliberately, as a title next to a reader link.

mod client;
mod error;
mod model;
mod repository;
mod service;
mod sites;

// The module types are re-exported as part of the module's API surface, even though the plugin
// itself only handles them by value.
#[allow(unused_imports)]
pub use error::Error;
pub use model::{CachedUrl, HostStatistics, Site, SiteRemoval, Statistics};
pub use service::UnwallService;

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use argh::{ArgsInfo, FromArgs};
use tracing::{Instrument, debug, warn};
use wildmatch::WildMatch;

use crate::{
    duration::TimeInWords,
    http,
    plugin::{
        filter::{compile_hostmasks, hostmask_matches},
        prelude::*,
    },
    url::redact_url,
    utils::{MAX_LISTING_LENGTH, append_entries_within_budget},
};

use client::UnwallClient;
use model::NewFetch;

/// The name this plugin replies under.
const NAME: &str = "UnWall";

/// The `.unwall` command.
const UNWALL: CommandSpec = CommandSpec::with_args::<Opts>(
    ".unwall",
    "Manage the paywalled sites unwalled through unwall.app (add/rm/list/stats/info)",
);

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
    /// How often the tested domains are refreshed from the API.
    #[serde(with = "humantime_serde")]
    pub refresh_interval: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_base: client::DEFAULT_API_BASE.to_string(),
            reader_base: "https://unwall.app/".to_string(),
            submit_timeout: Duration::from_mins(5),
            refresh_interval: Duration::from_hours(24),
        }
    }
}

/// Manage the paywalled sites unwalled through unwall.app.
#[derive(FromArgs, ArgsInfo, Debug)]
struct Opts {
    /// the unwall operation
    #[argh(subcommand)]
    command: Subcommand,
}

/// The `.unwall` subcommands.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand)]
enum Subcommand {
    /// add sites to unwall
    Add(Add),
    /// remove sites matching a pattern
    Rm(Rm),
    /// remove sites matching a pattern (alias of `rm`)
    Del(Del),
    /// remove sites matching a pattern (alias of `rm`)
    Delete(Delete),
    /// summarize the covered sites
    List(List),
    /// show overall statistics about the unwalled links
    Stats(Stats),
    /// show statistics for one host
    Info(Info),
}

/// Add sites to unwall.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "add")]
struct Add {
    /// the hostnames to unwall, e.g. `bloomberg.com` or `*.bloomberg.com`
    #[argh(positional, greedy)]
    hosts: Vec<String>,
}

/// Remove sites matching a pattern.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "rm")]
struct Rm {
    /// the hostname or wildcard to remove, e.g. `www.bloomberg.com` or `*.bloomberg.com`
    #[argh(positional)]
    pattern: String,
}

/// Remove sites matching a pattern.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "del")]
struct Del {
    /// the hostname or wildcard to remove, e.g. `www.bloomberg.com` or `*.bloomberg.com`
    #[argh(positional)]
    pattern: String,
}

/// Remove sites matching a pattern.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "delete")]
struct Delete {
    /// the hostname or wildcard to remove, e.g. `www.bloomberg.com` or `*.bloomberg.com`
    #[argh(positional)]
    pattern: String,
}

/// Summarize the covered sites.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "list")]
struct List {}

/// Show overall statistics about the unwalled links.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "stats")]
struct Stats {}

/// Show statistics for one host.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "info")]
struct Info {
    /// the host to show statistics for
    #[argh(positional)]
    host: String,
}

/// Unwall plugin.
///
/// Unwalls covered paywalled news links posted in a channel and manages the covered sites.
pub struct Unwall {
    /// The unwall service.
    service: Arc<UnwallService>,
    /// The compiled admin hostmasks; senders whose `nick!user@host` matches one of them are
    /// authorized to manage unwall sites.
    admins: Vec<WildMatch>,
    /// How often the tested domains are refreshed from the API.
    refresh_interval: Duration,
}

impl Unwall {
    /// Whether `sender` is authorized to manage unwall sites.
    fn is_admin(&self, sender: Sender<'_>) -> bool {
        hostmask_matches(&self.admins, sender)
    }

    /// Replies with the unauthorized message and yields `false` when `sender` is not an admin.
    fn authorized(
        &self,
        client: &Client,
        channel: &str,
        sender: Sender<'_>,
    ) -> Result<bool, ZetaError> {
        if self.is_admin(sender) {
            return Ok(true);
        }

        client.send_privmsg(channel, reply(NAME, UNAUTHORIZED))?;

        Ok(false)
    }

    /// Adds the sites of the `add` subcommand to the covered set.
    async fn add(
        &self,
        client: &Client,
        channel: &str,
        nickname: &str,
        opts: Add,
    ) -> Result<(), ZetaError> {
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
                            format!("{RESET}{input}{COLOR} does not look like a hostname."),
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
            reply(NAME, format!("Now unwalling {}{COLOR}.", join_and(&hosts))),
        )?;

        Ok(())
    }

    /// Removes the covered sites matching the `rm`/`del`/`delete` subcommand pattern.
    async fn remove(
        &self,
        client: &Client,
        channel: &str,
        nickname: &str,
        pattern: &str,
    ) -> Result<(), ZetaError> {
        let Some(pattern) = sites::normalize_site(pattern) else {
            client.send_privmsg(
                channel,
                reply(
                    NAME,
                    format!(
                        "{RESET}{pattern}{COLOR} does not look like a hostname or wildcard."
                    ),
                ),
            )?;

            return Ok(());
        };

        let removed = match self.service.remove_matching(&pattern, nickname).await {
            Ok(removed) => removed,
            Err(error) => {
                client.send_privmsg(
                    channel,
                    reply(
                        NAME,
                        format!("could not remove the sites:{RESET} {error}"),
                    ),
                )?;

                return Ok(());
            }
        };

        let response = if removed.is_empty() {
            format!("No unwalled sites match{RESET} {pattern}{COLOR}.")
        } else {
            format!(
                "Removed {}{COLOR} from unwalled sites.",
                join_and(&removed)
            )
        };

        client.send_privmsg(channel, reply(NAME, response))?;

        Ok(())
    }

    /// Summarizes the covered sites in a single line.
    fn list(&self, client: &Client, channel: &str) -> Result<(), ZetaError> {
        let (tested, removed, _) = self.service.coverage();
        let sites = self.service.added_sites();

        let response = if sites.is_empty() {
            reply(
                NAME,
                format!(
                    "{RESET}{tested}{COLOR} tested sites by default (unwall.app),\
                     {RESET} {removed}{COLOR} removed, none added{COLOR}."
                ),
            )
        } else {
            let mut message = reply(
                NAME,
                format!(
                    "{RESET}{tested}{COLOR} tested sites by default (unwall.app),\
                     {RESET} {removed}{COLOR} removed,{RESET} {}{COLOR} added:{RESET} ",
                    sites.len()
                ),
            );

            let reserved = " (999 more)".len();
            let appended = append_entries_within_budget(
                &mut message,
                sites.iter().map(|site| site.host.clone()),
                &format!("{COLOR}, {RESET}"),
                MAX_LISTING_LENGTH,
                reserved,
            );

            if appended < sites.len() {
                let _ = write!(message, "{COLOR} ({} more)", sites.len() - appended);
            }

            message
        };

        client.send_privmsg(channel, response)?;

        Ok(())
    }

    /// Replies with the overall statistics of the unwalled links.
    async fn stats(&self, client: &Client, channel: &str) -> Result<(), ZetaError> {
        let response = match self.service.statistics().await {
            Ok(stats) => reply(
                NAME,
                format!(
                    "{RESET}{}{COLOR} links unwalled across{RESET} {}{COLOR} sites \
                     by{RESET} {}{COLOR} users ({RESET}{}{COLOR} today).",
                    stats.urls, stats.hosts, stats.users, stats.urls_today
                ),
            ),
            Err(error) => reply(NAME, format!("could not load the statistics:{RESET} {error}")),
        };

        client.send_privmsg(channel, response)?;

        Ok(())
    }

    /// Replies with the statistics of one host.
    async fn info(&self, client: &Client, channel: &str, host: &str) -> Result<(), ZetaError> {
        let host = host.trim().to_lowercase();

        let response = match self.service.host_statistics(&host).await {
            Ok(Some(stats)) => reply(
                NAME,
                format!(
                    "{RESET}{}{COLOR} links unwalled for{RESET} {host}{COLOR} \
                     by{RESET} {}{COLOR} users ({RESET}{}{COLOR} today, most recent\
                     {RESET} {}{COLOR}).",
                    stats.urls,
                    stats.users,
                    stats.urls_today,
                    stats.latest.map(|latest| latest.time_ago()).unwrap_or_default(),
                ),
            ),
            Ok(None) => reply(NAME, format!("No unwalled links for{RESET} {host}{COLOR}.")),
            Err(error) => reply(NAME, format!("could not load the statistics:{RESET} {error}")),
        };

        client.send_privmsg(channel, response)?;

        Ok(())
    }

    /// Submits a covered article to unwall.app in a task of its own, replying with the reader
    /// link once the mirror is warm — or from the cache, when the article is known.
    fn submit(&self, client: &Client, event: &UrlEvent) {
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

        let fetch = NewFetch {
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
                        // The link rides on the plain `>` notice prefix, which colors the
                        // whole line cyan.
                        if let Err(error) = irc.send_privmsg(&channel, notice(&cached.unwall_url))
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
                            notice(format!("could not unwall the article:{RESET} {error}")),
                        ) {
                            warn!(%error, "could not send the unwall failure");
                        }
                    }
                }
            }
            .instrument(span),
        );
    }

    /// Runs the background refresh of the tested domains: on success, at
    /// `refresh_interval`; on failure, with a capped exponential backoff while the API is
    /// unreachable, keeping the previous set served.
    fn spawn_refresh(&self) {
        let service = Arc::clone(&self.service);
        let interval = self.refresh_interval;

        tokio::spawn(
            async move {
                let mut failures = 0u32;

                loop {
                    match service.refresh_tested_domains().await {
                        Ok(_) => failures = 0,
                        Err(error) => {
                            failures += 1;
                            warn!(failures, %error, "could not refresh the tested domains");
                        }
                    }

                    let delay = if failures == 0 {
                        interval
                    } else {
                        sites::refresh_backoff(failures)
                    };

                    tokio::time::sleep(delay).await;
                }
            }
            .instrument(tracing::info_span!("unwall_tested_domains_refresh")),
        );
    }
}

/// Joins `items` with cyan commas and a final cyan `and`, each item emphasized in the default
/// color: `\x0fa\x0310, \x0fb\x0310 and\x0f c`.
fn join_and(items: &[String]) -> String {
    let mut joined = String::new();

    for (index, item) in items.iter().enumerate() {
        if index == 0 {
            let _ = write!(joined, "{RESET}{item}");
        } else if index + 1 == items.len() {
            let _ = write!(joined, "{COLOR} and{RESET} {item}");
        } else {
            let _ = write!(joined, "{COLOR},{RESET} {item}");
        }
    }

    joined
}

#[async_trait]
impl Plugin<Context> for Unwall {
    type Settings = Settings;

    fn new(
        ctx: &Context,
        settings: &Settings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, ZetaError> {
        subscriptions.command(UNWALL).urls(UrlScope::Any);

        let api_base = client::base_url(&settings.api_base).map_err(plugin_err)?;
        let reader_base = client::base_url(&settings.reader_base).map_err(plugin_err)?;
        let client = UnwallClient::new(
            http::build_client(&ctx.config.http),
            settings.submit_timeout,
            api_base,
        );

        let admins = compile_hostmasks(&ctx.config.irc.admin_hostmasks);

        if admins.is_empty() {
            tracing::warn!(
                "no admin hostmasks configured; nobody is authorized to manage unwall sites \
                 (set [irc] admin_hostmasks)"
            );
        } else {
            debug!(count = admins.len(), "loaded admin hostmasks");
        }

        Ok(Self {
            service: Arc::new(UnwallService::new(ctx.db.clone(), client, reader_base)),
            admins,
            refresh_interval: settings.refresh_interval,
        })
    }

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        self.service.load().await.map_err(plugin_err)?;
        self.spawn_refresh();

        Ok(())
    }

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

        let Some(opts) = parse_words_or_usage::<Opts>(client, command, |line| reply(NAME, line))?
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

                self.remove(client, channel, sender.nick, &pattern).await
            }
            Subcommand::List(List {}) => self.list(client, channel),
            Subcommand::Stats(Stats {}) => self.stats(client, channel).await,
            Subcommand::Info(Info { host }) => self.info(client, channel, &host).await,
        }
    }

    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &Client,
        event: &UrlEvent,
    ) -> Result<(), ZetaError> {
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
            assert_eq!(settings.refresh_interval, Duration::from_hours(24));
        }
        deserialize: {
            "api_base": "https://unwall.example/api",
            "reader_base": "https://unwall.example/",
            "submit_timeout": "90s",
            "refresh_interval": "1h",
        } assert: {
            assert_eq!(settings.api_base, "https://unwall.example/api");
            assert_eq!(settings.reader_base, "https://unwall.example/");
            assert_eq!(settings.submit_timeout, Duration::from_secs(90));
            assert_eq!(settings.refresh_interval, Duration::from_mins(60));
        }
    }

    #[test]
    fn joins_with_a_final_and() {
        assert_eq!(join_and(&[]), "");
        assert_eq!(join_and(&["a".into()]), format!("{RESET}a"));
        assert_eq!(
            join_and(&["a".into(), "b".into()]),
            format!("{RESET}a{COLOR} and{RESET} b")
        );
        assert_eq!(
            join_and(&["a".into(), "b".into(), "c".into()]),
            format!("{RESET}a{COLOR},{RESET} b{COLOR} and{RESET} c")
        );
    }
}
