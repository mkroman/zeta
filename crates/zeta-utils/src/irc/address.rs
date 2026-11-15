//! Unwrapping a nickname mention from the start of a message.

/// Strips a nickname mention (`<nick>, ...` or `<nick>: ...`) from the start of `s`.
///
/// Plugins that react to being addressed use this to unwrap the mention and handle only the
/// text following it.
#[must_use]
pub fn strip_nick_prefix<'a>(s: &'a str, current_nickname: &str) -> Option<&'a str> {
    s.strip_prefix(current_nickname)
        .and_then(|s| s.strip_prefix(", ").or_else(|| s.strip_prefix(": ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_nick_prefix_borrows_only_the_message() {
        // The nickname is only compared against, so a temporary `String` can be passed and the
        // returned slice still borrows the message instead of the nickname.
        let message = String::from("zeta: hello");
        let stripped = strip_nick_prefix(&message, &String::from("zeta"));

        assert_eq!(stripped, Some("hello"));
    }
}
