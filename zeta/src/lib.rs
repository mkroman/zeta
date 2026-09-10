//! Zeta is an opinionated IRC bot with a bunch of plugins.

#![allow(clippy::use_self)]

/// Configuration loading and validation
pub mod config;
/// Commonly used constants
pub mod consts;
/// Shared context for plugins
pub mod context;
/// Database integration
#[cfg(feature = "database")]
pub mod database;
/// DNS resolution
pub mod dns;
mod error;
#[cfg(feature = "http")]
mod http;
mod plugin;
/// URL helper utillities
pub mod url;
mod utils;
mod zeta;

pub use config::Config;
pub use error::Error;
pub use plugin::{Plugin, PluginCatalog, PluginInfo, Registry};
pub use zeta::Zeta;
