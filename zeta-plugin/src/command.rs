//! IRC prefix command matching and argument parsing.
//!
//! Matches a static prefix against an IRC message and extracts the trailing arguments.
//!
//! # Example
//!
//! ```
//! use zeta_plugin::Prefix;
//!
//! const YT: Prefix = Prefix::new(".yt");
//!
//! assert_eq!(YT.parse(".yt"), Some(""));
//! assert_eq!(YT.parse(".yt rust"), Some("rust"));
//! assert_eq!(YT.parse(".youtube rust"), None);
//! assert_eq!(YT.parse(".goodbye"), None);
//! ```

use argh::FromArgs;
use thiserror::Error;

/// A zero-sized prefix matcher for IRC bot commands.
///
/// Stores a `&'static str` prefix and provides [`parse`](Prefix::parse) to check whether a message
/// starts with the prefix and extract the trailing arguments.
///
/// Because the prefix is a static reference, `Prefix` is [`Copy`], requires no heap allocation, and
/// can be constructed in `const` context.
///
/// # Matching commands by identity
///
/// Plugins handling multiple commands should declare each command as a constant and dispatch on
/// the identity of the declaration — not on string comparisons against literal prefixes. Since
/// `Prefix` is a structural-match newtype around `&'static str`, constants can be used directly
/// as `match` patterns:
///
/// ```
/// use zeta_plugin::Prefix;
///
/// const BYTES: Prefix = Prefix::new(".b");
/// const LENGTH: Prefix = Prefix::new(".len");
///
/// fn handle(command: &Prefix) -> &'static str {
///     match *command {
///         BYTES => "string to bytes",
///         LENGTH => "string length",
///         _ => "unhandled",
///     }
/// }
///
/// assert_eq!(handle(&BYTES), "string to bytes");
/// assert_eq!(handle(&LENGTH), "string length");
/// assert_eq!(handle(&Prefix::new(".other")), "unhandled");
/// ```
///
/// Note that this relies on `Prefix` remaining a structural-match type; should its definition
/// change, the compiler will reject const patterns with a loud error rather than misbehave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefix(&'static str);

impl Prefix {
    /// Creates a new prefix matcher for the given command prefix.
    #[must_use]
    pub const fn new(prefix: &'static str) -> Self {
        Self(prefix)
    }

    /// Returns the raw prefix string.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        self.0
    }

    /// Checks if the input starts with the command prefix, returning the trailing arguments (with
    /// leading whitespace stripped) if it matches.
    ///
    /// Returns `None` if the input does not start with the prefix, or if the character immediately
    /// following the prefix is not whitespace (i.e. it is part of a longer word).
    #[must_use]
    pub fn parse<'a>(&self, input: &'a str) -> Option<&'a str> {
        let suffix = input.strip_prefix(self.0)?;
        match suffix.chars().next() {
            // Input is exactly the prefix — no arguments.
            None => Some(""),
            // Prefix followed by whitespace — skip all leading whitespace.
            Some(c) if c.is_whitespace() => {
                let skipped: usize = suffix
                    .chars()
                    .take_while(|c| c.is_whitespace())
                    .map(char::len_utf8)
                    .sum();
                Some(&suffix[skipped..])
            }
            // Prefix followed by a non-whitespace character — not a match (e.g. `.y` vs `.yt`).
            Some(_) => None,
        }
    }

    /// Parses the trailing arguments of a command into an [`FromArgs`]-derived struct.
    ///
    /// The arguments are tokenized like a POSIX shell (via `shlex`), so quoted arguments and
    /// escapes are supported, and then parsed with `argh`. The prefix itself is used as the
    /// command name in generated usage and help output.
    ///
    /// # Errors
    ///
    /// Returns [`ArgsError::Quoting`] if the arguments could not be tokenized (e.g. unbalanced
    /// quotes), or [`ArgsError::Usage`] if `argh` rejected the arguments — the wrapped string is
    /// the human-readable usage or help output, suitable for replying with directly.
    ///
    /// # Examples
    ///
    /// ```
    /// use argh::FromArgs;
    /// use zeta_plugin::Prefix;
    ///
    /// /// Greeting options.
    /// #[derive(FromArgs)]
    /// struct Opts {
    ///     /// name to greet
    ///     #[argh(positional)]
    ///     name: String,
    /// }
    ///
    /// const HELLO: Prefix = Prefix::new(".hello");
    /// let opts: Opts = HELLO.parse_args("world").unwrap();
    /// assert_eq!(opts.name, "world");
    /// ```
    pub fn parse_args<T: FromArgs>(&self, args: &str) -> Result<T, ArgsError> {
        let tokens = shlex::split(args).ok_or(ArgsError::Quoting)?;
        let tokens = tokens.iter().map(String::as_str).collect::<Vec<_>>();

        T::from_args(&[self.0], &tokens).map_err(|early_exit| ArgsError::Usage(early_exit.output))
    }
}

