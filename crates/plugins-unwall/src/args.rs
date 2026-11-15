//! The `.unwall` command line and its clap definition.

use clap::Parser;

use zeta_plugin::CommandSpec;

/// The `.unwall` command.
pub const UNWALL: CommandSpec = CommandSpec::with_args::<Opts>(
    ".unwall",
    "Manage the paywalled sites unwalled through unwall.app (add/rm/list/stats/info)",
);

/// Manage the paywalled sites unwalled through unwall.app.
#[derive(Parser, Debug)]
pub struct Opts {
    /// the unwall operation
    #[command(subcommand)]
    pub(super) command: Subcommand,
}

/// The `.unwall` subcommands.
#[derive(clap::Subcommand, Debug)]
pub enum Subcommand {
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
pub struct Add {
    /// the hostnames to unwall, e.g. `bloomberg.com` or `*.bloomberg.com`
    #[arg(num_args = 1.., trailing_var_arg = true)]
    pub(super) hosts: Vec<String>,
}

/// Remove sites matching a pattern.
#[derive(clap::Args, Debug)]
pub struct Remove {
    /// the hostname or wildcard to remove, e.g. `www.bloomberg.com` or `*.bloomberg.com`
    pub(super) pattern: String,
}

/// Show statistics for one host.
#[derive(clap::Args, Debug)]
pub struct Info {
    /// the host to show statistics for
    pub(super) host: String,
}
