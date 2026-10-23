//! The plugin API for the zeta IRC bot.
//!
//! Plugins implement the [`Plugin`] trait: they register the events they want to receive in
//! [`Plugin::new`] through a [`Subscriptions`] set and handle them by overloading the
//! corresponding per-kind handler methods. See the [`event`] module for the event vocabulary.
//!
//! Alongside the plugin vocabulary, this crate carries the machinery shared by the host and the
//! bundled plugins: the plugin [`Context`], the plugin-facing configuration, the [`Registry`],
//! the shared URL [`Filters`], and the database helpers plugins persist their state through.
//! Further shared infrastructure lives in dedicated crates: `zeta-cache`, `zeta-http`,
//! `zeta-mirror` and `zeta-utils`.

pub mod command;
pub mod event;
pub mod irc;

mod error;
mod plugin;
mod registration;
mod types;

pub mod config;
pub mod context;
pub mod filtering;
pub mod registry;

pub use argh;
pub use async_trait;
pub use command::{ArgsError, CommandSpec};
pub use context::{Context, SharedState};
pub use error::Error;
pub use event::{
    CommandEvent, CtcpEvent, CtcpKind, Event, EventKind, JoinEvent, KickEvent, MessageEvent,
    NickEvent, PartEvent, QuitEvent, RawEvent, Sender, Subscriptions, UrlEvent, UrlScope,
};
pub use filtering::{Filters, UrlFilter, UrlFilters};
pub use inventory;
pub use plugin::Plugin;
pub use registration::{ConstructedPlugin, PluginRegistration};
pub use registry::{
    CatalogEntry, ErasedPlugin, PluginCatalog, PluginTask, RegisteredPlugin, Registry,
};
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

    pub use super::command::{ArgsError, CommandSpec};
    pub use super::error::{BoxError, plugin_err, resolve_secret};
    pub use super::event::{
        CommandEvent, CtcpEvent, CtcpKind, Event, JoinEvent, KickEvent, MessageEvent, NickEvent,
        PartEvent, QuitEvent, RawEvent, Sender, Subscriptions, UrlEvent, UrlScope,
    };
    pub use super::irc::{
        BOLD, COLOR, NOTICE_PREFIX, REPLY_PREFIX, RESET, notice, parse_args_or_usage,
        parse_words_or_usage, reply, reply_prefix, reply_usage_lines,
    };
    pub use super::types::NoSettings;
    pub use super::{Context, Error, Plugin};
    pub use zeta_plugin_macros::plugin;
}
