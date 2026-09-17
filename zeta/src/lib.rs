//! Zeta is an opinionated IRC bot with a bunch of plugins.

#![allow(clippy::use_self)]

/// Single-slot time-to-live caching
#[cfg(feature = "http")]
pub mod cache;
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
/// Duration formatting and parsing utilities
mod duration;
mod error;
#[cfg(feature = "http")]
mod http;
/// Shared media mirroring
#[cfg(feature = "mirror")]
pub mod mirror;
/// OAuth2 client-credentials token caching
#[cfg(feature = "http")]
pub mod oauth;
mod plugin;
/// URL helper utillities
pub mod url;
mod utils;
mod zeta;

pub use config::Config;
pub use error::Error;
pub use plugin::{ErasedPlugin, Plugin, PluginCatalog, PluginInfo, PluginsConfig, Registry};
pub use zeta::Zeta;
