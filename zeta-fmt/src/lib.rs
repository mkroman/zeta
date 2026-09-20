//! IRC message formatting for the zeta IRC bot.
//!
//! Every reply follows one visual convention: a teal `>` marker (mIRC color
//! 10), an optional bold plugin name, cyan scaffolding (labels, separators,
//! parentheses), and plain values that stand out over the scaffolding color.
//! This crate encodes that convention — and the [modern IRC formatting
//! grammar](https://modern.ircdocs.horse/formatting) — in one place, so
//! plugins never hand-write control characters.
//!
//! The entry point is [`Banner`]: a plugin's reply identity, defined once.
//! `banner.reply(sink)` mints a [`Reply`] builder whose operations (`label`,
//! `value`, `field`, ...) track the formatting state and emit each control
//! code only when the state actually changes — redundant and duplicate codes
//! are unrepresentable, not cleaned up afterwards. Interpolated values are
//! stripped of control characters, so user data cannot smuggle formatting
//! into a reply.
//!
//! # Examples
//!
//! ```
//! use zeta_fmt::{Banner, Color};
//!
//! const BANNER: Banner = Banner::new("Dig");
//!
//! // A one-shot reply string.
//! assert_eq!(BANNER.message("3 records"), "\x0310>\x0f\x02 Dig:\x02\x0310 3 records");
//!
//! // A composed reply, rendered into any `fmt::Write` sink.
//! let mut out = String::new();
//! BANNER.reply(&mut out).field("Workers:", 3).field("Tasks:", 5);
//! assert_eq!(out, "\x0310>\x0f\x02 Dig:\x02\x0310 Workers:\x0f 3\x0310 Tasks:\x0f 5");
//!
//! // Values are user data — control characters never reach the reply.
//! let mut out = String::new();
//! Banner::BARE.reply(&mut out).value("hi \x02there\r\nfriend");
//! assert_eq!(out, "\x0310> \x0fhi therefriend");
//!
//! // Arbitrary colors, toggles and combinations are state-tracked too.
//! let mut out = String::new();
//! Banner::BARE.reply(&mut out).fg(Color::Green, "up").sep(" — ").fg(Color::Red, "down");
//! assert_eq!(out, "\x0310> \x0303up\x0310 — \x0304down");
//! ```

pub mod banner;
pub mod color;
pub mod reply;
pub mod style;
pub mod strip;

pub use banner::{Banner, LineDisplay, Rendered};
pub use color::Color;
pub use reply::Reply;
pub use style::Style;

/// The teal color (mIRC color 10) used for plugin replies.
pub const COLOR: &str = "\x0310";

/// Resets bold, color, and any other text formatting back to the default.
pub const RESET: &str = "\x0f";

/// Starts bold text.
pub const BOLD: &str = "\x02";

/// The prefix shared by every reply banner, bold name or not.
pub const REPLY_PREFIX: &str = "\x0310>";

/// Formats `text` as a bare reply without a plugin name: `> <text>`.
#[must_use]
pub fn plain(text: impl fmt::Display) -> String {
    Banner::BARE.message(text)
}

use std::fmt;

/// Common includes for this crate.
pub mod prelude {
    pub use super::banner::Banner;
    pub use super::color::Color;
    pub use super::reply::Reply;
    pub use super::style::Style;
    pub use super::{BOLD, COLOR, RESET, REPLY_PREFIX, plain};
}

#[cfg(test)]
pub(crate) mod harness;
