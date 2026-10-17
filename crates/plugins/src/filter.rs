//! Silences matched senders and URLs in channels.
//!
//! Filters are managed with the `.filter add|list|delete` command and stored in the database,
//! from which they are loaded into memory at launch. Every filter carries optional criteria —
//! the channel, the host and path of a URL, and the nickname, username (ident), and hostname of
//! the sender — where all of the set criteria must match. Patterns support `*` and `?`
//! wildcards.
//!
//! What a match means depends on the criteria: a filter with URL criteria (host or path)
//! ignores the matching URLs, which the host's dispatcher drops before any URL plugin sees
//! them. A filter with sender criteria only ignores the matching senders entirely: their
//! commands do not trigger, their messages are not delivered to the message subscribers, and
//! their URLs are ignored.
//!
//! The command is restricted to admins through `[irc] admin_hostmasks`: the hostmask matching
//! lives in the crate-private `hostmask` module, shared with `.unwall`.
//!
//! The filter service is published to [`Context::shared`] as a type-erased [`UrlFilters`] handle
//! when the plugin is constructed; the host's dispatcher consults it through the [`Filters`]
//! facade before delivering URL events, so filtered URLs never reach the URL plugins.

mod error;
mod index;
mod model;
mod repository;
mod service;

// The module types are re-exported as part of the module's API surface, even though the plugin
// itself only handles them by value.
#[allow(unused_imports)]
pub use error::Error;
pub use model::{Criteria, Filter, NewFilter};
pub use service::FilterService;

use std::sync::Arc;

use argh::{ArgsInfo, FromArgs};
use tracing::debug;

use crate::hostmask::Hostmasks;
use crate::prelude::*;
use zeta_utils::utils::{MAX_LISTING_LENGTH, append_entries_within_budget};

/// The `.filter` command.
const FILTER: CommandSpec =
    CommandSpec::with_args::<Opts>(".filter", "Manage URL and sender filters (add/list/delete)");

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
            if value
                .as_deref()
                .is_some_and(|pattern| pattern.trim().is_empty())
            {
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
        let hosts = expand(&self.host);
        let paths = expand(&self.path);
        let (nickname, username, hostname) = (
            self.nick.as_deref().map(str::trim).map(str::to_owned),
            self.user.as_deref().map(str::trim).map(str::to_owned),
            self.hostname.as_deref().map(str::trim).map(str::to_owned),
        );

        let mut filters = Vec::with_capacity(hosts.len() * paths.len());

        for host in &hosts {
            for path in &paths {
                filters.push(NewFilter {
                    channel: channel.map(str::to_owned),
                    host: host.clone(),
                    path: path.clone(),
                    nickname: nickname.clone(),
                    username: username.clone(),
                    hostname: hostname.clone(),
                    created_by: created_by.to_owned(),
                });
            }
        }

        filters
    }
}

/// Expands the repeated `--host` or `--path` values into filter fields: no values yield the
/// single unset field, so a filter without either matches every URL.
fn expand(values: &[String]) -> Vec<Option<String>> {
    if values.is_empty() {
        vec![None]
    } else {
        values
            .iter()
            .map(|value| Some(value.trim().to_owned()))
            .collect()
    }
}

/// Builds `Criteria` from the pattern options `list` and `delete` share.
impl From<&List> for Criteria {
    fn from(list: &List) -> Self {
        Self {
            channel: list.channel.clone(),
            host: list.host.clone(),
            path: list.path.clone(),
            nickname: list.nick.clone(),
            username: list.user.clone(),
            hostname: list.hostname.clone(),
        }
    }
}

impl From<&Delete> for Criteria {
    fn from(delete: &Delete) -> Self {
        Self {
            channel: delete.channel.clone(),
            host: delete.host.clone(),
            path: delete.path.clone(),
            nickname: delete.nick.clone(),
            username: delete.user.clone(),
            hostname: delete.hostname.clone(),
        }
    }
}

