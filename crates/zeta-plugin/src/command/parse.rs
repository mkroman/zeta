//! Parsing of a command's trailing arguments.
//!
//! The [`CommandSpec`] methods here strip the trigger from an incoming message, tokenize the rest
//! according to [`ArgsMode`], and hand the tokens to `clap`; the configured command comes from the
//! `cache` module, so repeated parses reuse one argument tree instead of rebuilding it. The
//! [`args_info`](CommandSpec::args_info) method exposes the same configured command for help
//! introspection rather than parsing.

use std::ops::Range;

use clap::{Command, Parser};

use super::cache::{CACHE, configure};
use super::{ArgsError, ArgsMode, CommandSpec};

impl CommandSpec {
    /// Checks if `input` starts with the command trigger, returning the trailing arguments (with
    /// leading whitespace stripped) if it does.
    ///
    /// Returns `None` if the input does not start with the trigger, or if the character
    /// immediately following the trigger is not whitespace (i.e. it is part of a longer word).
    #[must_use]
    pub fn trailing_args<'a>(&self, input: &'a str) -> Option<&'a str> {
        self.args_range(input).map(|range| &input[range])
    }

    /// Returns the byte range of the trailing arguments within `input`, if it matches.
    ///
    /// The range is empty when the command was invoked without arguments, and covers the
    /// arguments with their leading whitespace stripped otherwise. Callers that hold the input
    /// (rather than a borrowed slice of it) use this instead of re-deriving the range from the
    /// returned slice's length.
    pub(crate) fn args_range(&self, input: &str) -> Option<Range<usize>> {
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
    /// let opts: Opts = HELLO.parse_shell("world").unwrap();
    /// assert_eq!(opts.name, "world");
    /// ```
    pub fn parse_shell<T: Parser + 'static>(&self, args: &str) -> Result<T, ArgsError> {
        let tokens = shlex::split(args).ok_or(ArgsError::Quoting)?;

        self.parse_tokens(tokens.iter().map(String::as_str))
    }

    /// Parses the trailing arguments of the command into a [`Parser`]-derived struct, splitting
    /// them on whitespace.
    ///
    /// Unlike [`parse_shell`](CommandSpec::parse_shell), the arguments are not tokenized like a
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
            ArgsMode::Shell => self.parse_shell(args),
            ArgsMode::Words => self.parse_words(args),
        }
    }

    /// Parses `tokens` into `T`, with the trigger as the command name in the usage output.
    fn parse_tokens<'a, T, I>(&self, tokens: I) -> Result<T, ArgsError>
    where
        T: Parser + 'static,
        I: IntoIterator<Item = &'a str>,
    {
        let matches = CACHE.with_cached_command::<T, _>(self.trigger, move |command| {
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
