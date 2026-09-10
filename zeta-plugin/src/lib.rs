//! Plugin and task management.

pub mod command;

mod error;
mod plugin;
mod types;

pub use argh;
pub use command::{ArgsError, PluginCommand, Prefix};
pub use error::Error;
pub use plugin::Plugin;
pub use types::{Author, Metadata, Name};

pub mod prelude {
    pub use async_trait::async_trait;

    pub use super::command::{ArgsError, PluginCommand, Prefix};
    pub use super::error::{BoxError, plugin_err, require_env};
    pub use super::{Author, Error, Metadata, Name, Plugin};
}
