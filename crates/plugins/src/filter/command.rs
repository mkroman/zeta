//! The `.filter` command and its argument handling.

use argh::{ArgsInfo, FromArgs};

use super::{
    model::{Criteria, NewFilterRule},
    pattern::has_wildcards,
};
use crate::prelude::*;

/// The `.filter` command.
pub(super) const FILTER: CommandSpec =
    CommandSpec::with_args::<Options>(".filter", "Manage URL and sender filters (add/list/delete)");

/// Manage URL and sender filters.
#[derive(FromArgs, ArgsInfo, Debug)]
pub(super) struct Options {
    /// the filter operation
    #[argh(subcommand)]
    pub(super) command: Subcommand,
}

/// The `.filter` subcommands.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand)]
pub(super) enum Subcommand {
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
pub(super) struct Add {
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
pub(super) struct List {
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
pub(super) struct Delete {
    /// the id of a single filter to remove
    #[argh(positional)]
    pub(super) id: Option<i32>,
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
    pub(super) force: bool,
}

impl Add {
    /// Validates the arguments and returns one filter per host × path combination.
    ///
    /// Without `--channel` or `--all-channels`, the filters apply to `current_channel`.
    pub(super) fn into_filters(
        self,
        current_channel: &str,
        created_by: &str,
    ) -> Result<Vec<NewFilterRule>, String> {
        let channel = self.channel_scope(current_channel)?;
        let hosts = unique_patterns("--host", &self.host)?;
        let paths = unique_patterns("--path", &self.path)?;
        let nickname = optional_pattern("--nick", self.nick.as_deref())?.map(str::to_owned);
        let username = optional_pattern("--user", self.user.as_deref())?.map(str::to_owned);
        let hostname = optional_pattern("--hostname", self.hostname.as_deref())?.map(str::to_owned);

        if hosts.is_empty()
            && paths.is_empty()
            && nickname.is_none()
            && username.is_none()
            && hostname.is_none()
        {
            return Err(
                "at least one of --host, --path, --nick, --user or --hostname is required"
                    .to_string(),
            );
        }

        let hosts = expand(hosts);
        let paths = expand(paths);
        let mut filters = Vec::with_capacity(hosts.len() * paths.len());

        for host in hosts {
            for path in &paths {
                filters.push(NewFilterRule {
                    channel: channel.clone(),
                    host: host.clone(),
                    path: path.clone(),
                    nickname: nickname.clone(),
                    username: username.clone(),
                    hostname: hostname.clone(),
                    created_by: created_by.to_owned(),
                });
            }
        }

        Ok(filters)
    }

    /// Returns the channel scope of the new filters: `None` for all channels, or the channel to
    /// apply them in.
    fn channel_scope(&self, current_channel: &str) -> Result<Option<String>, String> {
        if self.all_channels && self.channel.is_some() {
            return Err("use either --channel or --all-channels, not both".to_string());
        }

        if self.all_channels {
            return Ok(None);
        }

        let Some(channel) = self.channel.as_deref() else {
            return Ok(Some(current_channel.to_owned()));
        };

        if has_wildcards(channel) {
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

        Ok(Some(channel.to_owned()))
    }
}

/// Returns the unique, trimmed values of the repeated `option`, rejecting blank patterns.
fn unique_patterns(option: &str, values: &[String]) -> Result<Vec<String>, String> {
    let mut patterns: Vec<&str> = Vec::with_capacity(values.len());

    for value in values {
        let value = trimmed(option, value)?;

        if !patterns.contains(&value) {
            patterns.push(value);
        }
    }

    Ok(patterns.into_iter().map(str::to_owned).collect())
}

/// Turns the given URL patterns into filter fields: no patterns yield the single unset field,
/// so a filter without either URL criterion matches every URL. Repeated identical values are
/// deduplicated by [`unique_patterns`] — they would only create identical filters.
fn expand(patterns: Vec<String>) -> Vec<Option<String>> {
    if patterns.is_empty() {
        vec![None]
    } else {
        patterns.into_iter().map(Some).collect()
    }
}

/// Returns the optional `option` value trimmed, rejecting a blank pattern.
fn optional_pattern<'a>(option: &str, pattern: Option<&'a str>) -> Result<Option<&'a str>, String> {
    pattern.map(|pattern| trimmed(option, pattern)).transpose()
}

/// Returns `pattern` trimmed, rejecting a blank pattern with an error naming `option`.
fn trimmed<'a>(option: &str, pattern: &'a str) -> Result<&'a str, String> {
    let pattern = pattern.trim();

