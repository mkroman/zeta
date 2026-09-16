//! Channel-scoped URL and sender filters.
//!
//! Filters are managed with the `.filter add|list|delete` command and stored in the database,
//! from which they are loaded into memory at launch. Every filter carries optional criteria —
//! the channel, the host and path of a URL, and the nickname, username (ident), and hostname of
//! the sender — where all of the set criteria must match for a message to be ignored. Patterns
//! support `*` and `?` wildcards.
//!
//! The command is restricted to admins: a sender is an admin when their `nick!user@host`
//! matches one of the hostmasks configured in `[irc] admin_hostmasks`, where each component
//! supports wildcards. With no hostmasks configured, nobody is an admin.
//!
//! The filter service is published to [`Context::shared`] when the plugin is constructed, so the
//! URL-handling plugins can consult it through the
//! [`Filters`](crate::plugin::filtering::Filters) facade.

mod error;
mod index;
mod model;
mod repository;
mod service;

// The module types are re-exported as part of the module's API surface, even though the plugin
// itself only handles them by value.
#[allow(unused_imports)]
pub use error::Error;
pub use model::{Filter, NewFilter};
pub use service::{Criteria, FilterService};

use std::sync::Arc;

use argh::{ArgsInfo, FromArgs};
use irc::proto::Command;
use tracing::{debug, warn};
use wildmatch::WildMatch;

use crate::plugin::prelude::*;

/// The `.filter` command.
const FILTER: PluginCommand = PluginCommand::with_args::<Opts>(
    Prefix::new(".filter"),
    "Manage URL and sender filters (add/list/delete)",
);

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[FILTER];

/// The maximum length of a listing, leaving room for `PRIVMSG` framing overhead within the
/// classic 512-byte IRC line limit.
const MAX_LISTING_LENGTH: usize = 400;

/// Manage URL and sender filters.
#[derive(FromArgs, ArgsInfo, Debug)]
struct Opts {
    /// the filter operation
    #[argh(subcommand)]
    command: Subcommand,
}

/// The `.filter` subcommands.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand)]
enum Subcommand {
    /// add a new filter
    Add(Add),
    /// list filters matching criteria
    List(List),
    /// delete a filter by id, or all filters matching criteria
    Delete(Delete),
}

/// Add a new filter.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "add")]
struct Add {
    /// only apply the filter in this channel (defaults to the current channel)
    #[argh(option)]
    channel: Option<String>,
    /// apply the filter in all channels
    #[argh(switch)]
    all_channels: bool,
    /// URL host pattern to filter, e.g. `imdb.com` or `*.com` (repeatable)
    #[argh(option)]
    host: Vec<String>,
    /// URL path pattern to filter, e.g. `/title/*` (repeatable)
    #[argh(option)]
    path: Vec<String>,
    /// sender nickname pattern to filter, e.g. `*bot`
    #[argh(option)]
    nick: Option<String>,
    /// sender username (ident) pattern to filter, e.g. `*other`
    #[argh(option)]
    user: Option<String>,
    /// sender hostname pattern to filter, e.g. `*.isp.example`
    #[argh(option)]
    hostname: Option<String>,
}

/// List filters.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "list")]
struct List {
    /// the channel pattern to match
    #[argh(option)]
    channel: Option<String>,
    /// the URL host pattern to match
    #[argh(option)]
    host: Option<String>,
    /// the URL path pattern to match
    #[argh(option)]
    path: Option<String>,
    /// the sender nickname pattern to match
    #[argh(option)]
    nick: Option<String>,
    /// the sender username pattern to match
    #[argh(option)]
    user: Option<String>,
    /// the sender hostname pattern to match
    #[argh(option)]
    hostname: Option<String>,
}