impl Delete {
    /// Returns the deletion criteria, or `None` if no criterion was given.
    fn criteria(&self) -> Option<Criteria> {
        let criteria = Criteria::from(self);

        (!criteria.is_empty()).then_some(criteria)
    }
}

/// The confirmation reply for the added filters: a single plain host filter gets a message
/// naming the host.
fn added_response(new_filters: &[NewFilter]) -> String {
    if let [new_filter] = new_filters
        && new_filter.is_host_only()
        && let Some(host) = &new_filter.host
    {
        format!("The filter for {host} has been added.")
    } else {
        "The filter has been added.".to_string()
    }
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
    admins: Hostmasks,
}

/// The reply sent to senders that are not authorized to manage filters.
const UNAUTHORIZED: &str = "You are not authorized to manage filters.";

impl FilterPlugin {
    /// Whether `sender` is authorized to manage filters.
    fn is_admin(&self, sender: Sender<'_>) -> bool {
        self.admins.matches(sender)
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
                client.send_privmsg(channel, reply("Filter", &error))?;

                return Ok(());
            }
        };

        let new_filters = opts.new_filters(scope.as_deref(), nickname);
        let count = new_filters.len();
        let response = added_response(&new_filters);

        for new_filter in new_filters {
            if let Err(error) = self.service.add(new_filter).await {
                client.send_privmsg(
                    channel,
                    reply("Filter", format!("could not add the filter: {error}")),
                )?;

                return Ok(());
            }
        }

        debug!(count, "added filters");

        client.send_privmsg(channel, reply("Filter", &response))?;

        Ok(())
    }

    /// Lists the filters matching the `list` subcommand criteria.
    fn list(&self, client: &Client, channel: &str, opts: &List) -> Result<(), ZetaError> {
        let filters = self.service.select(&Criteria::from(opts));

        let response = if filters.is_empty() {
            reply("Filter", "No filters match your criteria.")
        } else {
            listing(
                reply(
                    "Filter",
                    format!("{} filters matching your criteria: ", filters.len()),
                ),
                &filters,
                0,
            )
        };

        client.send_privmsg(channel, response)?;

        Ok(())
    }

    /// Deletes a filter by id, or all filters matching the `delete` subcommand criteria.
    async fn delete(&self, client: &Client, channel: &str, opts: Delete) -> Result<(), ZetaError> {
        if let Some(id) = opts.id {
            if opts.criteria().is_some() {
                client.send_privmsg(
                    channel,
                    reply("Filter", "specify either a filter id or criteria, not both"),
                )?;

                return Ok(());
            }

            let response = match self.service.delete_ids(&[id]).await {
                Ok(1) => "The filter has been removed.".to_string(),
                Ok(0) => format!("No filter with id {id}."),
                Ok(removed) => format!("{removed} filters have been removed."),
                Err(error) => format!("could not delete the filter: {error}"),
            };

            client.send_privmsg(channel, reply("Filter", &response))?;

            return Ok(());
        }

        let Some(criteria) = opts.criteria() else {
            client.send_privmsg(
                channel,
                reply(
                    "Filter",
                    "specify a filter id, or at least one criterion to match filters by",
                ),
            )?;

            return Ok(());
        };

        let filters = self.service.select(&criteria);

        if filters.is_empty() {
            client.send_privmsg(channel, reply("Filter", "No filters match your criteria."))?;

            return Ok(());
        }

        let ids: Vec<i32> = filters.iter().map(|filter| filter.id).collect();

        if !opts.force {
            const FORCE_SUFFIX: &str = " - use --force to proceed.";

            let mut confirmation = listing(
                reply(
                    "Filter",
                    format!(
                        "{} filters matching the criteria will be deleted: ",
                        filters.len()
                    ),
                ),
                &filters,
                FORCE_SUFFIX.len(),
            );

            confirmation.push_str(FORCE_SUFFIX);

            client.send_privmsg(channel, confirmation)?;

            return Ok(());
        }

        let response = match self.service.delete_ids(&ids).await {
            Ok(removed) => format!("{removed} filters have been removed."),
            Err(error) => format!("could not delete the filters: {error}"),
        };

        client.send_privmsg(channel, reply("Filter", &response))?;

        Ok(())
    }
}

