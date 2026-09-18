//! The plugin API for the zeta IRC bot.
//!
//! Plugins implement the [`Plugin`] trait: they register the events they want to receive in
//! [`Plugin::new`] through a [`Subscriptions`] set and handle them by overloading the
//! corresponding per-kind handler methods. See the [`event`] module for the event vocabulary.

pub mod command;
pub mod event;
pub mod irc;

mod error;
mod plugin;
mod types;

pub use argh;
pub use command::{ArgsError, CommandSpec};
pub use error::Error;
pub use event::{
    CommandEvent, CtcpEvent, CtcpKind, Event, EventKind, JoinEvent, KickEvent, MessageEvent,
    NickEvent, PartEvent, QuitEvent, RawEvent, Sender, Subscriptions, Tag, UrlEvent, UrlScope,
};
pub use plugin::{Plugin, PluginName};
pub use types::{Author, Metadata, Name, NoSettings};

pub mod prelude {
    pub use async_trait::async_trait;

    pub use super::command::{ArgsError, CommandSpec};
    pub use super::error::{BoxError, plugin_err, require_env, resolve_secret};
    pub use super::event::{
        CommandEvent, CtcpEvent, CtcpKind, Event, JoinEvent, KickEvent, MessageEvent, NickEvent,
        PartEvent, QuitEvent, RawEvent, Sender, Subscriptions, Tag, UrlEvent, UrlScope,
    };
    pub use super::irc::{
        BOLD, COLOR, REPLY_PREFIX, RESET, notice, reply, reply_prefix, reply_usage_lines,
    };
    pub use super::plugin::PluginName;
    pub use super::types::NoSettings;
    pub use super::{Author, Error, Metadata, Name, Plugin};
}
