//! Plugin and task management.

mod error;
mod plugin;
mod types;

pub use error::Error;
pub use plugin::Plugin;
pub use types::{Author, Metadata, Name};

pub mod prelude {
    pub use async_trait::async_trait;

    pub use super::error::{BoxError, plugin_err, plugin_err_display};
    pub use super::{Author, Error, Metadata, Name, Plugin};
}
