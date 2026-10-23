//! Common includes used in plugins.

#[allow(unused_imports)]
pub use irc::client::Client;

#[allow(unused_imports)]
pub use zeta_plugin::prelude::{
    BOLD, BoxError, COLOR, CommandEvent, CommandSpec, MessageEvent, NOTICE_PREFIX, NoSettings,
    REPLY_PREFIX, RESET, Sender, Subscriptions, UrlEvent, notice, parse_args_or_usage,
    parse_words_or_usage, plugin, plugin_err, reply, reply_prefix, resolve_secret,
};

#[allow(unused_imports)]
pub use zeta_plugin::{
    CatalogEntry, Context, Error as ZetaError, Filters, Plugin, PluginCatalog, UrlFilters,
};