#[async_trait]
impl Plugin<Context> for FilterPlugin {
    type Settings = NoSettings;

    fn new(
        ctx: &Context,
        _settings: &NoSettings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, ZetaError> {
        subscriptions.command(FILTER);
        let service = Arc::new(FilterService::new(ctx.db.clone()));

        ctx.shared.publish(Arc::new(UrlFilters::new(service.clone())));

        let admins = Hostmasks::compile(&ctx.config.admin_hostmasks, "manage filters");

        Ok(Self { service, admins })
    }

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        self.service.load().await.map_err(plugin_err)?;

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

        if !self.is_admin(sender) {
            client.send_privmsg(channel, reply("Filter", UNAUTHORIZED))?;

            return Ok(());
        }

        let Some(opts) =
            parse_words_or_usage::<Opts>(client, command, |line| reply("Filter", line))?
        else {
            return Ok(());
        };

        match opts.command {
            Subcommand::Add(add) => self.add(client, channel, sender.nick, add).await,
            Subcommand::List(list) => self.list(client, channel, &list),
            Subcommand::Delete(delete) => self.delete(client, channel, delete).await,
        }
    }
}

/// Appends the described `filters` to `header`, within the line budget and leaving room for a
/// suffix of `reserved` bytes.
fn listing(header: String, filters: &[Filter], reserved: usize) -> String {
    let mut listing = header;

    append_entries_within_budget(
        &mut listing,
        filters.iter().map(describe),
        ", ",
        MAX_LISTING_LENGTH,
        reserved,
    );

    listing
}

