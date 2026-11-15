//! Command specification and argument parsing for IRC bot commands.
//!
//! A [`CommandSpec`] declares a command a plugin serves: its trigger (e.g. `.dig`), a short
//! description, and — for commands with typed arguments — the argument information the host
//! derives usage output from.
//!
//! # Example
//!
//! ```
//! use zeta_plugin::CommandSpec;
//!
//! const YT: CommandSpec = CommandSpec::new(".yt", "Search YouTube");
//!
//! assert_eq!(YT.trailing_args(".yt"), Some(""));
//! assert_eq!(YT.trailing_args(".yt rust"), Some("rust"));
//! assert_eq!(YT.trailing_args(".youtube rust"), None);
//! assert_eq!(YT.trailing_args(".goodbye"), None);
//! ```
//!
//! The module is split by concern: [`CommandSpec`] and [`ArgsMode`] describe a command, `args`
//! holds [`ArgsError`], `parse` the parsing methods, and `cache` the process-global cache of
//! configured `clap` commands that the parses reuse.

use clap::{Command, Parser};

mod args;
mod cache;
mod parse;
#[cfg(test)]
mod tests;

pub use args::ArgsError;

/// How a command's trailing arguments are split into tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgsMode {
    /// Tokenize like a POSIX shell, supporting quotes and escapes.
    Shell,
    /// Split on whitespace only, preserving quotes and escapes verbatim for free-form text.
    Words,
}

/// A command served by a plugin: its trigger, description, and the arguments it accepts.
///
/// Arguments are parsed with [`ArgsMode::Shell`] by default; declare a command's `words` flag
/// to parse free-form text with [`ArgsMode::Words`] instead.
///
/// The `trigger` is the command as users type it — most commands are dot-prefixed (`.yt`), but
/// any single-word prefix (e.g. `!imdb`) works. The description is shown by the host's help
/// command. Commands with typed arguments also store a function pointer to the [`Command`] built
/// by their argument type, so the host can derive usage and argument information for them without
/// having to parse anything.
///
/// Because every field is a `&'static str` or function pointer, `CommandSpec` is [`Copy`],
/// requires no heap allocation, and can be constructed in `const` context. The same constant
/// serves every role: it is registered through [`Subscriptions`](super::Subscriptions) during
/// initialization, and matched against the [`CommandEvent`](super::CommandEvent)'s specification
/// in the command handler.
///
/// The configured `clap` command is built on the first parse and cached, keyed by trigger and
/// argument type, for reuse on later parses; a parse that finds the cached entry busy builds an
/// uncached command instead of blocking. [`CommandSpec::args_info`] always builds its own copy.
///
/// # Matching commands by identity
///
/// Plugins handling multiple commands should declare each command as a constant and dispatch on
/// the identity of the declaration — not on string comparisons against literal triggers. Since
/// `CommandSpec` is a structural-match type, constants can be used directly as `match` patterns:
///
/// ```
/// use zeta_plugin::CommandSpec;
///
/// const BYTES: CommandSpec = CommandSpec::new(".b", "String to bytes");
/// const LENGTH: CommandSpec = CommandSpec::new(".len", "String length");
///
/// fn handle(command: CommandSpec) -> &'static str {
///     match command {
///         BYTES => "string to bytes",
///         LENGTH => "string length",
///         _ => "unhandled",
///     }
/// }
///
/// assert_eq!(handle(BYTES), "string to bytes");
/// assert_eq!(handle(LENGTH), "string length");
/// assert_eq!(handle(CommandSpec::new(".other", "")), "unhandled");
/// ```
///
/// This pattern requires a spec built with [`CommandSpec::new`]: a spec created with
/// [`CommandSpec::with_args`] holds a function pointer, and constants containing function
/// pointers cannot be used as match patterns. The `#[plugin]` macro therefore dispatches by
/// trigger instead.
///
/// Note that this relies on `CommandSpec` remaining a structural-match type; should its
/// definition change, the compiler will reject const patterns with a loud error rather than
/// misbehave.
///
/// Commands may overlap as long as no trigger is a word-prefix of another (e.g. `.y` and `.yt`).
///
/// Two specifications compare equal only when trigger, description, and argument information are
/// all identical — match against the very constant that was registered.
// The derived equality compares the argument function pointer; only structural (compile-time)
// equality for `match` patterns depends on it, never a meaningful runtime comparison.
#[allow(unpredictable_function_pointer_comparisons)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    /// The command trigger as users type it, e.g. `.dig`.
    trigger: &'static str,
    /// A short, user-facing description of the command.
    description: &'static str,
    /// Returns the argument command derived from the command's [`Parser`] type, if any.
    args: Option<fn() -> Command>,
}

impl CommandSpec {
    /// Creates a command with the given `description`, whose arguments are parsed manually.
    ///
    /// The `trigger` must be a single word without whitespace (e.g. `.dig` or `!imdb`); the host
    /// indexes commands by the first word of an incoming message.
    #[must_use]
    pub const fn new(trigger: &'static str, description: &'static str) -> Self {
        Self {
            trigger,
            description,
            args: None,
        }
    }

    /// Creates a command with the given `description`, whose arguments are parsed into the
    /// [`Parser`]-derived type `T`.
    ///
    /// `T` must be `'static` because configured commands are cached and keyed by
    /// [`TypeId`](std::any::TypeId), which requires a `'static` type.
    #[must_use]
    pub const fn with_args<T: Parser + 'static>(
        trigger: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            trigger,
            description,
            args: Some(T::command),
        }
    }

    /// Returns the command trigger as users type it, e.g. `.dig`.
    #[must_use]
    pub const fn trigger(&self) -> &'static str {
        self.trigger
    }

    /// Returns the short, user-facing description of the command.
    #[must_use]
    pub const fn description(&self) -> &'static str {
        self.description
    }
}
