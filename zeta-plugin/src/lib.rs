//! Plugin and task management.

mod error;
mod plugin;
mod types;

pub use error::{BoxError, Error};
pub use plugin::Plugin;
pub use types::{Author, Metadata, Name};

pub mod prelude {
    pub use async_trait::async_trait;

    pub use super::{Author, BoxError, Metadata, Name, Plugin};
}
