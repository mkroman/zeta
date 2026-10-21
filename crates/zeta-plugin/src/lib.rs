//! The plugin API for the zeta IRC bot.
//!
//! Plugins implement the [`Plugin`] trait: they register the events they want to receive in
//! [`Plugin::new`] through a [`Subscriptions`] set and handle them by overloading the
//! corresponding per-kind handler methods. See the [`event`] module for the event vocabulary.
//!
//! Alongside the plugin vocabulary, this crate carries the machinery shared by the host and the
//! bundled plugins: the plugin [`Context`], the plugin-facing configuration, the
//! [`PluginRegistry`], the shared URL [`FilterGate`], and the database helpers plugins persist
//! their state through. Further shared infrastructure lives in dedicated crates: `zeta-cache`,
//! `zeta-http`, `zeta-mirror` and `zeta-utils`.

pub mod command;
pub mod event;
pub mod irc;

mod error;
mod plugin;
mod types;

pub mod config;
pub mod context;
pub mod filtering;
pub mod registry;

pub use argh;
pub use command::{ArgumentError, CommandSpec};
pub use context::{Context, SharedState};
pub use error::Error;
pub use event::{
    CommandEvent, CtcpEvent, CtcpKind, Event, EventKind, JoinEvent, KickEvent, MessageEvent,
    NickEvent, PartEvent, QuitEvent, RawEvent, Sender, Subscriptions, UrlEvent, UrlScope,
};
pub use filtering::{FilterGate, FilterHandle, FilterPolicy};
pub use plugin::{NamedPlugin, Plugin};
pub use registry::{
    ErasedPlugin, PluginCatalog, PluginEntry, PluginRegistry, PluginTask, RegisteredPlugin,
};
pub use types::{NoSettings, PluginName};

/// Common includes used in plugins.
pub mod prelude {
    pub use async_trait::async_trait;

    pub use super::command::{ArgumentError, CommandSpec};
    pub use super::error::{BoxError, plugin_err, resolve_secret};
    pub use super::event::{
        CommandEvent, CtcpEvent, CtcpKind, Event, JoinEvent, KickEvent, MessageEvent, NickEvent,
        PartEvent, QuitEvent, RawEvent, Sender, Subscriptions, UrlEvent, UrlScope,
    };
    pub use super::irc::{
        BOLD, REPLY_COLOR, REPLY_MARKER, RESET, parse_args_or_usage, parse_words_or_usage,
        plain_reply, reply, reply_prefix, reply_usage_lines,
    };
    pub use super::plugin::NamedPlugin;
    pub use super::types::{NoSettings, PluginName};
    pub use super::{Error, Plugin};
}