/// Delete filters.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "delete")]
struct Delete {
    /// the id of a single filter to remove
    #[argh(positional)]
    id: Option<i32>,
    /// the channel pattern to match
    #[argh(option)]
    channel: Option<String>,
    /// the URL host pattern to match
    #[argh(option)]
    host: Option<String>,
    /// the URL path pattern to match
    #[argh(option)]
    path: Option<String>,
    /// the sender nickname pattern to match
    #[argh(option)]
    nick: Option<String>,
    /// the sender username pattern to match
    #[argh(option)]
    user: Option<String>,
    /// the sender hostname pattern to match
    #[argh(option)]
    hostname: Option<String>,
    /// delete the matching filters without confirmation
    #[argh(switch)]
    force: bool,
}

impl Add {
    /// Validates the arguments, returning the channel scope of the new filters: `None` for all
    /// channels, or the channel to apply them in.
    fn validate(&self, current_channel: &str) -> Result<Option<String>, String> {
        if self.all_channels && self.channel.is_some() {
            return Err("use either --channel or --all-channels, not both".to_string());
        }

        let channel = if self.all_channels {
            None
        } else if let Some(channel) = self.channel.as_deref() {
            if index::is_wildcard(channel) {
                return Err(
                    "--channel does not support wildcards; use --all-channels to match every channel"
                        .to_string(),
                );
            }

            Some(channel.to_owned())
        } else {
            Some(current_channel.to_owned())
        };

        for (name, value) in [
            ("--host", self.host.as_slice()),
            ("--path", self.path.as_slice()),
        ] {
            if value.iter().any(|pattern| pattern.trim().is_empty()) {
                return Err(format!("{name} patterns cannot be empty"));
            }
        }

        for (name, value) in [
            ("--nick", &self.nick),
            ("--user", &self.user),
            ("--hostname", &self.hostname),
        ] {
            if value.as_deref().is_some_and(|pattern| pattern.trim().is_empty()) {
                return Err(format!("{name} patterns cannot be empty"));
            }
        }

        if self.host.is_empty()
            && self.path.is_empty()
            && self.nick.is_none()
            && self.user.is_none()
            && self.hostname.is_none()
        {
            return Err(
                "at least one of --host, --path, --nick, --user or --hostname is required"
                    .to_string(),
            );
        }

        Ok(channel)
    }

    /// Returns one filter per host × path combination of the arguments.
    fn new_filters(&self, channel: Option<&str>, created_by: &str) -> Vec<NewFilter> {
        let hosts: Vec<Option<String>> = if self.host.is_empty() {
            vec![None]
        } else {
            self.host
                .iter()
                .map(|host| Some(host.trim().to_owned()))
                .collect()
        };

        let paths: Vec<Option<String>> = if self.path.is_empty() {
            vec![None]
        } else {
            self.path
                .iter()
                .map(|path| Some(path.trim().to_owned()))
                .collect()
        };

        let mut filters = Vec::new();

        for host in &hosts {
            for path in &paths {
                filters.push(NewFilter {
                    channel: channel.map(str::to_owned),
                    host: host.clone(),
                    path: path.clone(),
                    nickname: trimmed(self.nick.as_deref()),
                    username: trimmed(self.user.as_deref()),
                    hostname: trimmed(self.hostname.as_deref()),
                    created_by: created_by.to_owned(),
                });
            }
        }

        filters
    }
}

impl From<&List> for Criteria {
    fn from(list: &List) -> Criteria {
        Criteria {
            channel: list.channel.clone(),
            host: list.host.clone(),
            path: list.path.clone(),
            nickname: list.nick.clone(),
            username: list.user.clone(),
            hostname: list.hostname.clone(),
        }
    }
}

impl Delete {
    /// Returns the deletion criteria, or `None` if no criterion was given.
    fn criteria(&self) -> Option<Criteria> {
        let criteria = Criteria {
            channel: self.channel.clone(),
            host: self.host.clone(),
            path: self.path.clone(),
            nickname: self.nick.clone(),
            username: self.user.clone(),
            hostname: self.hostname.clone(),
        };

        let is_empty = criteria.channel.is_none()
            && criteria.host.is_none()
            && criteria.path.is_none()
            && criteria.nickname.is_none()
            && criteria.username.is_none()
            && criteria.hostname.is_none();

        (!is_empty).then_some(criteria)
    }
}

