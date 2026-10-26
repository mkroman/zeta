//! Zeta is an opinionated IRC bot with a bunch of plugins.
//!
//! The bundled plugins live in the `plugins` crate; their API lives in `zeta-plugin`, and the
//! registry, plugin tasks, and event dispatch that run them live in `zeta-plugin-runtime`. This
//! crate carries the IRC bot itself: the client loop and configuration.

/// Configuration loading and validation.
pub mod config;
/// Commonly used constants.
mod consts;
/// Database integration.
#[cfg(feature = "database")]
pub mod database;
mod error;
mod zeta;

pub use config::Config;
pub use error::Error;
pub use zeta::Zeta;
