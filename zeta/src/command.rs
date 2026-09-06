//! IRC prefix command matching and argument parsing.
//!
//! The command types live in [`zeta_plugin`] so that the [`zeta_plugin::Plugin`] trait can use
//! them; they are re-exported here for convenience.

pub use zeta_plugin::command::{ArgsError, Prefix};
