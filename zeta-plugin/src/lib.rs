//! Plugin and task management.

pub mod command;
pub mod irc;

mod error;
mod plugin;
mod types;

pub use argh;
pub use command::{ArgsError, PluginCommand, Prefix};
pub use error::Error;
pub use plugin::{Plugin, PluginName};
pub use types::{Author, Metadata, Name, NoSettings};

pub mod prelude {
    pub use async_trait::async_trait;

    pub use super::command::{ArgsError, PluginCommand, Prefix};
    pub use super::error::{BoxError, plugin_err, require_env, resolve_secret};
    pub use super::irc::{BOLD, COLOR, REPLY_PREFIX, RESET, notice, reply, reply_prefix, reply_usage_lines};
    pub use super::plugin::PluginName;
    pub use super::types::NoSettings;
    pub use super::{Author, Error, Metadata, Name, Plugin};
}
