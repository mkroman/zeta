//! The plugin API for the zeta IRC bot.
//!
//! Plugins implement the [`Plugin`] trait: they register the events they want to receive in
//! [`Registered::new`] through a [`Subscriptions`] set and handle them in a single
//! [`Plugin::handle`] that matches the [`Event`]. See the [`event`] module for the event
//! vocabulary.
//!
//! The host keeps the compiled-in plugins as one statically known value each —
//! [`PluginSet`] implemented by the facade's generated enum — and awaits each handler future
//! inline, so no plugin or handler future is boxed by the dispatch path.
//!
//! Alongside the plugin vocabulary, this crate carries the machinery shared by the host and the
//! bundled plugins: the plugin [`Context`], the plugin-facing configuration, the shared event
//! [`FilterHandle`], the admin [`Hostmasks`] matcher, and the database helpers plugins persist
//! their state through. The registry, plugin tasks, and event dispatch live in
//! `zeta-plugin-runtime`. Further shared infrastructure lives in dedicated crates: `zeta-cache`,
//! `zeta-http`, `zeta-mirror` and `zeta-utils`.

pub mod command;
pub mod event;
pub mod irc;

mod catalog;
mod error;
mod loaded;
mod plugin;
mod types;

pub mod config;
pub mod context;
#[cfg(feature = "database")]
pub mod database;
pub mod filtering;
pub mod hostmask;

pub use catalog::{CatalogEntry, PluginCatalog};
pub use command::{ArgsError, ArgsMode, CommandSpec};
pub use config::HostConfig;
pub use context::{Context, SharedState};
pub use error::Error;
pub use error::Error as ZetaError;
pub use error::{BoxError, plugin_err, resolve_secret};
pub use event::{
    CommandEvent, CtcpEvent, CtcpKind, Event, EventKind, JoinEvent, KickEvent, MessageEvent,
    MessagingOperation, NickEvent, PartEvent, QuitEvent, RawEvent, Sender, Subscriptions, UrlEvent,
    UrlScope,
};
pub use filtering::{EventFilter, FilterHandle};
pub use hostmask::Hostmasks;
#[doc(hidden)]
pub use loaded::validate_settings;
pub use loaded::{LoadedPlugin, PluginSet};
pub use plugin::{Plugin, Registered};
#[cfg(feature = "database")]
pub use sqlx;
pub use types::NoSettings;
pub use zeta_plugin_macros::{instrument_database, plugin};

// Re-exported through the crate root so the code the `#[plugin]` macro generates — and plugin
// crates generally — can reference it as absolute `::zeta_plugin::…` paths without adding
// dependencies of their own.
pub use self::irc::{
    labeled_reply, parse_or_usage, plain_reply, reply_lookup, send_reply, send_reply_via,
    usage_line, write_labeled_reply_prefix,
};
pub use ::irc::client::Client;