/// Returns `value` with surrounding whitespace trimmed, if set.
fn trimmed(value: Option<&str>) -> Option<String> {
    value.map(str::trim).map(str::to_owned)
}

/// Compiles admin hostmasks into case-insensitive wildcard matchers, skipping blank entries.
fn compile_hostmasks(hostmasks: &[String]) -> Vec<WildMatch> {
    hostmasks
        .iter()
        .map(|hostmask| hostmask.trim())
        .filter(|hostmask| !hostmask.is_empty())
        .map(WildMatch::new_case_insensitive)
        .collect()
}

/// Whether `sender`'s `nick!user@host` matches any of the compiled admin hostmasks.
fn hostmask_matches(admins: &[WildMatch], sender: Sender<'_>) -> bool {
    let hostmask = format!("{}!{}@{}", sender.nick, sender.username, sender.hostname);

    admins.iter().any(|pattern| pattern.matches(&hostmask))
}

/// Filter plugin.
///
/// Manages the database-backed URL and sender filters and publishes the filter service for the
/// URL-handling plugins to consult.
pub struct FilterPlugin {
    /// The filter service, also published for other plugins to use.
    service: Arc<FilterService>,
    /// The compiled admin hostmasks; senders whose `nick!user@host` matches one of them are
    /// authorized to manage filters.
    admins: Vec<WildMatch>,
}

/// The reply sent to senders that are not authorized to manage filters.
const UNAUTHORIZED: &str = "You are not authorized to manage filters.";

impl FilterPlugin {
    /// Whether `sender` is authorized to manage filters.
    fn is_admin(&self, sender: Sender<'_>) -> bool {
        hostmask_matches(&self.admins, sender)
    }

    /// Adds one or more filters from the `add` subcommand.
    async fn add(
        &self,
        client: &Client,
        channel: &str,
        nickname: &str,
        opts: Add,
    ) -> Result<(), ZetaError> {
        let scope = match opts.validate(channel) {
            Ok(scope) => scope,
            Err(error) => {
                client.send_privmsg(channel, formatted(&error))?;

                return Ok(());
            }
        };

        let new_filters = opts.new_filters(scope.as_deref(), nickname);

        for new_filter in &new_filters {
            if let Err(error) = self.service.add(new_filter.clone()).await {
                client.send_privmsg(channel, formatted(&format!("could not add the filter: {error}")))?;

                return Ok(());
            }
        }

        let reply = if let [new_filter] = new_filters.as_slice()
            && new_filter.path.is_none()
            && new_filter.nickname.is_none()
            && new_filter.username.is_none()
            && new_filter.hostname.is_none()
            && let Some(host) = &new_filter.host
        {
            format!("The filter for {host} has been added.")
        } else {
            "The filter has been added.".to_string()
        };

        debug!(count = new_filters.len(), "added filters");

        client.send_privmsg(channel, formatted(&reply))?;

        Ok(())
    }

    /// Lists the filters matching the `list` subcommand criteria.
    fn list(&self, client: &Client, channel: &str, opts: &List) -> Result<(), ZetaError> {
        let filters = self.service.select(&Criteria::from(opts));

        let reply = if filters.is_empty() {
            formatted("No filters match your criteria.")
        } else {
            let mut listing = formatted(&format!(
                "{} filters matching your criteria: ",
                filters.len()
            ));

            append_entries(&mut listing, filters.iter().map(describe), "");

            listing
        };

        client.send_privmsg(channel, reply)?;

        Ok(())
    }