/// Formats a filter for listings.
fn describe(filter: &Filter) -> String {
    let criteria = [
        ("host", filter.host.as_deref()),
        ("path", filter.path.as_deref()),
        ("nick", filter.nickname.as_deref()),
        ("user", filter.username.as_deref()),
        ("hostname", filter.hostname.as_deref()),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|value| format!("{name}={value}")))
    .collect::<Vec<_>>()
    .join(" ");

    let scope = filter.channel.as_deref().unwrap_or("all channels");

    format!("#{}: {criteria} ({scope})", filter.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publishes_a_url_filters_handle_for_the_host() {
        // The host's dispatcher consults the `Filters` facade, which only sees the service
        // through the type-erased handle published here — without it, no URL is ever filtered.
        let ctx = zeta_test_support::test_context();
        let mut subscriptions = Subscriptions::new();

        <FilterPlugin as Plugin<Context>>::new(&ctx, &NoSettings {}, &mut subscriptions).unwrap();

        assert!(ctx.shared.get::<UrlFilters>().is_some());
    }

    /// Parses `input` with the filter command spec and unpacks the `add` subcommand.
    fn parse_add(input: &str) -> Add {
        let opts: Opts = FILTER
            .parse_words(input)
            .unwrap_or_else(|error| panic!("`{input}` should parse: {error}"));

        let Subcommand::Add(add) = opts.command else {
            panic!("`{input}` should be the add subcommand");
        };

        add
    }

    /// Parses `input` with the filter command spec and unpacks the `delete` subcommand.
    fn parse_delete(input: &str) -> Delete {
        let opts: Opts = FILTER
            .parse_words(input)
            .unwrap_or_else(|error| panic!("`{input}` should parse: {error}"));

        let Subcommand::Delete(delete) = opts.command else {
            panic!("`{input}` should be the delete subcommand");
        };

        delete
    }

    #[test]
    fn parses_add_with_repeated_hosts() {
        let add = parse_add("add --host imdb.com --host www.imdb.com --user other");

        assert_eq!(add.host, ["imdb.com", "www.imdb.com"]);
        assert_eq!(add.user.as_deref(), Some("other"));
    }

    #[test]
    fn builds_one_filter_per_host_and_path() {
        let add = parse_add("add --host a.com --host b.com --path /x/*");

        let filters = add.new_filters(Some("#chan"), "smoke");

        assert_eq!(filters.len(), 2);
        assert!(
            filters
                .iter()
                .all(|filter| filter.path.as_deref() == Some("/x/*"))
        );
        assert_eq!(filters[0].host.as_deref(), Some("a.com"));
        assert_eq!(filters[1].host.as_deref(), Some("b.com"));
        assert!(
            filters
                .iter()
                .all(|filter| filter.channel.as_deref() == Some("#chan"))
        );
    }

    #[test]
    fn builds_a_hostless_filter_when_only_sender_criteria_are_given() {
        let add = parse_add("add --user *other");

        let filters = add.new_filters(None, "smoke");

        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].host, None);
        assert_eq!(filters[0].channel, None);
        assert_eq!(filters[0].username.as_deref(), Some("*other"));
    }

    #[test]
    fn a_single_plain_host_filter_is_confirmed_by_host() {
        let add = parse_add("add --host example.com");

        assert_eq!(
            added_response(&add.new_filters(Some("#chan"), "smoke")),
            "The filter for example.com has been added."
        );
    }

    #[test]
    fn combined_criteria_get_a_plain_confirmation() {
        let add = parse_add("add --host example.com --path /title/*");

        assert_eq!(
            added_response(&add.new_filters(None, "smoke")),
            "The filter has been added."
        );

        let repeated = parse_add("add --host a.com --host b.com");

        assert_eq!(
            added_response(&repeated.new_filters(None, "smoke")),
            "The filter has been added."
        );
    }

    #[test]
    fn add_requires_a_criterion() {
        let add = parse_add("add");

        assert!(add.validate("#chan").is_err());
    }

    #[test]
    fn add_rejects_channel_and_all_channels_together() {
        let add = parse_add("add --channel #foo --all-channels --host example.com");

        assert!(add.validate("#chan").is_err());
    }

    #[test]
    fn add_rejects_wildcard_channels() {
        let add = parse_add("add --channel #foo* --host example.com");

        assert!(add.validate("#chan").is_err());
    }

    #[test]
    fn add_defaults_to_the_current_channel() {
        let add = parse_add("add --host example.com");

        assert_eq!(add.validate("#chan").unwrap().as_deref(), Some("#chan"));
    }

    #[test]
    fn delete_parses_id_and_force() {
        let delete = parse_delete("delete 12 --force");

        assert_eq!(delete.id, Some(12));
        assert!(delete.force);
        assert!(delete.criteria().is_none());
    }

    #[test]
    fn delete_collects_criteria() {
        let delete = parse_delete("delete --host *.com");

        assert_eq!(delete.id, None);
        assert_eq!(delete.criteria().unwrap().host.as_deref(), Some("*.com"));
    }

    #[test]
    fn list_criteria_convert() {
        let opts: Opts = FILTER
            .parse_words("list --host *.com")
            .expect("`list --host *.com` should parse");

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
        };

        assert_eq!(describe(&global), "#3: path=/title/* (all channels)");
    }

    #[test]
    fn listings_stay_within_the_budget() {
        let mut message = reply("Filter", "2 filters matching your criteria: ");

        let appended = append_entries_within_budget(
            &mut message,
            [
                format!("long entry {}", "x".repeat(180)),
                format!("long entry {}", "y".repeat(180)),
            ],
            ", ",
            MAX_LISTING_LENGTH,
            0,
        );

        assert_eq!(appended, 1);
        assert!(message.len() <= MAX_LISTING_LENGTH);
        assert!(message.contains("long entry xxx"));
    }
}
