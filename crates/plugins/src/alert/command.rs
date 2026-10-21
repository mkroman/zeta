//! The `.alert` command and its argument handling.

use argh::{ArgsInfo, FromArgs};

use crate::prelude::*;

/// The `.alert` command.
pub(super) const ALERT: CommandSpec = CommandSpec::with_args::<Opts>(
    ".alert",
    "Schedule an alert to be posted later, or list pending alerts",
);

/// The usage hint for the `.alert` command.
pub(super) const USAGE: &str = "Usage: .alert\x0f [-l] <message> <in|at> <datetime>";

/// Schedule an alert to be posted later, or list pending alerts.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(help_triggers("--help"))]
pub(super) struct Opts {
    /// list your pending alerts in this channel
    #[argh(switch, short = 'l')]
    pub(super) list: bool,
    /// the alert message, followed by `in` or `at` and a datetime
    #[argh(positional, greedy)]
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
    use super::*;

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
        let opts: Opts = ALERT.parse_words("hello world at 4:20").unwrap();

        assert!(!opts.list);
        assert_eq!(opts.input(), "hello world at 4:20");
    }

    #[test]
    fn parses_empty_arguments() {
        let opts: Opts = ALERT.parse_words("").unwrap();

        assert!(!opts.list);
        assert_eq!(opts.input(), "");
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
