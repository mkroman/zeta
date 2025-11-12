//! Plugin and task management.

mod error;
mod plugin;
mod types;

pub use error::Error;
pub use plugin::Plugin;
pub use types::{Author, Name, Version};
