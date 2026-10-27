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

use std::ops::Range;

use clap::{ArgAction, ColorChoice, Command, Parser};
use thiserror::Error;

/// A command served by a plugin: its trigger, description, and the arguments it accepts.
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
    #[must_use]
    pub const fn with_args<T: Parser>(trigger: &'static str, description: &'static str) -> Self {
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

    /// Checks if `input` starts with the command trigger, returning the trailing arguments (with
    /// leading whitespace stripped) if it does.
    ///
    /// Returns `None` if the input does not start with the trigger, or if the character
    /// immediately following the trigger is not whitespace (i.e. it is part of a longer word).
    #[must_use]
    pub fn parse<'a>(&self, input: &'a str) -> Option<&'a str> {
        self.parse_range(input).map(|range| &input[range])
    }

    /// Returns the byte range of the trailing arguments within `input`, if it matches.
    ///
    /// The range is empty when the command was invoked without arguments, and covers the
    /// arguments with their leading whitespace stripped otherwise. Callers that hold the input
    /// (rather than a borrowed slice of it) use this instead of re-deriving the range from the
    /// returned slice's length.
    pub(crate) fn parse_range(&self, input: &str) -> Option<Range<usize>> {
        let suffix = input.strip_prefix(self.trigger)?;

        match suffix.chars().next() {
            // Input is exactly the trigger — no arguments.
            None => Some(input.len()..input.len()),
            // Trigger followed by whitespace — skip all leading whitespace.
            Some(c) if c.is_whitespace() => {
                let start = input.len() - suffix.trim_start().len();

                Some(start..input.len())
            }
            // Trigger followed by a non-whitespace character — not a match (e.g. `.y` vs `.yt`).
            Some(_) => None,
        }
    }

    /// Parses the trailing arguments of the command into a [`Parser`]-derived struct.
    ///
    /// The arguments are tokenized like a POSIX shell (via `shlex`), so quoted arguments and
    /// escapes are supported, and then parsed with `clap`. The trigger itself is used as the
    /// command name in generated usage and help output.
    ///
    /// # Errors
    ///
    /// Returns [`ArgsError::Quoting`] if the arguments could not be tokenized (e.g. unbalanced
    /// quotes), or [`ArgsError::Usage`] if `clap` rejected the arguments — the wrapped string is
    /// the human-readable usage or help output, suitable for replying with directly.
    ///
    /// # Examples
    ///
    /// ```
    /// use clap::Parser;
    /// use zeta_plugin::CommandSpec;
    ///
    /// /// Greeting options.
    /// #[derive(Parser)]
    /// struct Opts {
    ///     /// name to greet
    ///     name: String,
    /// }
    ///
    /// const HELLO: CommandSpec = CommandSpec::new(".hello", "Greet someone");
    /// let opts: Opts = HELLO.parse_args("world").unwrap();
    /// assert_eq!(opts.name, "world");
    /// ```
    pub fn parse_args<T: Parser>(&self, args: &str) -> Result<T, ArgsError> {
        let tokens = shlex::split(args).ok_or(ArgsError::Quoting)?;
        let tokens = tokens.iter().map(String::as_str).collect::<Vec<_>>();

        self.parse_tokens(&tokens)
    }

    /// Parses the trailing arguments of the command into a [`Parser`]-derived struct, splitting
    /// them on whitespace.
    ///
    /// Unlike [`parse_args`](CommandSpec::parse_args), the arguments are not tokenized like a
    /// POSIX shell: quotes and escapes are preserved verbatim, so free-form text (e.g. messages
    /// containing apostrophes) reaches a trailing positional argument unharmed.
    ///
    /// # Errors
    ///
    /// Returns [`ArgsError::Usage`] if `clap` rejected the arguments — the wrapped string is the
    /// human-readable usage or help output, suitable for replying with directly. Shell quoting
    /// errors ([`ArgsError::Quoting`]) cannot occur.
    ///
    /// # Examples
    ///
    /// ```
    /// use clap::Parser;
    /// use zeta_plugin::CommandSpec;
    ///
    /// /// An alert message and datetime.
    /// #[derive(Parser)]
    /// struct Opts {
    ///     /// the message and datetime
    ///     #[arg(num_args = 1.., trailing_var_arg = true)]
    ///     args: Vec<String>,
    /// }
    ///
    /// const ALERT: CommandSpec = CommandSpec::new(".alert", "Add an alert");
    /// let opts: Opts = ALERT.parse_words("don't forget at 4:20").unwrap();
    /// assert_eq!(opts.args.join(" "), "don't forget at 4:20");
    /// ```
    pub fn parse_words<T: Parser>(&self, args: &str) -> Result<T, ArgsError> {
        let tokens: Vec<&str> = args.split_whitespace().collect();

        self.parse_tokens(&tokens)
    }

    /// Parses `tokens` into `T`, with the trigger as the command name in the usage output.
    fn parse_tokens<T: Parser>(&self, tokens: &[&str]) -> Result<T, ArgsError> {
        let command = configure(T::command(), self.trigger);
        let argv = std::iter::once(self.trigger).chain(tokens.iter().copied());
        let matches = command
            .try_get_matches_from(argv)
            .map_err(|error| ArgsError::Usage(error.to_string()))?;

        T::from_arg_matches(&matches).map_err(|error| ArgsError::Usage(error.to_string()))
    }

    /// Returns the argument command derived from the command's [`Parser`] type, if any.
    ///
    /// The returned [`Command`] describes the arguments and subcommands a user may pass; the
    /// host's help command renders its usage and details.
    #[must_use]
    pub fn args_info(&self) -> Option<Command> {
        self.args.map(|info| configure(info(), self.trigger))
    }
}

