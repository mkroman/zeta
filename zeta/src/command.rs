//! IRC prefix command matching.
//!
//! Matches a static prefix against an IRC message and extracts the trailing arguments.
//!
//! # Example
//!
//! ```
//! use zeta::command::Prefix;
//!
//! const YT: Prefix = Prefix::new(".yt");
//!
//! assert_eq!(YT.parse(".yt"), Some(""));
//! assert_eq!(YT.parse(".yt rust"), Some("rust"));
//! assert_eq!(YT.parse(".youtube rust"), None);
//! assert_eq!(YT.parse(".goodbye"), None);
//! ```

/// A zero-sized prefix matcher for IRC bot commands.
///
/// Stores a `&'static str` prefix and provides [`parse`](Prefix::parse) to check
/// whether a message starts with the prefix and extract the trailing arguments.
///
/// Because the prefix is a static reference, `Prefix` is [`Copy`], requires no
/// heap allocation, and can be constructed in `const` context.
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

    /// Checks if the input starts with the command prefix, returning the trailing
    /// arguments (with leading whitespace stripped) if it matches.
    ///
    /// Returns `None` if the input does not start with the prefix, or if the
    /// character immediately following the prefix is not whitespace (i.e. it is
    /// part of a longer word).
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
}
