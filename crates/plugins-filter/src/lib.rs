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
//! The command is restricted to admins through `[irc] admin_hostmasks`, using the plugin API's
//! shared [`Hostmasks`] matcher (also used by `.unwall`).
//!
//! The filter service is published to [`Context::shared`] as a type-erased [`Filters`] handle
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

use clap::Parser;
use tracing::debug;

use zeta_plugin::prelude::*;
use zeta_utils::irc::{MAX_LISTING_LENGTH, append_entries_with_more};
use zeta_utils::text::pluralize;

/// The `.filter` command.
const FILTER: CommandSpec =
    CommandSpec::with_args::<Opts>(".filter", "Manage URL and sender filters (add/list/delete)");

/// Manage URL and sender filters.
#[derive(Parser, Debug)]
struct Opts {
    /// the filter operation
    #[command(subcommand)]
    command: Subcommand,
}

/// The `.filter` subcommands.
#[derive(clap::Subcommand, Debug)]
enum Subcommand {
    /// add a new filter
    Add(Add),
    /// list filters matching criteria
    List(List),
    /// delete a filter by id, or all filters matching criteria
    Delete(Delete),
}

/// The sender patterns `add`, `list` and `delete` share.
#[derive(clap::Args, Debug, Default)]
struct SenderArgs {
    /// sender nickname pattern, e.g. `*bot`
    #[arg(long)]
    nick: Option<String>,
    /// sender username (ident) pattern, e.g. `*other`
    #[arg(long)]
    user: Option<String>,
    /// sender hostname pattern, e.g. `*.isp.example`
    #[arg(long)]
    hostname: Option<String>,
}

/// The filter selection criteria `list` and `delete` share.
#[derive(clap::Args, Debug, Default)]
struct CriteriaArgs {
    /// the channel pattern to match
    #[arg(long)]
    channel: Option<String>,
    /// the URL host pattern to match
    #[arg(long)]
    host: Option<String>,
    /// the URL path pattern to match
    #[arg(long)]
    path: Option<String>,
    /// the sender patterns to match
    #[command(flatten)]
    sender: SenderArgs,
}

/// Add a new filter.
#[derive(clap::Args, Debug)]
struct Add {
    /// only apply the filter in this channel (defaults to the current channel)
    #[arg(long)]
    channel: Option<String>,
    /// apply the filter in all channels
    #[arg(long)]
    all_channels: bool,
    /// URL host pattern to filter, e.g. `imdb.com` or `*.com` (repeatable)
    #[arg(long)]
    host: Vec<String>,
    /// URL path pattern to filter, e.g. `/title/*` (repeatable)
    #[arg(long)]
    path: Vec<String>,
    /// the sender patterns to filter
    #[command(flatten)]
    sender: SenderArgs,
}

/// List filters.
#[derive(clap::Args, Debug)]
struct List {
    /// the criteria to match
    #[command(flatten)]
    criteria: CriteriaArgs,
}

/// Delete filters.
#[derive(clap::Args, Debug)]
struct Delete {
    /// the id of a single filter to remove
    id: Option<i32>,
    /// the criteria to match
    #[command(flatten)]
    criteria: CriteriaArgs,
    /// delete the matching filters without confirmation
    #[arg(long)]
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

