//! The `.alert` command line and its clap definition.

use clap::Parser;

use zeta_plugin::CommandSpec;

/// The `.alert` command.
pub(crate) const ALERT: CommandSpec = CommandSpec::with_args::<Opts>(
    ".alert",
    "Schedule an alert to be posted later, or list pending alerts",
);

/// The usage hint for the `.alert` command.
pub(crate) const USAGE: &str = "Usage: .alert\x0f [-l] <message> <in|at> <datetime>";

/// Schedule an alert to be posted later, or list pending alerts.
#[derive(Parser, Debug)]
pub(crate) struct Opts {
    /// list your pending alerts in this channel
    #[arg(short = 'l', long)]
    pub(super) list: bool,
    /// the alert message, followed by `in` or `at` and a datetime
    #[arg(num_args = 1.., trailing_var_arg = true)]
    pub(super) args: Vec<String>,
}

impl Opts {
    /// Returns the joined message and datetime expression.
    pub(super) fn input(&self) -> String {
        self.args.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;
    use zeta_plugin::ArgsError;

    #[test]
    fn cli_definition_is_valid() {
        // clap reports contradictory definitions through asserts; evaluating them here turns a
        // bad definition into a test failure instead of a panic in debug builds.
        Opts::command().debug_assert();
    }

    #[test]
    fn parses_the_list_flag() {
        let opts: Opts = ALERT.parse_words("-l").unwrap();

        assert!(opts.list);
        assert_eq!(opts.input(), "");

        let opts: Opts = ALERT.parse_words("--list").unwrap();

        assert!(opts.list);
    }

    #[test]
    fn parses_message_and_datetime() {
        let cases = [("hello world at 4:20", "hello world at 4:20"), ("", "")];

        for (input, expected) in cases {
            let opts: Opts = ALERT.parse_words(input).unwrap();

            assert!(!opts.list, "for {input:?}");
            assert_eq!(opts.input(), expected, "for {input:?}");
        }
    }

    #[test]
    fn keeps_messages_verbatim() {
        let opts: Opts = ALERT
            .parse_words(r#"don't "forget me"  at   4:20"#)
            .unwrap();

        assert_eq!(opts.input(), r#"don't "forget me" at 4:20"#);
    }

    #[test]
    fn rejects_unknown_flags() {
        assert!(matches!(
            ALERT.parse_words::<Opts>("-x").unwrap_err(),
            ArgsError::Usage(_)
        ));
    }
}
