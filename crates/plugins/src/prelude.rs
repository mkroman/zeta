//! Common includes used in plugins.

#[allow(unused_imports)]
pub use async_trait::async_trait;
#[allow(unused_imports)]
pub use irc::client::Client;

#[allow(unused_imports)]
pub use zeta_plugin::prelude::{
    ArgsError, BOLD, BoxError, COLOR, CommandEvent, CommandSpec, CtcpEvent, CtcpKind, Event,
    JoinEvent, KickEvent, MessageEvent, NickEvent, NoSettings, PartEvent, QuitEvent, REPLY_PREFIX,
    RESET, RawEvent, Sender, Subscriptions, UrlEvent, UrlScope, notice, parse_args_or_usage,
    parse_words_or_usage, plugin_err, reply, reply_prefix, reply_usage_lines, resolve_secret,
};

#[allow(unused_imports)]
pub use zeta_plugin::{
    CatalogEntry, Context, Error as ZetaError, Filters, Plugin, PluginCatalog, SharedState,
    UrlFilter, UrlFilters,
};