    /// Deletes a filter by id, or all filters matching the `delete` subcommand criteria.
    async fn delete(&self, client: &Client, channel: &str, opts: Delete) -> Result<(), ZetaError> {
        if let Some(id) = opts.id {
            if opts.criteria().is_some() {
                client.send_privmsg(
                    channel,
                    formatted("specify either a filter id or criteria, not both"),
                )?;

                return Ok(());
            }

            let reply = match self.service.delete_ids(&[id]).await {
                Ok(1) => "The filter has been removed.".to_string(),
                Ok(0) => format!("No filter with id {id}."),
                Ok(removed) => format!("{removed} filters have been removed."),
                Err(error) => format!("could not delete the filter: {error}"),
            };

            client.send_privmsg(channel, formatted(&reply))?;

            return Ok(());
        }

        let Some(criteria) = opts.criteria() else {
            client.send_privmsg(
                channel,
                formatted("specify a filter id, or at least one criterion to match filters by"),
            )?;

            return Ok(());
        };

        let filters = self.service.select(&criteria);

        if filters.is_empty() {
            client.send_privmsg(channel, formatted("No filters match your criteria."))?;

            return Ok(());
        }

        let ids: Vec<i32> = filters.iter().map(|filter| filter.id).collect();

        if !opts.force {
            let mut listing = formatted(&format!(
                "{} filters matching the criteria will be deleted: ",
                filters.len()
            ));

            append_entries(
                &mut listing,
                filters.iter().map(describe),
                " - use --force to proceed.",
            );

            client.send_privmsg(channel, listing)?;

            return Ok(());
        }

        let reply = match self.service.delete_ids(&ids).await {
            Ok(removed) => format!("{removed} filters have been removed."),
            Err(error) => format!("could not delete the filters: {error}"),
        };

        client.send_privmsg(channel, formatted(&reply))?;

        Ok(())
    }
}

#[async_trait]
impl Plugin<Context> for FilterPlugin {
    type Settings = NoSettings;

    fn new(ctx: &Context, _settings: &NoSettings) -> Result<Self, ZetaError> {
        let service = Arc::new(FilterService::new(ctx.db.clone()));

        ctx.shared.publish(Arc::clone(&service));

        let admins = compile_hostmasks(&ctx.config.irc.admin_hostmasks);

        if admins.is_empty() {
            warn!(
                "no admin hostmasks configured; nobody is authorized to manage filters \
                 (set [irc] admin_hostmasks)"
            );
        } else {
            debug!(count = admins.len(), "loaded admin hostmasks");
        }

        Ok(FilterPlugin { service, admins })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "filter".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn commands(&self) -> &'static [PluginCommand] {
        COMMANDS
    }

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        self.service.load().await.map_err(plugin_err)?;

        debug!(count = self.service.list().len(), "loaded filters");

        Ok(())
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        let Command::PRIVMSG(channel, msg) = &message.command else {
            return Ok(());
        };

        let Some(sender) = Sender::from_message(message) else {
            return Ok(());
        };

        let Some(args) = FILTER.parse(msg) else {
            return Ok(());
        };

        if !self.is_admin(sender) {
            client.send_privmsg(channel, formatted(UNAUTHORIZED))?;

            return Ok(());
        }

        let opts = match FILTER.parse_words::<Opts>(args) {
            Ok(opts) => opts,
            Err(err) => {
                for line in err.to_string().lines().filter(|line| !line.is_empty()) {
                    client.send_privmsg(channel, formatted(line))?;
                }

                return Ok(());
            }
        };

        match opts.command {
            Subcommand::Add(add) => self.add(client, channel, sender.nick, add).await,
            Subcommand::List(list) => self.list(client, channel, &list),
            Subcommand::Delete(delete) => self.delete(client, channel, delete).await,
        }
    }
}

/// Appends `entries` to `message`, separated by `, `, as long as the listing — including
/// `suffix` — stays within [`MAX_LISTING_LENGTH`]. The first entry is always included, so the
/// count in the header is never misleading. Returns whether every entry was included.
fn append_entries(
    message: &mut String,
    entries: impl Iterator<Item = String>,
    suffix: &str,
) -> bool {
    let mut first = message.ends_with(' ');
    let mut complete = true;

    for entry in entries {
        let separator = if first { "" } else { ", " };

        if !first && message.len() + separator.len() + entry.len() + suffix.len() > MAX_LISTING_LENGTH
        {
            complete = false;

            break;
        }

        message.push_str(separator);
        message.push_str(&entry);
        first = false;
    }

    if !complete || !suffix.is_empty() {
        message.push_str(suffix);
    }

    complete
}