/// An error returned when command arguments could not be parsed.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ArgsError {
    /// The arguments could not be tokenized (e.g. unbalanced quotes).
    #[error("could not parse arguments")]
    Quoting,
    /// The arguments were rejected; the string contains the generated usage or help output.
    #[error("{0}")]
    Usage(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extracts_args() {
        const CMD: Prefix = Prefix::new("!test");

        assert_eq!(CMD.parse("!test --help"), Some("--help"));
    }

    #[test]
    fn parse_command_is_some() {
        const CMD: Prefix = Prefix::new("!test");

        assert_eq!(CMD.parse("!test"), Some(""));
    }

    #[test]
    fn parse_normalizes_whitespace() {
        const CMD: Prefix = Prefix::new("!test");

        assert_eq!(CMD.parse("!test   --help"), Some("--help"));
        assert_eq!(CMD.parse("!test  \t  args"), Some("args"));
    }

    #[test]
    fn skip_on_non_whitespace_chars() {
        const CMD: Prefix = Prefix::new("!test");

        assert_eq!(CMD.parse("!testing --help"), None);
    }

    #[test]
    fn unicode_whitespace_is_safe() {
        // Ideographic space (U+3000) is 3 bytes — must not panic on byte slice.
        const CMD: Prefix = Prefix::new("!test");

        assert_eq!(CMD.parse("!test\u{3000}args"), Some("args"));
    }

    #[test]
    fn as_str_returns_prefix() {
        const CMD: Prefix = Prefix::new(".yt");

        assert_eq!(CMD.as_str(), ".yt");
    }

    #[test]
    fn parse_args_tokenizes_like_a_shell() {
        #[derive(FromArgs, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// first argument
            #[argh(positional)]
            first: String,
            /// second argument
            #[argh(positional)]
            second: String,
            /// third argument
            #[argh(positional)]
            third: String,
        }

        const CMD: Prefix = Prefix::new(".test");

        let opts: Opts = CMD.parse_args(r#"one "two words" th\ ree"#).unwrap();

        assert_eq!(
            opts,
            Opts {
                first: "one".into(),
                second: "two words".into(),
                third: "th ree".into(),
            }
        );
    }

    #[test]
    fn parse_args_reports_usage_on_error() {
        #[derive(FromArgs, Debug)]
        /// Test options.
        struct Opts {
            /// the required positional argument
            #[argh(positional)]
            #[allow(dead_code)]
            name: String,
        }

        const CMD: Prefix = Prefix::new(".test");

        let err = CMD.parse_args::<Opts>("").unwrap_err();

        assert!(matches!(err, ArgsError::Usage(ref out) if out.contains("name")));
    }

    #[test]
    fn parse_args_reports_quoting_error() {
        #[derive(FromArgs, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// the required positional argument
            #[argh(positional)]
            #[allow(dead_code)]
            name: String,
        }

        const CMD: Prefix = Prefix::new(".test");

        assert_eq!(
            CMD.parse_args::<Opts>("\"unbalanced"),
            Err(ArgsError::Quoting)
        );
    }
}
