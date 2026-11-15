//! Locating quoted string literals.

/// Splits a leading string literal quoted with `quote` from `input`.
///
/// Returns the literal including its surrounding quotes and the input following it. The literal
/// ends at the first unescaped `quote`, so backslash escapes such as `\"` are skipped.
///
/// Supported quotes are `"` and `'`, which share JavaScript's backslash escaping rules. Template
/// literals are not supported because `${…}` interpolation needs different parsing.
///
/// A double-quoted literal can be passed to `serde_json::from_str` when the escaped contents are
/// needed; this helper only locates the literal.
///
/// # Examples
///
/// ```
/// # use zeta_utils::text::quoted_string_literal;
/// assert_eq!(
///     quoted_string_literal(r#""nonce" rest"#, '"'),
///     Some((r#""nonce""#, " rest"))
/// );
/// assert_eq!(
///     quoted_string_literal(r"'nonce' rest", '\''),
///     Some((r"'nonce'", " rest"))
/// );
/// ```
#[must_use]
pub fn quoted_string_literal(input: &str, quote: char) -> Option<(&str, &str)> {
    if !matches!(quote, '"' | '\'') {
        return None;
    }

    let body = input.strip_prefix(quote)?;
    let mut escaped = false;

    for (index, character) in body.char_indices() {
        match (escaped, character) {
            (true, _) => escaped = false,
            (false, '\\') => escaped = true,
            (false, character) if character == quote => {
                let end = index + 2;
                return Some((&input[..end], &input[end..]));
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_string_literals() {
        // The literal includes its quotes, and the remainder follows it.
        assert_eq!(
            quoted_string_literal(r#""abc" rest"#, '"'),
            Some((r#""abc""#, " rest"))
        );
        assert_eq!(quoted_string_literal(r#""""#, '"'), Some((r#""""#, "")));
        assert_eq!(
            quoted_string_literal(r"'abc' rest", '\''),
            Some((r"'abc'", " rest"))
        );

        // Escaped quotes do not terminate the literal.
        assert_eq!(
            quoted_string_literal(r#""a\"b" rest"#, '"'),
            Some((r#""a\"b""#, " rest"))
        );
        assert_eq!(
            quoted_string_literal(r"'a\'b' rest", '\''),
            Some((r"'a\'b'", " rest"))
        );

        // An escaped backslash does not escape the closing quote.
        assert_eq!(
            quoted_string_literal(r#""a\\" rest"#, '"'),
            Some((r#""a\\""#, " rest"))
        );

        assert_eq!(quoted_string_literal("abc", '"'), None);
        assert_eq!(quoted_string_literal("", '"'), None);
        assert_eq!(quoted_string_literal(r#""abc"#, '"'), None);
        assert_eq!(quoted_string_literal(r#""a\"#, '"'), None);

        // Template literals and other delimiters are intentionally unsupported.
        assert_eq!(quoted_string_literal("`abc` rest", '`'), None);
    }
}
