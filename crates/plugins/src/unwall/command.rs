//! The `.unwall` command and its argument handling.

use argh::{ArgsInfo, FromArgs};

use crate::prelude::*;

/// The `.unwall` command.
pub(super) const UNWALL: CommandSpec = CommandSpec::with_args::<Opts>(
    ".unwall",
    "Manage the paywalled sites unwalled through unwall.app (add/rm/list/stats/info)",
);

/// Manage the paywalled sites unwalled through unwall.app.
#[derive(FromArgs, ArgsInfo, Debug)]
pub(super) struct Opts {
    /// the unwall operation
    #[argh(subcommand)]
    pub(super) command: Subcommand,
}

/// The `.unwall` subcommands.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand)]
pub(super) enum Subcommand {
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
pub(super) struct Add {
    /// the hostnames to unwall, e.g. `bloomberg.com` or `*.bloomberg.com`
    #[argh(positional, greedy)]
    pub(super) hosts: Vec<String>,
}

/// Remove sites matching a pattern.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "rm")]
pub(super) struct Rm {
    /// the hostname or wildcard to remove, e.g. `www.bloomberg.com` or `*.bloomberg.com`
    #[argh(positional)]
    pub(super) pattern: String,
}

/// Remove sites matching a pattern.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "del")]
pub(super) struct Del {
    /// the hostname or wildcard to remove, e.g. `www.bloomberg.com` or `*.bloomberg.com`
    #[argh(positional)]
    pub(super) pattern: String,
}

/// Remove sites matching a pattern.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "delete")]
pub(super) struct Delete {
    /// the hostname or wildcard to remove, e.g. `www.bloomberg.com` or `*.bloomberg.com`
    #[argh(positional)]
    pub(super) pattern: String,
}

/// Summarize the covered sites.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "list")]
pub(super) struct List {}

/// Show overall statistics about the unwalled links.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "stats")]
pub(super) struct Stats {}

/// Show statistics for one host.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(subcommand, name = "info")]
pub(super) struct Info {
    /// the host to show statistics for
    #[argh(positional)]
    pub(super) host: String,
}