    if pattern.is_empty() {
        Err(format!("{option} patterns cannot be empty"))
    } else {
        Ok(pattern)
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
    pub(super) fn criteria(&self) -> Option<Criteria> {
        let criteria = Criteria::from(self);

        (!criteria.is_empty()).then_some(criteria)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `input` with the filter command spec and unpacks the `add` subcommand.
    fn parse_add(input: &str) -> Add {
        let opts: Options = FILTER
            .parse_words(input)
            .unwrap_or_else(|error| panic!("`{input}` should parse: {error}"));

        let Subcommand::Add(add) = opts.command else {
            panic!("`{input}` should be the add subcommand");
        };

        add
    }

    /// Parses `input` with the filter command spec and unpacks the `delete` subcommand.
    fn parse_delete(input: &str) -> Delete {
        let opts: Options = FILTER
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
        let filters = parse_add("add --host a.com --host b.com --path /x/*")
            .into_filters("#chan", "smoke")
            .unwrap();

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
        let filters = parse_add("add --user *other --all-channels")
            .into_filters("#chan", "smoke")
            .unwrap();

        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].host, None);
        assert_eq!(filters[0].channel, None);
        assert_eq!(filters[0].username.as_deref(), Some("*other"));
    }

    #[test]
    fn add_rejects_channels_without_a_channel_prefix() {
        let error = parse_add("add --channel foo --host example.com")
            .into_filters("#chan", "smoke")
            .unwrap_err();

        assert!(error.contains("did you mean #foo?"), "{error}");
    }

    #[test]
    fn repeated_identical_patterns_are_deduplicated() {
        let filters = parse_add("add --host a.com --host a.com --host b.com")
            .into_filters("#chan", "smoke")
            .unwrap();

        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].host.as_deref(), Some("a.com"));
        assert_eq!(filters[1].host.as_deref(), Some("b.com"));
    }

    #[test]
    fn add_requires_a_criterion() {
        let error = parse_add("add").into_filters("#chan", "smoke").unwrap_err();

        assert!(error.contains("at least one of"), "{error}");
    }

    #[test]
    fn add_rejects_blank_patterns() {
        let error = unique_patterns("--host", &["  ".to_string()]).unwrap_err();

        assert_eq!(error, "--host patterns cannot be empty");
    }

    #[test]
    fn add_rejects_channel_and_all_channels_together() {
        let error = parse_add("add --channel #foo --all-channels --host example.com")
            .into_filters("#chan", "smoke")
            .unwrap_err();

        assert!(error.contains("not both"), "{error}");
    }

    #[test]
    fn add_rejects_wildcard_channels() {
        let error = parse_add("add --channel #foo* --host example.com")
            .into_filters("#chan", "smoke")
            .unwrap_err();

        assert!(error.contains("does not support wildcards"), "{error}");
    }

    #[test]
    fn add_defaults_to_the_current_channel() {
        let filters = parse_add("add --host example.com")
            .into_filters("#chan", "smoke")
            .unwrap();

        assert_eq!(filters[0].channel.as_deref(), Some("#chan"));
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
        let opts: Options = FILTER
            .parse_words("list --host *.com")
            .expect("`list --host *.com` should parse");

        let Subcommand::List(list) = opts.command else {
            panic!("expected the list subcommand");
        };

        let criteria = Criteria::from(&list);

        assert_eq!(criteria.host.as_deref(), Some("*.com"));
        assert!(criteria.channel.is_none());
    }
}
