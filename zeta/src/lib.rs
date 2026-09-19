//! Zeta is an opinionated IRC bot with a bunch of plugins.

#![allow(clippy::use_self)]

/// Single-slot time-to-live caching
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
/// HTTP client integration.
///
/// Enabled by the `http` feature (plain reqwest) or the `emulated` feature (browser-emulated
/// wreq for anti-bot-protected sites).
#[cfg(any(feature = "http", feature = "emulated"))]
mod http;
/// Shared media mirroring
#[cfg(feature = "mirror")]
pub mod mirror;
/// OAuth2 client-credentials token caching
#[cfg(feature = "http")]
pub mod oauth;
/// Plugin system: registry, event dispatching, and the bundled plugins
pub mod plugin;
/// URL helper utillities
pub mod url;
mod utils;
mod zeta;

pub use config::Config;
pub use error::Error;
pub use plugin::{
    CatalogEntry, ErasedPlugin, Plugin, PluginCatalog, PluginsConfig, RegisteredPlugin, Registry,
};
pub use zeta::Zeta;
