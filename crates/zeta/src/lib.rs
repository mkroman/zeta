//! Zeta is an opinionated IRC bot with a bunch of plugins.
//!
//! The bundled plugins live in the `plugins` crate; the plugin API and the infrastructure they
//! share with this host live in `zeta-plugin`. This crate carries the IRC bot itself: the
//! client loop, event dispatching, configuration, and the plugin task machinery.

/// Configuration loading and validation.
pub mod config;
/// Commonly used constants.
mod consts;
/// Database integration.
#[cfg(feature = "database")]
pub mod database;
mod error;
/// Plugin host support: the registry facade, event dispatching, and plugin tasks.
pub mod plugin;
mod zeta;

pub use config::Config;
pub use error::Error;
pub use zeta::Zeta;
