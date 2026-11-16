//! The `.filter` command-line arguments and their validation.
//!
//! Holds the `add`, `list` and `delete` subcommand types and the criteria `list` and `delete`
//! share. The `add` validation and [`NewFilter`] construction live here too: they turn the
//! repeated `--host` and `--path` values into one filter per host × path combination, scoped to
//! the current channel unless another scope was requested.

use clap::Parser;

use crate::model::{Criteria, NewFilter, is_wildcard};

/// Manage URL and sender filters.
#[derive(Parser, Debug)]
pub(crate) struct Opts {
    /// the filter operation
    #[command(subcommand)]
    pub(super) command: Subcommand,
}

/// The `.filter` subcommands.
#[derive(clap::Subcommand, Debug)]
pub(crate) enum Subcommand {
    /// add a new filter
    Add(Add),
    /// list filters matching criteria
    List(List),
    /// delete a filter by id, or all filters matching criteria
    Delete(Delete),
}

/// The sender patterns `add`, `list` and `delete` share.
#[derive(clap::Args, Debug, Default)]
pub(crate) struct SenderArgs {
    /// sender nickname pattern, e.g. `*bot`
    #[arg(long)]
    pub(super) nick: Option<String>,
    /// sender username (ident) pattern, e.g. `*other`
    #[arg(long)]
    pub(super) user: Option<String>,
    /// sender hostname pattern, e.g. `*.isp.example`
    #[arg(long)]
    pub(super) hostname: Option<String>,
}

/// The filter selection criteria `list` and `delete` share.
#[derive(clap::Args, Debug, Default)]
pub struct CriteriaArgs {
    /// the channel pattern to match
    #[arg(long)]
    pub(super) channel: Option<String>,
    /// the URL host pattern to match
    #[arg(long)]
    pub(super) host: Option<String>,
    /// the URL path pattern to match
    #[arg(long)]
    pub(super) path: Option<String>,
    /// the sender patterns to match
    #[command(flatten)]
    pub(super) sender: SenderArgs,
}

/// Add a new filter.
#[derive(clap::Args, Debug)]
pub(crate) struct Add {
    /// only apply the filter in this channel (defaults to the current channel)
    #[arg(long)]
    pub(super) channel: Option<String>,
    /// apply the filter in all channels
    #[arg(long)]
    pub(super) all_channels: bool,
    /// URL host pattern to filter, e.g. `imdb.com` or `*.com` (repeatable)
    #[arg(long)]
    pub(super) host: Vec<String>,
    /// URL path pattern to filter, e.g. `/title/*` (repeatable)
    #[arg(long)]
    pub(super) path: Vec<String>,
    /// the sender patterns to filter
    #[command(flatten)]
    pub(super) sender: SenderArgs,
}

/// List filters.
#[derive(clap::Args, Debug)]
pub(crate) struct List {
    /// the criteria to match
    #[command(flatten)]
    pub(super) criteria: CriteriaArgs,
}

/// Delete filters.
#[derive(clap::Args, Debug)]
pub(crate) struct Delete {
    /// the id of a single filter to remove
    pub(super) id: Option<i32>,
    /// the criteria to match
    #[command(flatten)]
    pub(super) criteria: CriteriaArgs,
    /// delete the matching filters without confirmation
    #[arg(long)]
    pub(super) force: bool,
}

impl Add {
    /// Validates the arguments, returning the channel scope of the new filters: `None` for all
    /// channels, or the channel to apply them in.
    pub(super) fn validate(&self, current_channel: &str) -> Result<Option<String>, String> {
        if self.all_channels && self.channel.is_some() {
            return Err("use either --channel or --all-channels, not both".to_string());
        }

        let channel = if self.all_channels {
            None
        } else if let Some(channel) = self.channel.as_deref() {
            if is_wildcard(channel) {
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
    pub(super) fn new_filters(&self, channel: Option<&str>, created_by: &str) -> Vec<NewFilter> {
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
    pub(super) fn criteria(&self) -> Criteria {
        Criteria::from(&self.criteria)
    }
}

impl Delete {
    /// Returns the deletion criteria, or `None` if no criterion was given.
    pub(super) fn criteria(&self) -> Option<Criteria> {
        let criteria = Criteria::from(&self.criteria);

        (!criteria.is_empty()).then_some(criteria)
    }
}
