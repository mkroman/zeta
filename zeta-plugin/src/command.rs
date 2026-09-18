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
//! assert_eq!(YT.parse(".yt"), Some(""));
//! assert_eq!(YT.parse(".yt rust"), Some("rust"));
//! assert_eq!(YT.parse(".youtube rust"), None);
//! assert_eq!(YT.parse(".goodbye"), None);
//! ```

use argh::{ArgsInfo, CommandInfoWithArgs, FromArgs};
use thiserror::Error;

/// A command served by a plugin: its trigger, description, and the arguments it accepts.
///
/// The `trigger` is the command as users type it — most commands are dot-prefixed (`.yt`), but
/// any single-word prefix (e.g. `!imdb`) works. The description is shown by the host's help
/// command. Commands with typed arguments also store a function pointer to the [`ArgsInfo`]
/// implementation of their argument type, so the host can derive usage and argument information
/// for them without having to parse anything.
///
/// Because every field is a `&'static str` or function pointer, `CommandSpec` is [`Copy`],
/// requires no heap allocation, and can be constructed in `const` context. The same constant
/// serves every role: it is registered through [`Subscriptions`](crate::Subscriptions) during
/// initialization, and matched against the [`CommandEvent`](crate::CommandEvent)'s specification
/// in the command handler.
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
    /// Returns the argument information derived from the command's [`ArgsInfo`] type, if any.
    args: Option<fn() -> CommandInfoWithArgs>,
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
    /// [`ArgsInfo`]-derived type `T`.
    #[must_use]
    pub const fn with_args<T: ArgsInfo>(trigger: &'static str, description: &'static str) -> Self {
        Self {
            trigger,
            description,
            args: Some(T::get_args_info),
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

    /// Checks if `input` starts with the command trigger, returning the trailing arguments (with
    /// leading whitespace stripped) if it does.
    ///
    /// Returns `None` if the input does not start with the trigger, or if the character
    /// immediately following the trigger is not whitespace (i.e. it is part of a longer word).
    #[must_use]
    pub fn parse<'a>(&self, input: &'a str) -> Option<&'a str> {
        let suffix = input.strip_prefix(self.trigger)?;
        match suffix.chars().next() {
            // Input is exactly the trigger — no arguments.
            None => Some(""),
            // Trigger followed by whitespace — skip all leading whitespace.
            Some(c) if c.is_whitespace() => {
                let skipped: usize = suffix
                    .chars()
                    .take_while(|c| c.is_whitespace())
                    .map(char::len_utf8)
                    .sum();
                Some(&suffix[skipped..])
            }
            // Trigger followed by a non-whitespace character — not a match (e.g. `.y` vs `.yt`).
            Some(_) => None,
        }
    }

    /// Parses the trailing arguments of the command into a [`FromArgs`]-derived struct.
    ///
    /// The arguments are tokenized like a POSIX shell (via `shlex`), so quoted arguments and
    /// escapes are supported, and then parsed with `argh`. The trigger itself is used as the
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
    /// use zeta_plugin::CommandSpec;
    ///
    /// /// Greeting options.
    /// #[derive(FromArgs)]
    /// struct Opts {
    ///     /// name to greet
    ///     #[argh(positional)]
    ///     name: String,
    /// }
    ///
    /// const HELLO: CommandSpec = CommandSpec::new(".hello", "Greet someone");
    /// let opts: Opts = HELLO.parse_args("world").unwrap();
    /// assert_eq!(opts.name, "world");
    /// ```
    pub fn parse_args<T: FromArgs>(&self, args: &str) -> Result<T, ArgsError> {
        let tokens = shlex::split(args).ok_or(ArgsError::Quoting)?;
        let tokens = tokens.iter().map(String::as_str).collect::<Vec<_>>();

        T::from_args(&[self.trigger], &tokens).map_err(|early_exit| ArgsError::Usage(early_exit.output))
    }

    /// Parses the trailing arguments of the command into a [`FromArgs`]-derived struct, splitting
    /// them on whitespace.
    ///
    /// Unlike [`parse_args`](CommandSpec::parse_args), the arguments are not tokenized like a
    /// POSIX shell: quotes and escapes are preserved verbatim, so free-form text (e.g. messages
    /// containing apostrophes) reaches a greedy positional argument unharmed.
    ///
    /// # Errors
    ///
    /// Returns [`ArgsError::Usage`] if `argh` rejected the arguments — the wrapped string is the
    /// human-readable usage or help output, suitable for replying with directly. Shell quoting
    /// errors ([`ArgsError::Quoting`]) cannot occur.
    ///
    /// # Examples
    ///
    /// ```
    /// use argh::FromArgs;
    /// use zeta_plugin::CommandSpec;
    ///
    /// /// An alert message and datetime.
    /// #[derive(FromArgs)]
    /// struct Opts {
    ///     /// the message and datetime
    ///     #[argh(positional, greedy)]
    ///     args: Vec<String>,
    /// }
    ///
    /// const ALERT: CommandSpec = CommandSpec::new(".alert", "Add an alert");
    /// let opts: Opts = ALERT.parse_words("don't forget at 4:20").unwrap();
    /// assert_eq!(opts.args.join(" "), "don't forget at 4:20");
    /// ```
    pub fn parse_words<T: FromArgs>(&self, args: &str) -> Result<T, ArgsError> {
        let tokens: Vec<&str> = args.split_whitespace().collect();

        T::from_args(&[self.trigger], &tokens).map_err(|early_exit| ArgsError::Usage(early_exit.output))
    }

    /// Returns the argument information derived from the command's [`ArgsInfo`] type, if any.
    #[must_use]
    pub fn args_info(&self) -> Option<CommandInfoWithArgs> {
        self.args.map(|info| info())
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
        const CMD: CommandSpec = CommandSpec::new("!test", "test");

        assert_eq!(CMD.parse("!test --help"), Some("--help"));
    }

    #[test]
    fn parse_command_is_some() {
        const CMD: CommandSpec = CommandSpec::new("!test", "test");

        assert_eq!(CMD.parse("!test"), Some(""));
    }

    #[test]
    fn parse_normalizes_whitespace() {
        const CMD: CommandSpec = CommandSpec::new("!test", "test");

        assert_eq!(CMD.parse("!test   --help"), Some("--help"));
        assert_eq!(CMD.parse("!test  \t  args"), Some("args"));
    }

    #[test]
    fn skip_on_non_whitespace_chars() {
        const CMD: CommandSpec = CommandSpec::new("!test", "test");

        assert_eq!(CMD.parse("!testing --help"), None);
    }

    #[test]
    fn unicode_whitespace_is_safe() {
        // Ideographic space (U+3000) is 3 bytes — must not panic on byte slice.
        const CMD: CommandSpec = CommandSpec::new("!test", "test");

        assert_eq!(CMD.parse("!test\u{3000}args"), Some("args"));
    }

    #[test]
    fn trigger_returns_trigger() {
        const CMD: CommandSpec = CommandSpec::new(".yt", "yt");

        assert_eq!(CMD.trigger(), ".yt");
    }

    #[test]
    fn description_returns_description() {
        const CMD: CommandSpec = CommandSpec::new(".dig", "Look up DNS records for a domain");

        assert_eq!(CMD.description(), "Look up DNS records for a domain");
    }

    #[test]
    fn equality_is_structural() {
        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        assert_eq!(CMD, CommandSpec::new(".test", "test"));
        assert_ne!(CMD, CommandSpec::new(".other", "test"));
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

        const CMD: CommandSpec = CommandSpec::new(".test", "test");

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

        const CMD: CommandSpec = CommandSpec::new(".test", "test");

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

        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        assert_eq!(
            CMD.parse_args::<Opts>("\"unbalanced"),
            Err(ArgsError::Quoting)
        );
    }

    #[test]
    fn parse_words_splits_on_whitespace() {
        #[derive(FromArgs, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// the message and datetime
            #[argh(positional, greedy)]
            #[allow(dead_code)]
            args: Vec<String>,
        }

        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        let opts: Opts = CMD.parse_words("  hello\tworld  ").unwrap();

        assert_eq!(opts.args, vec!["hello".to_owned(), "world".to_owned()]);

        let opts: Opts = CMD.parse_words("").unwrap();

        assert!(opts.args.is_empty());
    }

    #[test]
    fn parse_words_keeps_quotes_and_apostrophes_verbatim() {
        #[derive(FromArgs, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// the message and datetime
            #[argh(positional, greedy)]
            #[allow(dead_code)]
            args: Vec<String>,
        }

        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        let opts: Opts = CMD.parse_words(r#"don't "forget me" at 4:20"#).unwrap();

        assert_eq!(opts.args.join(" "), r#"don't "forget me" at 4:20"#);
    }

    #[test]
    fn parse_words_parses_flags() {
        #[derive(FromArgs, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// list instead
            #[argh(switch, short = 'l')]
            list: bool,
            /// the message and datetime
            #[argh(positional, greedy)]
            #[allow(dead_code)]
            args: Vec<String>,
        }

        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        let opts: Opts = CMD.parse_words("-l hello world").unwrap();

        assert!(opts.list);
        assert_eq!(opts.args, vec!["hello".to_owned(), "world".to_owned()]);

        let opts: Opts = CMD.parse_words("-- don't panic").unwrap();

        assert!(!opts.list);
        assert_eq!(opts.args, vec!["don't".to_owned(), "panic".to_owned()]);
    }

    #[test]
    fn parse_words_reports_usage_on_error() {
        #[derive(FromArgs, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// the message and datetime
            #[argh(positional, greedy)]
            #[allow(dead_code)]
            args: Vec<String>,
        }

        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        let err = CMD.parse_words::<Opts>("-x").unwrap_err();

        assert!(matches!(err, ArgsError::Usage(ref out) if out.contains("-x")));
    }

    #[test]
    fn args_info_returns_derived_information() {
        /// Look up a domain name.
        #[derive(argh::ArgsInfo)]
        struct Opts {
            /// the domain to look up
            #[argh(positional)]
            #[allow(dead_code)]
            name: String,
        }

        const DIG: CommandSpec = CommandSpec::with_args::<Opts>(
            ".dig",
            "Look up DNS records for a domain",
        );

        assert_eq!(DIG.description(), "Look up DNS records for a domain");

        let info = DIG.args_info().unwrap();

        assert_eq!(info.positionals[0].name, "name");
    }

    #[test]
    fn plain_command_has_no_args_info() {
        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        assert!(CMD.args_info().is_none());
    }
}
