//! Common includes used in plugins.

#[allow(unused_imports)]
pub use async_trait::async_trait;
#[allow(unused_imports)]
pub use irc::client::Client as IrcClient;

#[allow(unused_imports)]
pub use zeta_plugin::prelude::{
    ArgumentError, BOLD, BoxError, CommandEvent, CommandSpec, CtcpEvent, CtcpKind, Event,
    JoinEvent, KickEvent, MessageEvent, NickEvent, NoSettings, PartEvent, QuitEvent, REPLY_COLOR,
    REPLY_MARKER, RESET, RawEvent, Sender, Subscriptions, UrlEvent, UrlScope, parse_args_or_usage,
    parse_words_or_usage, plain_reply, plugin_err, reply, reply_prefix, reply_usage_lines,
    resolve_secret,
};

#[allow(unused_imports)]
pub use zeta_plugin::{
    Context, Error as PluginError, FilterGate, FilterHandle, FilterPolicy, Plugin, PluginCatalog,
    PluginEntry, SharedState,
};
