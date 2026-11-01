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

use std::any::TypeId;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, LazyLock, Mutex, TryLockError};

use clap::{ArgAction, ColorChoice, Command, Parser};
use thiserror::Error;
use zeta_utils::sync;

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
    /// [`TypeId`], which requires a `'static` type.
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
    pub fn parse_args<T: Parser + 'static>(&self, args: &str) -> Result<T, ArgsError> {
        let tokens = shlex::split(args).ok_or(ArgsError::Quoting)?;

        self.parse_tokens(tokens.iter().map(String::as_str))
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
    pub fn parse_words<T: Parser + 'static>(&self, args: &str) -> Result<T, ArgsError> {
        self.parse_tokens(args.split_whitespace())
    }

    /// Parses the trailing arguments with the tokenization selected by `mode`.
    ///
    /// # Errors
    ///
    /// Returns the error of the selected parser: [`ArgsError::Quoting`] for shell tokenization,
    /// or [`ArgsError::Usage`] when `clap` rejected the arguments.
    pub fn parse_with<T: Parser + 'static>(
        &self,
        args: &str,
        mode: ArgsMode,
    ) -> Result<T, ArgsError> {
        match mode {
            ArgsMode::Shell => self.parse_args(args),
            ArgsMode::Words => self.parse_words(args),
        }
    }

    /// Parses `tokens` into `T`, with the trigger as the command name in the usage output.
    fn parse_tokens<'a, T, I>(&self, tokens: I) -> Result<T, ArgsError>
    where
        T: Parser + 'static,
        I: IntoIterator<Item = &'a str>,
    {
        let matches = with_cached_command::<T, _>(self.trigger, move |command| {
            let argv = std::iter::once(self.trigger).chain(tokens);

            command
                .try_get_matches_from_mut(argv)
                .map_err(|error| ArgsError::Usage(error.to_string()))
        })?;

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

/// The cache key of a configured command: its trigger and the argument type it parses.
type CommandKey = (&'static str, TypeId);

/// The configured commands reused across parses, initialized on first use.
///
/// [`Command`] is `Send`, and [`Command::try_get_matches_from_mut`] is clap's API for parsing
/// with a command that stays alive, so one configured tree per command is reused instead of
/// rebuilding the whole argument tree on every call. The trigger is part of the key because it
/// is rendered as the binary name in usage and help output; the argument type is keyed by
/// [`TypeId`], so parsers must be `'static`.
static COMMANDS: LazyLock<Mutex<HashMap<CommandKey, Arc<Mutex<Command>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Returns the cached command for `T` under `trigger`, configuring it on first use.
fn cached_command<T: Parser + 'static>(trigger: &'static str) -> Arc<Mutex<Command>> {
    let mut commands = sync::lock(&COMMANDS);

    Arc::clone(
        commands
            .entry((trigger, TypeId::of::<T>()))
            .or_insert_with(|| Arc::new(Mutex::new(configure(T::command(), trigger)))),
    )
}

/// Runs `parse` against the cached command for `T` under `trigger`.
///
/// A busy entry means another thread is parsing the same command (or a value parser recursed
/// into it); rather than block, that call configures a fresh command. A poisoned entry comes
/// from a parse that panicked: it is evicted so a later call caches a fresh command, and this
/// call uses a fresh command directly.
fn with_cached_command<T, R>(trigger: &'static str, parse: impl FnOnce(&mut Command) -> R) -> R
where
    T: Parser + 'static,
{
    match cached_command::<T>(trigger).try_lock() {
        Ok(mut command) => parse(&mut command),
        Err(TryLockError::Poisoned(_)) => {
            sync::lock(&COMMANDS).remove(&(trigger, TypeId::of::<T>()));

            parse(&mut configure(T::command(), trigger))
        }
        Err(TryLockError::WouldBlock) => parse(&mut configure(T::command(), trigger)),
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

    use clap::{CommandFactory, FromArgMatches};
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

        // The second parse runs against the cached command; the relaxation must survive reuse.
        let opts: Opts = CMD
            .parse_words("--list -l -l")
            .expect("switches are idempotent with a cached command");

        assert!(opts.list);
    }

    #[test]
    fn repeated_parses_reuse_the_cached_command() {
        #[derive(Parser, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// the query
            query: String,
            /// the language
            #[arg(long, default_value = "en")]
            language: String,
        }

        const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".test", "test");

        let first: Opts = CMD.parse_words("hello").unwrap();
        let second: Opts = CMD.parse_words("--language fr hello").unwrap();
        let third: Opts = CMD.parse_args("\"hello\" --language de").unwrap();

        assert_eq!(
            first,
            Opts {
                query: "hello".into(),
                language: "en".into(),
            }
        );
        assert_eq!(
            second,
            Opts {
                query: "hello".into(),
                language: "fr".into(),
            }
        );
        assert_eq!(
            third,
            Opts {
                query: "hello".into(),
                language: "de".into(),
            }
        );
    }

    #[test]
    fn repeated_parses_build_the_command_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use clap::{Arg, ArgMatches};

        /// Test options built by hand, so the test can count command builds.
        #[derive(Debug, PartialEq)]
        struct CountedOpts {
            /// the required positional argument
            name: String,
        }

        static BUILDS: AtomicUsize = AtomicUsize::new(0);

        impl CommandFactory for CountedOpts {
            fn command() -> Command {
                BUILDS.fetch_add(1, Ordering::SeqCst);

                Command::new("counted").arg(Arg::new("name").required(true))
            }

            fn command_for_update() -> Command {
                Self::command()
            }
        }

        impl FromArgMatches for CountedOpts {
            fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
                matches
                    .get_one::<String>("name")
                    .cloned()
                    .map(|name| Self { name })
                    .ok_or_else(|| {
                        clap::Error::raw(
                            clap::error::ErrorKind::MissingRequiredArgument,
                            "missing name",
                        )
                    })
            }

            fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
                *self = Self::from_arg_matches(matches)?;

                Ok(())
            }
        }

        impl Parser for CountedOpts {}

        const CMD: CommandSpec = CommandSpec::with_args::<CountedOpts>(".counted", "test");

        let first: CountedOpts = CMD.parse_words("one").unwrap();
        let second: CountedOpts = CMD.parse_words("two").unwrap();
        let third: CountedOpts = CMD.parse_args("three").unwrap();

        assert_eq!(
            BUILDS.load(Ordering::SeqCst),
            1,
            "the command should be built once and then reused"
        );
        assert_eq!(first, CountedOpts { name: "one".into() });
        assert_eq!(second, CountedOpts { name: "two".into() });
        assert_eq!(
            third,
            CountedOpts {
                name: "three".into()
            }
        );
    }

    #[test]
    fn a_busy_cached_command_parses_without_blocking() {
        use std::sync::mpsc;
        use std::time::Duration;

        #[derive(Parser, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// the required positional argument
            name: String,
        }

        const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".busy", "test");

        let cached = cached_command::<Opts>(".busy");
        let guard = sync::lock(&cached);

        let (sender, receiver) = mpsc::channel();
        let parse = std::thread::spawn(move || {
            let parsed: Result<Opts, ArgsError> = CMD.parse_words("one");
            sender
                .send(parsed)
                .expect("the test thread should receive the parse result");
        });

        let parsed = match receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(parsed) => parsed,
            Err(error) => {
                drop(guard);
                let _ = parse.join();

                panic!("parsing a busy cached command blocked: {error}");
            }
        };

        drop(guard);

        assert_eq!(parsed.unwrap(), Opts { name: "one".into() });

        // Once the lock is released, parsing goes back to the cached command.
        let again: Opts = CMD.parse_words("two").unwrap();

        assert_eq!(again, Opts { name: "two".into() });
    }

    #[test]
    fn a_poisoned_cached_command_is_replaced() {
        use std::sync::atomic::{AtomicBool, Ordering};

        static PANICKED: AtomicBool = AtomicBool::new(false);

        /// A value parser that fails the first parse, leaving the cached command poisoned.
        // clap's value parser functions must return `Result`, even though this one only panics.
        #[allow(clippy::unnecessary_wraps)]
        fn panics_once(value: &str) -> Result<String, String> {
            assert!(
                PANICKED.swap(true, Ordering::SeqCst),
                "the first parse panics while holding the cached command"
            );

            Ok(value.to_owned())
        }

        #[derive(Parser, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// the required positional argument
            #[arg(value_parser = panics_once)]
            name: String,
        }

        const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".poisoned", "test");

        let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            CMD.parse_words::<Opts>("one")
        }));

        assert!(first.is_err(), "the first parse should panic");

        // The poisoned entry is evicted, so this parse configures a fresh command.
        let second: Opts = CMD
            .parse_words("two")
            .expect("a poisoned command should be replaced");

        assert_eq!(second, Opts { name: "two".into() });
    }

    #[test]
    fn repeated_missing_argument_reports_the_same_usage() {
        const CMD: CommandSpec = CommandSpec::with_args::<NameOpts>(".missing", "test");

        let first = CMD.parse_words::<NameOpts>("").unwrap_err();
        let second = CMD.parse_words::<NameOpts>("").unwrap_err();

        assert_eq!(first, second);

        let ArgsError::Usage(usage) = first else {
            panic!("a missing argument should be a usage error");
        };
        assert!(usage.contains(".missing"), "{usage}");
        assert!(usage.contains("<NAME>"), "{usage}");
    }

    #[test]
    fn repeated_unknown_flag_reports_the_same_usage() {
        const CMD: CommandSpec = CommandSpec::with_args::<NameOpts>(".unknown", "test");

        let first = CMD.parse_words::<NameOpts>("-x").unwrap_err();
        let second = CMD.parse_words::<NameOpts>("-x").unwrap_err();

        assert_eq!(first, second);

        let ArgsError::Usage(usage) = first else {
            panic!("an unknown flag should be a usage error");
        };
        assert!(usage.contains("-x"), "{usage}");
    }

    #[test]
    fn repeated_help_output_is_identical() {
        const CMD: CommandSpec = CommandSpec::with_args::<GreedyOpts>(".help", "test");

        // A successful parse first, so help is rendered from an already-built cached command.
        assert_eq!(
            CMD.parse_words::<GreedyOpts>("hello").unwrap().args,
            vec!["hello".to_owned()]
        );

        let first = CMD.parse_words::<GreedyOpts>("--help").unwrap_err();
        let second = CMD.parse_words::<GreedyOpts>("--help").unwrap_err();

        assert_eq!(first, second);

        let ArgsError::Usage(help) = first else {
            panic!("--help should return the rendered help");
        };
        assert!(help.contains(".help"), "{help}");
        assert!(help.contains("--help"), "{help}");
    }

    #[test]
    fn cached_command_parses_subcommands_repeatedly() {
        #[derive(Parser, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// the subcommand
            #[command(subcommand)]
            command: Action,
        }

        #[derive(clap::Subcommand, Debug, PartialEq)]
        /// Test actions.
        enum Action {
            /// add an entry
            Add {
                /// the entry name
                name: String,
            },
            /// list the entries
            List,
        }

        const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".sub", "test");

        assert_eq!(
            CMD.parse_words::<Opts>("add one").unwrap(),
            Opts {
                command: Action::Add { name: "one".into() },
            }
        );
        assert_eq!(
            CMD.parse_words::<Opts>("list").unwrap(),
            Opts {
                command: Action::List,
            }
        );
        assert_eq!(
            CMD.parse_words::<Opts>("add two").unwrap(),
            Opts {
                command: Action::Add { name: "two".into() },
            }
        );
    }

    #[test]
    fn cached_parses_match_freshly_configured_commands() {
        #[derive(Parser, Debug, PartialEq)]
        /// Test options.
        struct Opts {
            /// the query
            query: String,
            /// the language
            #[arg(long, default_value = "en")]
            language: String,
            /// verbose output
            #[arg(short, long)]
            verbose: bool,
        }

        const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".compare", "test");

        // Populate the cache before the comparison.
        assert_eq!(CMD.parse_words::<Opts>("warm").unwrap().query, "warm");

        for args in [
            "hello",
            "--language fr hello",
            "-v hello",
            "-v -v hello",
            "--verbose --verbose hello",
            "",
            "-x",
            "--language",
            "one two",
            "--help",
        ] {
            let cached = CMD
                .parse_words::<Opts>(args)
                .map_err(|error| error.to_string());

            let fresh = configure(Opts::command(), CMD.trigger())
                .try_get_matches_from(std::iter::once(CMD.trigger()).chain(args.split_whitespace()))
                .map_err(|error| error.to_string())
                .and_then(|matches| {
                    Opts::from_arg_matches(&matches).map_err(|error| error.to_string())
                });

            assert_eq!(cached, fresh, "arguments: {args:?}");
        }
    }

    #[test]
    fn different_command_types_do_not_share_a_cache_entry() {
        #[derive(Parser, Debug, PartialEq)]
        /// Test options.
        struct Left {
            /// left flag
            #[arg(long)]
            left: bool,
        }

        #[derive(Parser, Debug, PartialEq)]
        /// Test options.
        struct Right {
            /// right flag
            #[arg(long)]
            right: bool,
        }

        const LEFT: CommandSpec = CommandSpec::with_args::<Left>(".shared", "left");
        const RIGHT: CommandSpec = CommandSpec::with_args::<Right>(".shared", "right");

        assert!(LEFT.parse_words::<Left>("--left").unwrap().left);
        assert!(RIGHT.parse_words::<Right>("--right").unwrap().right);

        // Each type only accepts its own flag.
        assert!(LEFT.parse_words::<Left>("--right").is_err());
        assert!(RIGHT.parse_words::<Right>("--left").is_err());

        // The first entry still parses after the second type was configured.
        assert!(LEFT.parse_words::<Left>("--left").unwrap().left);
    }

    #[test]
    fn the_same_type_gets_one_entry_per_trigger() {
        const ONE: CommandSpec = CommandSpec::with_args::<NameOpts>(".one", "test");
        const TWO: CommandSpec = CommandSpec::with_args::<NameOpts>(".two", "test");

        let Err(ArgsError::Usage(one)) = ONE.parse_words::<NameOpts>("") else {
            panic!("a missing argument should be a usage error");
        };
        let Err(ArgsError::Usage(two)) = TWO.parse_words::<NameOpts>("") else {
            panic!("a missing argument should be a usage error");
        };

        assert!(one.contains(".one"), "{one}");
        assert!(!one.contains(".two"), "{one}");
        assert!(two.contains(".two"), "{two}");
        assert!(!two.contains(".one"), "{two}");
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
