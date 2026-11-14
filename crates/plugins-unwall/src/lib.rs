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

mod client;
mod error;
mod model;
mod repository;
mod service;
mod sites;

pub use error::Error;
pub use model::{CachedUrl, HostStatistics, Site, Statistics};
pub use service::UnwallService;

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tracing::{debug, warn};

use zeta_plugin::database::Attribution;
use zeta_plugin::irc::{
    COLOR, RESET, labeled_reply, parse_or_usage, plain_reply, send_reply, send_reply_via,
};
use zeta_plugin::{
    ArgsMode, Client, CommandEvent, CommandSpec, Context, Hostmasks, Sender, Subscriptions,
    UrlEvent, ZetaError, plugin, plugin_err,
};
use zeta_utils::duration::TimeInWordsExt;
use zeta_utils::irc::{self, MAX_LISTING_LENGTH};

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

/// The maximum number of article submissions that run at once.
///
/// A first mirror runs server-side for up to `submit_timeout`, so one message with many covered
/// links must not fan out into an unbounded number of requests against the rate-limited API.
const SUBMIT_CONCURRENCY: usize = 4;

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

/// Manage the paywalled sites unwalled through unwall.app.
#[derive(Parser, Debug)]
struct Opts {
    /// the unwall operation
    #[command(subcommand)]
    command: Subcommand,
}

/// The `.unwall` subcommands.
#[derive(clap::Subcommand, Debug)]
enum Subcommand {
    /// add sites to unwall
    Add(Add),
    /// remove sites matching a pattern
    #[command(name = "rm", visible_alias = "del", visible_alias = "delete")]
    Remove(Remove),
    /// summarize the covered sites
    List,
    /// show overall statistics about the unwalled links
    Stats,
    /// show statistics for one host
    Info(Info),
}

/// Add sites to unwall.
#[derive(clap::Args, Debug)]
struct Add {
    /// the hostnames to unwall, e.g. `bloomberg.com` or `*.bloomberg.com`
    #[arg(num_args = 1.., trailing_var_arg = true)]
    hosts: Vec<String>,
}

/// Remove sites matching a pattern.
#[derive(clap::Args, Debug)]
struct Remove {
    /// the hostname or wildcard to remove, e.g. `www.bloomberg.com` or `*.bloomberg.com`
    pattern: String,
}

/// Show statistics for one host.
#[derive(clap::Args, Debug)]
struct Info {
    /// the host to show statistics for
    host: String,
}

