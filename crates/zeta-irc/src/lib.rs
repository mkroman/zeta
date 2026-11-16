//! IRC reply formatting, addressing, and line budgets for the zeta IRC bot.
//!
//! Plugin replies follow a common visual convention: a teal `>` marker (mIRC color 10), an
//! optional bold label naming the source, and the message body with values emphasized by
//! switching between the reply color and the default color. A reply with a label is a labeled
//! reply; one without is a plain reply. The helpers here produce those byte sequences so the
//! exact escape codes are written down in one place.
//!
//! The crate is split by concern: `reply` builds the formatted reply lines, `address` unwraps
//! a nickname mention from the start of a message, `formatting` removes formatting codes and
//! control characters from text that came from elsewhere, and `budget` fills a listing up to
//! the reply line limit. Every public item is re-exported here, so `zeta_irc::{…}` is the
//! import path for all of them; plugins reach them through `zeta_plugin::irc::{…}` instead of
//! depending on this crate.

mod address;
mod budget;
mod formatting;
mod reply;

pub use address::strip_nick_prefix;
pub use budget::{MAX_LISTING_LENGTH, append_entries_with_more, append_entries_within_budget};
pub use formatting::{strip_control_chars, strip_formatting, unstyle_text};
pub use reply::{
    BOLD, COLOR, LABELED_PREFIX_BYTES, PLAIN_REPLY_PREFIX, REPLY_MARKER, RESET, labeled_reply,
    labeled_reply_prefix, plain_reply, usage_line, write_labeled_reply_prefix,
};