/// Formats a filter for listings.
fn describe(filter: &Filter) -> String {
    let mut parts = Vec::new();

    if let Some(host) = filter.host.as_deref() {
        parts.push(format!("host={host}"));
    }

    if let Some(path) = filter.path.as_deref() {
        parts.push(format!("path={path}"));
    }

    if let Some(nick) = filter.nickname.as_deref() {
        parts.push(format!("nick={nick}"));
    }

    if let Some(user) = filter.username.as_deref() {
        parts.push(format!("user={user}"));
    }

    if let Some(hostname) = filter.hostname.as_deref() {
        parts.push(format!("hostname={hostname}"));
    }

    let scope = filter.channel.as_deref().unwrap_or("all channels");

    format!("#{}: {} ({})", filter.id, parts.join(" "), scope)
}

/// Formats `s` as a filter response.
fn formatted(s: &str) -> String {
    format!("\x0310>\x0f\x02 Filter:\x02\x0310 {s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_hostmasks_match_wildcards_and_case_insensitively() {
        let admins = compile_hostmasks(&["mk!mk@*".to_string(), "*!*@example.org".to_string()]);

        assert!(hostmask_matches(&admins, Sender::new("mk", "mk", "user.example")));
        assert!(hostmask_matches(&admins, Sender::new("MK", "MK", "anything.tld")));
        assert!(hostmask_matches(&admins, Sender::new("someone", "x", "EXAMPLE.ORG")));
        assert!(!hostmask_matches(&admins, Sender::new("someone", "x", "example.com")));
        assert!(!hostmask_matches(&admins, Sender::new("mk", "other", "user.example")));
    }

    #[test]
    fn an_empty_admin_list_denies_everyone() {
        let admins = compile_hostmasks(&[]);

        assert!(!hostmask_matches(&admins, Sender::new("mk", "mk", "example.com")));
    }

    #[test]
    fn blank_admin_hostmasks_are_skipped() {
        let admins = compile_hostmasks(&["  ".to_string(), String::new()]);

        assert!(!hostmask_matches(&admins, Sender::new("mk", "mk", "example.com")));
    }

    #[test]
    fn parses_add_with_repeated_hosts() {
        let opts: Opts = FILTER
            .parse_words("add --host imdb.com --host www.imdb.com --user other")
            .unwrap();

        let Subcommand::Add(add) = opts.command else {
            panic!("expected the add subcommand");
        };

        assert_eq!(add.host, ["imdb.com", "www.imdb.com"]);
        assert_eq!(add.user.as_deref(), Some("other"));
    }

    #[test]
    fn builds_one_filter_per_host_and_path() {
        let opts: Opts = FILTER
            .parse_words("add --host a.com --host b.com --path /x/*")
            .unwrap();

        let Subcommand::Add(add) = opts.command else {
            panic!("expected the add subcommand");
        };

        let filters = add.new_filters(Some("#chan"), "smoke");

        assert_eq!(filters.len(), 2);
        assert!(filters.iter().all(|filter| filter.path.as_deref() == Some("/x/*")));
        assert_eq!(filters[0].host.as_deref(), Some("a.com"));
        assert_eq!(filters[1].host.as_deref(), Some("b.com"));
        assert!(filters.iter().all(|filter| filter.channel.as_deref() == Some("#chan")));
        assert!(filters.iter().all(|filter| filter.created_by == "smoke"));
    }

    #[test]
    fn builds_a_hostless_filter_when_only_sender_criteria_are_given() {
        let opts: Opts = FILTER.parse_words("add --user *other").unwrap();

        let Subcommand::Add(add) = opts.command else {
            panic!("expected the add subcommand");
        };

        let filters = add.new_filters(None, "smoke");

        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].host, None);
        assert_eq!(filters[0].channel, None);
        assert_eq!(filters[0].username.as_deref(), Some("*other"));
    }

    #[test]
    fn add_requires_a_criterion() {
        let opts: Opts = FILTER.parse_words("add").unwrap();

        let Subcommand::Add(add) = opts.command else {
            panic!("expected the add subcommand");
        };

        assert!(add.validate("#chan").is_err());
    }

    #[test]
    fn add_rejects_channel_and_all_channels_together() {
        let opts: Opts = FILTER
            .parse_words("add --channel #foo --all-channels --host example.com")
            .unwrap();

        let Subcommand::Add(add) = opts.command else {
            panic!("expected the add subcommand");
        };

        assert!(add.validate("#chan").is_err());
    }

    #[test]
    fn add_rejects_wildcard_channels() {
        let opts: Opts = FILTER
            .parse_words("add --channel #foo* --host example.com")
            .unwrap();

        let Subcommand::Add(add) = opts.command else {
            panic!("expected the add subcommand");
        };

        assert!(add.validate("#chan").is_err());
    }

    #[test]
    fn add_defaults_to_the_current_channel() {
        let opts: Opts = FILTER.parse_words("add --host example.com").unwrap();

        let Subcommand::Add(add) = opts.command else {
            panic!("expected the add subcommand");
        };

        assert_eq!(add.validate("#chan").unwrap().as_deref(), Some("#chan"));
        assert!(add.validate("#chan").is_ok());
    }

    #[test]
    fn delete_parses_id_and_force() {
        let opts: Opts = FILTER.parse_words("delete 12 --force").unwrap();

        let Subcommand::Delete(delete) = opts.command else {
            panic!("expected the delete subcommand");
        };

        assert_eq!(delete.id, Some(12));
        assert!(delete.force);
        assert!(delete.criteria().is_none());
    }

    #[test]
    fn delete_collects_criteria() {
        let opts: Opts = FILTER.parse_words("delete --host *.com").unwrap();

        let Subcommand::Delete(delete) = opts.command else {
            panic!("expected the delete subcommand");
        };

        assert_eq!(delete.id, None);
        assert_eq!(delete.criteria().unwrap().host.as_deref(), Some("*.com"));
    }

    #[test]
    fn list_criteria_convert() {
        let opts: Opts = FILTER.parse_words("list --host *.com").unwrap();

        let Subcommand::List(list) = opts.command else {
            panic!("expected the list subcommand");
        };

        let criteria = Criteria::from(&list);

        assert_eq!(criteria.host.as_deref(), Some("*.com"));
        assert!(criteria.channel.is_none());
    }

    #[test]
    fn describes_filters() {
        let filter = Filter {
            id: 12,
            channel: Some("#foo".into()),
            host: Some("*.com".into()),
            path: None,
            nickname: None,
            username: Some("*other".into()),
            hostname: None,
            created_by: "smoke".into(),
            created_at: sqlx::types::chrono::Utc::now(),
        };

        assert_eq!(describe(&filter), "#12: host=*.com user=*other (#foo)");

        let global = Filter {
            id: 3,
            channel: None,
            host: None,
            path: Some("/title/*".into()),
            nickname: None,
            username: None,
            hostname: None,
            created_by: "smoke".into(),
            created_at: sqlx::types::chrono::Utc::now(),
        };

        assert_eq!(describe(&global), "#3: path=/title/* (all channels)");
    }

    #[test]
    fn listings_stay_within_the_budget() {
        let mut message = formatted("2 filters matching your criteria: ");

        let complete = append_entries(
            &mut message,
            [
                format!("long entry {}", "x".repeat(180)),
                format!("long entry {}", "y".repeat(180)),
            ]
            .into_iter(),
            "",
        );

        assert!(!complete);
        assert!(message.len() <= MAX_LISTING_LENGTH);
        assert!(message.contains("long entry xxx"));
    }
}
