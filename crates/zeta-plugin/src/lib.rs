//! The plugin API for the zeta IRC bot.
//!
//! Plugins implement the [`Plugin`] trait: they register the events they want to receive in
//! [`Plugin::new`] through a [`Subscriptions`] set and handle them by overloading the
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
mod erased;
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

pub use argh;
pub use async_trait;
pub use catalog::{CatalogEntry, PluginCatalog};
pub use command::{ArgsError, CommandSpec};
pub use context::{Context, SharedState};
pub use erased::ErasedPlugin;
pub use error::Error;
pub use event::{
    CommandEvent, CtcpEvent, CtcpKind, Event, EventKind, JoinEvent, KickEvent, MessageEvent,
    NickEvent, PartEvent, QuitEvent, RawEvent, Sender, Subscriptions, UrlEvent, UrlScope,
};
pub use filtering::{Filters, UrlFilter};
pub use hostmask::Hostmasks;
pub use inventory;
pub use plugin::Plugin;
pub use registration::{LoadedPlugin, PluginRegistration};
#[cfg(feature = "database")]
pub use sqlx;
pub use types::NoSettings;
pub use zeta_plugin_macros::plugin;

// Re-exported through the crate root so the code the `#[plugin]` macro generates — and plugin
// crates generally — can reference it as absolute `::zeta_plugin::…` paths without adding
// dependencies of their own.
pub use self::irc::{parse_args_or_usage, parse_words_or_usage, reply};
pub use ::irc::client::Client;

/// Common includes used in plugins.
pub mod prelude {
    pub use async_trait::async_trait;

    pub use super::catalog::{CatalogEntry, PluginCatalog};
    pub use super::command::{ArgsError, CommandSpec};
    pub use super::error::{BoxError, Error as ZetaError, plugin_err, resolve_secret};
    pub use super::event::{
        CommandEvent, CtcpEvent, CtcpKind, Event, JoinEvent, KickEvent, MessageEvent, NickEvent,
        PartEvent, QuitEvent, RawEvent, Sender, Subscriptions, UrlEvent, UrlScope,
    };
    pub use super::filtering::{Filters, UrlFilter};
    pub use super::hostmask::Hostmasks;
    pub use super::irc::{
        BOLD, COLOR, NOTICE_PREFIX, REPLY_PREFIX, RESET, notice, parse_args_or_usage,
        parse_words_or_usage, reply, reply_prefix, sanitize_external,
    };
    pub use super::types::NoSettings;
    pub use super::{Context, Error, Plugin};
    pub use ::irc::client::Client;
    pub use zeta_plugin_macros::plugin;
}
