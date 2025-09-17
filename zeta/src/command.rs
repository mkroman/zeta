//! User command parsing

/// Simple prefix command parser.
///
/// This is useful when you want to extract a command and some arguments from a users message.
///
/// # Example
///
/// ```rust
/// use zeta::command::Command;
/// let command = Command::new(".hello");
/// assert_eq!(command.parse(".hello"), Some(""));
/// assert_eq!(command.parse(".hello world"), Some("world"));
/// assert_eq!(command.parse(".goodbye world"), None);
/// ```
pub struct Command {
    /// The prefix to match against.
    prefix: String,
}

impl Command {
    /// Creates a new prefix command parser that expects the given prefix.
    #[must_use]
    pub fn new(prefix: &str) -> Command {
        Command {
            prefix: prefix.to_string(),
        }
    }

    /// Checks if the supplied input starts with the command prefix, and if so, returns a string
    /// slice that makes up the arguments, if any.
    #[must_use]
    pub fn parse<'a>(&self, input: &'a str) -> Option<&'a str> {
        if let Some(suffix) = input.strip_prefix(&self.prefix) {
            if suffix.chars().nth(0) == Some(' ') {
                return Some(&suffix[1..]);
            }

            return Some("");
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extracts_args() {
        let command = Command::new("!test");

        assert_eq!(command.parse("!test --help"), Some("--help"));
    }

    #[test]
    fn parse_command_is_some() {
        let command = Command::new("!test");

        assert_eq!(command.parse("!test"), Some(""));
    }

    #[test]
    fn parse_preserves_whitespace() {
        let command = Command::new("!test");

        assert_eq!(command.parse("!test   --help"), Some("  --help"));
    }
}