/// Formats a failed article resolution.
///
/// A rejected article and an unusable URL reuse the error's own user-facing text; every other
/// failure is reduced to the context, because the caller logs the error.
fn resolve_reply(error: &Error) -> String {
    match error {
        Error::Status(_) | Error::InvalidUrl => error.to_string(),
        _ => "could not unwall the article".to_owned(),
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

impl Unwall {
    /// Whether `sender` is authorized to manage unwall sites.
    fn is_admin(&self, sender: Sender<'_>) -> bool {
        self.admins.matches(sender)
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

        send_reply(client, channel, labeled_reply(NAME, UNAUTHORIZED))?;

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
                    send_reply(
                        client,
                        channel,
                        labeled_reply(
                            NAME,
                            format_args!("{RESET}{input}{COLOR} does not look like a hostname."),
                        ),
                    )?;

                    return Ok(());
                }
            }
        }

        if hosts.is_empty() {
            send_reply(
                client,
                channel,
                labeled_reply(NAME, "specify at least one hostname to unwall."),
            )?;

            return Ok(());
        }

        if let Err(error) = self.service.add_sites(&hosts, nickname).await {
            warn!(
                exception.message = %error,
                unwall.site.count = hosts.len(),
                "could not add the unwall sites"
            );

            send_reply(
                client,
                channel,
                labeled_reply(NAME, "could not add the sites."),
            )?;

            return Ok(());
        }

        debug!(unwall.site.count = hosts.len(), "added unwall sites");

        send_reply(
            client,
            channel,
            labeled_reply(
                NAME,
                format_args!("Now unwalling {}{COLOR}.", join_and(&hosts)),
            ),
        )?;

        Ok(())
    }

    /// Removes the covered sites matching the `rm`/`del`/`delete` subcommand pattern.
    async fn remove(&self, client: &Client, channel: &str, pattern: &str) -> Result<(), ZetaError> {
        let Some(pattern) = sites::normalize_site(pattern) else {
            send_reply(
                client,
                channel,
                labeled_reply(
                    NAME,
                    format_args!(
                        "{RESET}{pattern}{COLOR} does not look like a hostname or wildcard."
                    ),
                ),
            )?;

            return Ok(());
        };

        let removed = match self.service.remove_matching(&pattern).await {
            Ok(removed) => removed,
            Err(error) => {
                warn!(exception.message = %error, "could not remove the unwall sites");

                send_reply(
                    client,
                    channel,
                    labeled_reply(NAME, "could not remove the sites."),
                )?;

                return Ok(());
            }
        };

        let response = if removed.is_empty() {
            format!("No unwalled sites match{RESET} {pattern}{COLOR}.")
        } else {
            format!("Removed {}{COLOR} from unwalled sites.", join_and(&removed))
        };

        send_reply(client, channel, labeled_reply(NAME, response))?;

        Ok(())
    }

    /// Summarizes the covered sites in a single line.
    fn list(&self, client: &Client, channel: &str) -> Result<(), ZetaError> {
        let sites = self.service.sites();

        let response = if sites.is_empty() {
            labeled_reply(
                NAME,
                format_args!(
                    "{RESET}No unwalled sites yet{COLOR}: add one with\
                     {RESET} .unwall add <host>{COLOR}, e.g.{RESET} .unwall add bloomberg.com{COLOR}."
                ),
            )
        } else {
            let mut message = labeled_reply(
                NAME,
                format_args!("{RESET}{}{COLOR} unwalled sites:{RESET} ", sites.len()),
            );

            irc::append_entries_with_more(
                &mut message,
                sites.iter().map(|site| site.host.as_str()),
                sites.len(),
                &format!("{COLOR}, {RESET}"),
                MAX_LISTING_LENGTH,
                0,
                |dropped| format!("{COLOR} ({dropped} more)"),
            );

            message
        };

        send_reply(client, channel, response)?;

        Ok(())
    }

    /// Replies with the overall statistics of the unwalled links.
    async fn stats(&self, client: &Client, channel: &str) -> Result<(), ZetaError> {
        let response = match self.service.statistics().await {
            Ok(stats) => labeled_reply(
                NAME,
                format_args!(
                    "{RESET}{}{COLOR} links unwalled across{RESET} {}{COLOR} sites \
                     by{RESET} {}{COLOR} users ({RESET}{}{COLOR} today).",
                    stats.urls, stats.hosts, stats.users, stats.urls_today
                ),
            ),
            Err(error) => {
                warn!(exception.message = %error, "could not load the unwall statistics");

                labeled_reply(NAME, "could not load the statistics.")
            }
        };

        send_reply(client, channel, response)?;

        Ok(())
    }

    /// Replies with the statistics of one host.
    async fn info(&self, client: &Client, channel: &str, host: &str) -> Result<(), ZetaError> {
        let response = match self.service.host_statistics(host).await {
            Ok(Some(stats)) => labeled_reply(
                NAME,
                format_args!(
                    "{RESET}{}{COLOR} links unwalled for{RESET} {host}{COLOR} \
                     by{RESET} {}{COLOR} users ({RESET}{}{COLOR} today, most recent\
                     {RESET} {}{COLOR}).",
                    stats.urls,
                    stats.users,
                    stats.urls_today,
                    stats.latest.map_or_default(|latest| latest.time_ago()),
                ),
            ),
            Ok(None) => labeled_reply(
                NAME,
                format_args!("No unwalled links for{RESET} {host}{COLOR}."),
            ),
            Err(error) => {
                warn!(exception.message = %error, "could not load the unwall statistics");

                labeled_reply(NAME, "could not load the statistics.")
            }
        };

        send_reply(client, channel, response)?;

        Ok(())
    }

    /// Submits a covered article to unwall.app in a task of its own, replying with the reader
    /// link once the mirror is warm — or from the cache, when the article is known.
    ///
    /// Waits for one of [`SUBMIT_CONCURRENCY`] permits before the task is spawned, so a message
    /// with many covered links queues in the handler instead of spawning an unbounded number of
    /// tasks.
    async fn submit(&self, client: &Client, event: &UrlEvent) {
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
            attribution: Attribution::from_sender(sender, event.channel()),
            network_id: NETWORK_ID.to_owned(),
        };

        let service = Arc::clone(&self.service);

        plugins_shared::spawn_reply_task!(
            Arc::clone(&self.submits),
            client,
            event.channel(),
            "unwall.resolve",
            &article,
            move |irc, channel| async move {
                match service.resolve(&article, fetch).await {
                    Ok(cached) => {
                        // The link rides on the plain reply prefix, which colors the
                        // whole line cyan.
                        if let Err(error) =
                            send_reply_via(&irc, &channel, plain_reply(&cached.unwall_url))
                        {
                            warn!(exception.message = %error, "could not send the unwall link");
                        }
                    }
                    Err(error) => {
                        warn!(
                            exception.message = %error,
                            "could not unwall the article"
                        );

                        let reply = resolve_reply(&error);

                        if let Err(error) = send_reply_via(&irc, &channel, plain_reply(reply)) {
                            warn!(exception.message = %error, "could not send the unwall failure");
                        }
                    }
                }
            },
        )
        .await;
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

        let admins = Hostmasks::compile(&ctx.config.admin_hostmasks, "manage unwall sites");

        Ok(Self {
            service: Arc::new(UnwallService::new(ctx.db.clone(), client, reader_base)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use zeta_test_support::settings_tests;

    #[test]
    fn cli_definition_is_valid() {
        // clap reports contradictory definitions through asserts; evaluating them here turns a
        // bad definition into a test failure instead of a panic in debug builds.
        Opts::command().debug_assert();
    }

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

    #[test]
    fn resolve_reply_uses_the_domain_text() {
        assert_eq!(
            resolve_reply(&Error::Status(503)),
            "unwall could not fetch the article (HTTP 503)"
        );
        assert_eq!(
            resolve_reply(&Error::InvalidUrl),
            "not a usable article URL"
        );
    }

    #[test]
    fn resolve_reply_hides_internal_errors() {
        let reply = resolve_reply(&Error::BaseUrl(url::ParseError::EmptyHost));

        assert_eq!(reply, "could not unwall the article");
        assert!(!reply.contains("base URL"));
    }
}
