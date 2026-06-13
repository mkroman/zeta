//! Plugin and task management.

mod error;
mod plugin;
mod types;

pub use error::Error;
pub use plugin::Plugin;
pub use types::{Author, Metadata, Name};

pub mod prelude {
    pub use async_trait::async_trait;

    pub use super::error::{BoxError, plugin_err, require_env};
    pub use super::{Author, Error, Metadata, Name, Plugin};
}

#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UrlFilter {
    /// Match URLs with a given host.
    Host(String),
}

#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventType {
    /// A user joined a channel.
    Join,
    /// A user left a channel.
    Part,
    /// A user sent a message to a channel.
    Message,
    /// A user sent a message to a user.
    PrivateMessage,
}