/// Applies the settings shared by argument parsing and help introspection.
///
/// The command is named after the trigger so generated usage reads like the user typed it, the
/// implicit `help` subcommand is disabled (plugins provide their own `.help`), and styling is
/// turned off because replies are IRC messages, not terminal output.
fn configure(command: Command, trigger: &'static str) -> Command {
    let command = command
        .bin_name(trigger)
        .disable_help_subcommand(true)
        .color(ColorChoice::Never);

    tolerate_repeated_switches(command)
}

/// Allows a switch to appear more than once.
///
/// clap reports an argument conflict for a second `SetTrue`/`SetFalse` occurrence, whereas the
/// parser this API replaced treated repeated switches as idempotent. Only switches are relaxed:
/// repeating an option that takes a value still reports a conflict, as before.
fn tolerate_repeated_switches(command: Command) -> Command {
    let command = command.mut_args(|argument| {
        if matches!(
            *argument.get_action(),
            ArgAction::SetTrue | ArgAction::SetFalse
        ) {
            let id = argument.get_id().clone();

            argument.overrides_with(id)
        } else {
            argument
        }
    });

    command.mut_subcommands(tolerate_repeated_switches)
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

    use zeta_test_support::assert_empty;

    #[derive(Parser, Debug, PartialEq)]
    /// Test options.
    struct NameOpts {
        /// the required positional argument
        #[allow(dead_code)]
        name: String,
    }

    #[derive(Parser, Debug, PartialEq)]
    /// Test options.
    struct GreedyOpts {
        /// the message and datetime
        #[arg(num_args = 1.., trailing_var_arg = true)]
        #[allow(dead_code)]
        args: Vec<String>,
    }

    #[test]
    fn parse_matches_the_trigger_prefix() {
        const CMD: CommandSpec = CommandSpec::new("!test", "test");

        assert_eq!(CMD.parse("!test --help"), Some("--help"));
        assert_eq!(CMD.parse("!test"), Some(""));
        // Repeated whitespace between the trigger and the arguments collapses.
        assert_eq!(CMD.parse("!test   --help"), Some("--help"));
        assert_eq!(CMD.parse("!test  \t  args"), Some("args"));
        // Only the exact trigger prefix matches.
        assert_eq!(CMD.parse("!testing --help"), None);
        // Ideographic space (U+3000) is 3 bytes — must not panic on byte slice.
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
        #[derive(Parser, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// first argument
            first: String,
            /// second argument
            second: String,
            /// third argument
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
        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        let err = CMD.parse_args::<NameOpts>("").unwrap_err();

        // The string is replied to verbatim, so it must read as a usage message for the
        // command and name the missing argument; the exact layout belongs to clap.
        let ArgsError::Usage(usage) = err else {
            panic!("a missing argument should be a usage error");
        };
        assert!(usage.contains("Usage:"), "{usage}");
        assert!(usage.contains(".test"), "{usage}");
        assert!(usage.contains("<NAME>"), "{usage}");
    }

    #[test]
    fn parse_args_reports_quoting_error() {
        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        assert_eq!(
            CMD.parse_args::<NameOpts>("\"unbalanced"),
            Err(ArgsError::Quoting)
        );
    }

    #[test]
    fn parse_words_splits_on_whitespace() {
        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        let opts: GreedyOpts = CMD.parse_words("  hello\tworld  ").unwrap();

        assert_eq!(opts.args, vec!["hello".to_owned(), "world".to_owned()]);

        let opts: GreedyOpts = CMD.parse_words("").unwrap();

        assert_empty!(opts.args);
    }

    #[test]
    fn parse_words_keeps_quotes_and_apostrophes_verbatim() {
        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        let opts: GreedyOpts = CMD.parse_words(r#"don't "forget me" at 4:20"#).unwrap();

        assert_eq!(opts.args.join(" "), r#"don't "forget me" at 4:20"#);
    }

    #[test]
    fn parse_words_parses_flags() {
        #[derive(Parser, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// list instead
            #[arg(short = 'l', long)]
            list: bool,
            /// the message and datetime
            #[arg(num_args = 1.., trailing_var_arg = true)]
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
    fn repeated_switches_are_idempotent() {
        #[derive(Parser, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// list instead
            #[arg(short = 'l', long)]
            list: bool,
            /// the message and datetime
            #[arg(num_args = 1.., trailing_var_arg = true)]
            #[allow(dead_code)]
            args: Vec<String>,
        }

        const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".test", "test");

        let opts: Opts = CMD.parse_words("-l -l").expect("switches are idempotent");

        assert!(opts.list);
    }

    #[test]
    fn repeated_options_still_conflict() {
        #[derive(Parser, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// the channel
            #[arg(long)]
            channel: Option<String>,
        }

        const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".test", "test");

        assert!(CMD.parse_words::<Opts>("--channel a --channel b").is_err());
    }

    #[test]
    fn parse_words_reports_usage_on_error() {
        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        let err = CMD.parse_words::<GreedyOpts>("-x").unwrap_err();

        // The reply must name the offending token; the rest of the layout is clap's.
        let ArgsError::Usage(usage) = err else {
            panic!("an unknown flag should be a usage error");
        };
        assert!(usage.contains("-x"), "{usage}");
    }

    #[test]
    fn option_definitions_are_valid() {
        use clap::CommandFactory;

        // clap reports contradictory definitions through asserts; evaluating them here turns a
        // bad definition into a test failure instead of a panic in debug builds.
        NameOpts::command().debug_assert();
        GreedyOpts::command().debug_assert();
    }

    #[test]
    fn args_info_is_named_after_the_trigger() {
        /// Look up a domain name.
        #[derive(Parser)]
        struct Opts {
            /// the domain to look up
            #[allow(dead_code)]
            name: String,
        }

        const DIG: CommandSpec = CommandSpec::with_args::<Opts>(".dig", "dig");

        let info = DIG.args_info().unwrap();

        assert_eq!(info.get_bin_name(), Some(".dig"));
        assert_eq!(info.get_subcommands().count(), 0);
        // IRC replies cannot carry ANSI styling.
        assert_eq!(info.get_color(), ColorChoice::Never);
    }

    #[test]
    fn args_info_returns_derived_information() {
        /// Look up a domain name.
        #[derive(Parser)]
        struct Opts {
            /// the domain to look up
            #[allow(dead_code)]
            name: String,
        }

        const DIG: CommandSpec =
            CommandSpec::with_args::<Opts>(".dig", "Look up DNS records for a domain");

        assert_eq!(DIG.description(), "Look up DNS records for a domain");

        let info = DIG.args_info().unwrap();
        let positional = info.get_positionals().next().unwrap();

        assert_eq!(positional.get_id().as_str(), "name");
    }

    #[test]
    fn plain_command_has_no_args_info() {
        const CMD: CommandSpec = CommandSpec::new(".test", "test");

        assert!(CMD.args_info().is_none());
    }
}