            if !channel.starts_with(['#', '&']) {
                return Err(format!(
                    "--channel should be a channel name starting with # or & — did you mean \
                     #{channel}?"
                ));
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
            ("--nick", &self.sender.nick),
            ("--user", &self.sender.user),
            ("--hostname", &self.sender.hostname),
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
            && self.sender.nick.is_none()
            && self.sender.user.is_none()
            && self.sender.hostname.is_none()
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
        let trimmed = |value: &Option<String>| value.as_deref().map(str::trim).map(str::to_owned);
        let nickname = trimmed(&self.sender.nick);
        let username = trimmed(&self.sender.user);
        let hostname = trimmed(&self.sender.hostname);

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
/// single unset field, so a filter without either matches every URL. Repeated identical values
/// are deduplicated — they would only create identical filters.
fn expand(values: &[String]) -> Vec<Option<String>> {
    let mut unique: Vec<String> = Vec::new();

    for value in values {
        let value = value.trim().to_owned();

        if !unique.contains(&value) {
            unique.push(value);
        }
    }

    if unique.is_empty() {
        vec![None]
    } else {
        unique.into_iter().map(Some).collect()
    }
}

impl From<&CriteriaArgs> for Criteria {
    fn from(args: &CriteriaArgs) -> Self {
        Self {
            channel: args.channel.clone(),
            host: args.host.clone(),
            path: args.path.clone(),
            nickname: args.sender.nick.clone(),
            username: args.sender.user.clone(),
            hostname: args.sender.hostname.clone(),
        }
    }
}

impl List {
    /// Returns the listing criteria.
    fn criteria(&self) -> Criteria {
        Criteria::from(&self.criteria)
    }
}

impl Delete {
    /// Returns the deletion criteria, or `None` if no criterion was given.
    fn criteria(&self) -> Option<Criteria> {
        let criteria = Criteria::from(&self.criteria);

        (!criteria.is_empty()).then_some(criteria)
    }
}

/// The complete labeled confirmation for the added or removed `filters`, describing each one so
/// the sender can act on the ids without a follow-up `.filter list`. Callers send the result
/// as-is.
fn changed_response(name: &str, verb: &str, filters: &[Filter]) -> String {
    match filters {
        [filter] => labeled_reply(name, format!("{verb} {}.", describe(filter))),
        // More than one filter, so the plural is always right.
        filters => listing(
            labeled_reply(name, format!("{verb} {} filters: ", filters.len())),
            filters,
            0,
        ),
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
                client.send_privmsg(channel, labeled_reply("Filter", &error))?;

                return Ok(());
            }
        };

        let new_filters = opts.new_filters(scope.as_deref(), nickname);
        let total = new_filters.len();
        let mut added = Vec::with_capacity(total);

        for new_filter in new_filters {
            match self.service.add(new_filter).await {
                Ok(filter) => added.push(filter),
                Err(error) => {
                    let response = if added.is_empty() {
                        format!("could not add the filter: {error}")
                    } else {
                        format!(
                            "added {} of {total} filters, then could not add the rest: {error}",
                            added.len()
                        )
                    };

                    client.send_privmsg(channel, labeled_reply("Filter", response))?;

                    return Ok(());
                }
            }
        }

        debug!(filter.count = total, "added filters");

        client.send_privmsg(channel, changed_response("Filter", "Added", &added))?;

        Ok(())
    }

