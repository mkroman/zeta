//! The plugin API for the zeta IRC bot.
//!
//! Plugins implement the [`Plugin`] trait: they register the events they want to receive in
//! [`Registered::new`] through a [`Subscriptions`] set and handle them by overloading the
//! corresponding per-kind handler methods. See the [`event`] module for the event vocabulary.
//!
//! Alongside the plugin vocabulary, this crate carries the machinery shared by the host and the
//! bundled plugins: the plugin [`Context`], the plugin-facing configuration, the shared URL
//! [`Filters`], the admin [`Hostmasks`] matcher, and the database helpers plugins persist their
//! state through. The registry, plugin tasks, and event dispatch live in `zeta-plugin-runtime`.
//! Further shared infrastructure lives in dedicated crates: `zeta-cache`, `zeta-http`,
//! `zeta-mirror` and `zeta-utils`.

pub mod command;
pub mod event;
pub mod irc;

mod catalog;
mod error;
mod plugin;
mod registration;
mod types;

pub mod config;
pub mod context;
#[cfg(feature = "database")]
pub mod database;
pub mod filtering;
pub mod hostmask;

pub use async_trait;
pub use catalog::{CatalogEntry, PluginCatalog};
pub use clap;
pub use command::{ArgsError, ArgsMode, CommandSpec};
pub use context::{Context, SharedState};
pub use error::Error;
pub use event::{
    CommandEvent, CtcpEvent, CtcpKind, Event, EventKind, JoinEvent, KickEvent, MessageEvent,
    NickEvent, PartEvent, QuitEvent, RawEvent, Sender, Subscriptions, UrlEvent, UrlScope,
};
pub use filtering::{Filters, UrlFilter};
pub use hostmask::Hostmasks;
pub use inventory;
pub use plugin::{Plugin, Registered, handle_event};
pub use registration::{LoadedPlugin, PluginRegistration};
#[cfg(feature = "database")]
pub use sqlx;
pub use types::NoSettings;
pub use zeta_plugin_macros::plugin;

// Re-exported through the crate root so the code the `#[plugin]` macro generates — and plugin
// crates generally — can reference it as absolute `::zeta_plugin::…` paths without adding
// dependencies of their own.
pub use self::irc::{labeled_reply, parse_or_usage, plain_reply, reply_lookup};
pub use ::irc::client::Client;

/// Common includes used in plugins.
pub mod prelude {
    pub use async_trait::async_trait;

    // The crate's own `irc` module cannot be glob-imported here: its name collides with the
    // external `irc` crate that plugins also import. Everything else the public API exports is
    // listed, including the items used less often, so the prelude does not silently omit them.
    pub use crate::catalog::{CatalogEntry, PluginCatalog};
    pub use crate::command::{ArgsError, ArgsMode, CommandSpec};
    pub use crate::error::{BoxError, Error as ZetaError, plugin_err, resolve_secret};
    pub use crate::event::{
        CommandEvent, CtcpEvent, CtcpKind, Event, EventKind, JoinEvent, KickEvent, MessageEvent,
        NickEvent, PartEvent, QuitEvent, RawEvent, Sender, Subscriptions, UrlEvent, UrlScope,
    };
    pub use crate::filtering::{Filters, UrlFilter};
    pub use crate::hostmask::Hostmasks;
    pub use crate::irc::{
        BOLD, COLOR, PLAIN_REPLY_PREFIX, REPLY_MARKER, RESET, labeled_reply, labeled_reply_prefix,
        parse_or_usage, plain_reply, reply_lookup, unstyle_text,
    };
    pub use crate::registration::{LoadedPlugin, PluginRegistration};
    pub use crate::types::NoSettings;
    pub use crate::{Context, Error, Plugin, Registered, SharedState};
    pub use ::irc::client::Client;
    pub use zeta_plugin_macros::plugin;
}