    /// Lists the filters matching the `list` subcommand criteria.
    fn list(&self, client: &Client, channel: &str, opts: &List) -> Result<(), ZetaError> {
        let filters = self.service.select(&opts.criteria());

        let response = if filters.is_empty() {
            if self.service.list().is_empty() {
                labeled_reply("Filter", "No filters are configured.")
            } else {
                labeled_reply("Filter", "No filters match your criteria.")
            }
        } else {
            listing(
                labeled_reply(
                    "Filter",
                    format!(
                        "{} matching your criteria: ",
                        pluralize(filters.len(), "filter")
                    ),
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
                    labeled_reply("Filter", "specify either a filter id or criteria, not both"),
                )?;

                return Ok(());
            }

            // The removed filter is named in the reply, and an unknown id is answered without
            // touching the database.
            let Some(filter) = self
                .service
                .list()
                .into_iter()
                .find(|filter| filter.id == id)
            else {
                client.send_privmsg(
                    channel,
                    labeled_reply("Filter", format!("No filter with id {id}.")),
                )?;

                return Ok(());
            };

            let response = match self.service.delete_ids(&[id]).await {
                Ok(1) => format!("Removed {}.", describe(&filter)),
                Ok(_) => format!("No filter with id {id}."),
                Err(error) => format!("could not delete the filter: {error}"),
            };

            client.send_privmsg(channel, labeled_reply("Filter", &response))?;

            return Ok(());
        }

        let Some(criteria) = opts.criteria() else {
            client.send_privmsg(
                channel,
                labeled_reply(
                    "Filter",
                    "specify a filter id, or at least one criterion to match filters by (e.g. \
                     `.filter delete --host example.com`)",
                ),
            )?;

            return Ok(());
        };

        let filters = self.service.select(&criteria);

        if filters.is_empty() {
            client.send_privmsg(
                channel,
                labeled_reply("Filter", "No filters match your criteria."),
            )?;

            return Ok(());
        }

        let ids: Vec<i32> = filters.iter().map(|filter| filter.id).collect();

        if !opts.force {
            const FORCE_SUFFIX: &str = " - use --force to proceed.";

            let mut confirmation = listing(
                labeled_reply(
                    "Filter",
                    format!(
                        "{} matching the criteria will be deleted: ",
                        pluralize(filters.len(), "filter")
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
            Ok(removed) if removed == filters.len() as u64 => {
                changed_response("Filter", "Removed", &filters)
            }
            Ok(removed) => labeled_reply(
                "Filter",
                format!("Removed {removed} of {} matching filters.", filters.len()),
            ),
            Err(error) => labeled_reply("Filter", format!("could not delete the filters: {error}")),
        };

        client.send_privmsg(channel, response)?;

        Ok(())
    }
}

#[plugin(name = "filter")]
impl FilterPlugin {
    fn new(ctx: &Context, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.command(FILTER);
        let service = Arc::new(FilterService::new(ctx.db.clone()));

        // The host resolves this handle once, when it starts the runtime, so it must be
        // published here rather than from the `loaded` hook: a later publication never reaches
        // the dispatcher. Filter changes go through the service's own index instead.
        ctx.shared.publish(Arc::new(Filters::new(service.clone())));

        let admins = Hostmasks::compile(&ctx.config.admin_hostmasks, "manage filters");

        Ok(Self { service, admins })
    }

    #[handler(loaded)]
    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        self.service.load().await.map_err(plugin_err)?;

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

        if !self.is_admin(sender) {
            client.send_privmsg(channel, labeled_reply("Filter", UNAUTHORIZED))?;

            return Ok(());
        }

        let Some(opts) = parse_or_usage::<Opts>(client, command, ArgsMode::Words, |line| {
            labeled_reply("Filter", line)
        })?
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
/// suffix of `reserved` bytes. A listing that does not fit entirely gains a ` (+N more)`
/// suffix naming how many filters were left out.
fn listing(header: String, filters: &[Filter], reserved: usize) -> String {
    let mut listing = header;

    append_entries_with_more(
        &mut listing,
        filters.iter().map(describe),
        filters.len(),
        ", ",
        MAX_LISTING_LENGTH,
        reserved,
        |dropped| format!(" (+{dropped} more)"),
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
    use clap::CommandFactory;
    use zeta_utils::irc::strip_formatting;

    #[test]
    fn cli_definition_is_valid() {
        // clap reports contradictory definitions through asserts; evaluating them here turns a
        // bad definition into a test failure instead of a panic in debug builds.
        Opts::command().debug_assert();
    }

    #[tokio::test]
    async fn publishes_a_url_filters_handle_for_the_host() {
        // The host's dispatcher consults the `Filters` facade, which only sees the service
        // through the type-erased handle published here — without it, no URL is ever filtered.
        let ctx = zeta_test_support::test_context();
        let mut subscriptions = Subscriptions::new();

        <FilterPlugin as Registered>::new(&ctx, &NoSettings {}, &mut subscriptions).unwrap();

        assert!(ctx.shared.get::<Filters>().is_some());
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
        assert_eq!(add.sender.user.as_deref(), Some("other"));
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
    fn describes_added_filters() {
        let first = Filter {
            id: 12,
            channel: None,
            host: Some("a.com".into()),
            path: Some("/x/*".into()),
            nickname: None,
            username: None,
            hostname: None,
        };

        let cases = [
            (
                "a single filter",
                vec![Filter {
                    id: 12,
                    channel: Some("#foo".into()),
                    host: Some("example.com".into()),
                    ..first.clone()
                }],
                "> Filter: Added #12: host=example.com path=/x/* (#foo).",
            ),
            (
                "multiple filters",
                vec![
                    first.clone(),
                    Filter {
                        id: 13,
                        host: Some("b.com".into()),
                        ..first
                    },
                ],
                "> Filter: Added 2 filters: #12: host=a.com path=/x/* (all channels), \
                 #13: host=b.com path=/x/* (all channels)",
            ),
        ];

        for (case, filters, expected) in cases {
            assert_eq!(
                strip_formatting(&changed_response("Filter", "Added", &filters)),
                expected,
                "for {case}"
            );
        }
    }

    #[test]
    fn counts_are_pluralized() {
        assert_eq!(pluralize(1, "filter"), "1 filter");
        assert_eq!(pluralize(2, "filter"), "2 filters");
        assert_eq!(pluralize(0, "filter"), "0 filters");
    }

    #[test]
    fn add_rejects_invalid_arguments() {
        let cases = [
            (
                "a channel without a prefix",
                "add --channel foo --host example.com",
                "did you mean #foo?",
            ),
            (
                "no criterion",
                "add",
                "at least one of --host, --path, --nick, --user or --hostname",
            ),
            (
                "channel and all-channels together",
                "add --channel #foo --all-channels --host example.com",
                "not both",
            ),
            (
                "a wildcard channel",
                "add --channel #foo* --host example.com",
                "does not support wildcards",
            ),
        ];

        for (case, input, expected) in cases {
            let error = parse_add(input).validate("#chan").unwrap_err();

            assert!(error.contains(expected), "for {case}: {error}");
        }
    }

    #[test]
    fn repeated_identical_patterns_are_deduplicated() {
        let add = parse_add("add --host a.com --host a.com --host b.com");

        let filters = add.new_filters(None, "smoke");

        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].host.as_deref(), Some("a.com"));
        assert_eq!(filters[1].host.as_deref(), Some("b.com"));
    }

    #[test]
    fn a_truncated_listing_names_the_dropped_filters() {
        let long = Filter {
            id: 1,
            channel: None,
            host: Some(format!("long.example/{}", "x".repeat(180))),
            path: None,
            nickname: None,
            username: None,
            hostname: None,
        };
        let second = Filter {
            id: 2,
            ..long.clone()
        };

        let message = listing(labeled_reply("Filter", "2 filters: "), &[long, second], 0);

        assert!(message.len() <= MAX_LISTING_LENGTH);
        assert!(message.contains("#1: "), "{message}");
        assert!(message.ends_with(" (+1 more)"), "{message}");
    }

    #[test]
    fn add_defaults_to_the_current_channel() {
        let add = parse_add("add --host example.com");

        assert_eq!(add.validate("#chan").unwrap().as_deref(), Some("#chan"));
    }

    #[test]
    fn parses_delete_arguments() {
        let by_id = parse_delete("delete 12 --force");

        assert_eq!(by_id.id, Some(12));
        assert!(by_id.force);
        assert!(by_id.criteria().is_none());

        let by_criteria = parse_delete("delete --host *.com");

        assert_eq!(by_criteria.id, None);
        assert_eq!(
            by_criteria.criteria().unwrap().host.as_deref(),
            Some("*.com")
        );
    }

    #[test]
    fn repeated_switches_parse_in_subcommands() {
        let add = parse_add("add --all-channels --all-channels --host example.com");
        assert!(add.all_channels);

        let delete = parse_delete("delete 12 --force --force");
        assert!(delete.force);
    }

    #[test]
    fn list_criteria_convert() {
        let opts: Opts = FILTER
            .parse_words("list --host *.com")
            .expect("`list --host *.com` should parse");

        let Subcommand::List(list) = opts.command else {
            panic!("expected the list subcommand");
        };

        let criteria = list.criteria();

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
}
