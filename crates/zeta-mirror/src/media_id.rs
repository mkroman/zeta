//! Validated media ids.
//!
//! A media id ends up in downloaded file names, S3 object keys and the fragment of the public
//! link, so it must be safe to use in all three. [`MediaId`] is the validated form: it can only be
//! constructed through [`MediaId::parse`] (or its `FromStr` and `TryFrom` implementations), which
//! restrict it to the identifier alphabet.

use std::fmt;
use std::str::FromStr;

use zeta_url::is_identifier;

/// The characters a media id may contain in addition to ASCII alphanumerics.
const ALLOWED: &str = "_-";

/// A media id that is safe to use in file names, object keys and URL fragments.
///
/// Construct one with [`MediaId::parse`], [`MediaId::from_str`], or the `TryFrom` implementations;
/// there is no unchecked constructor.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MediaId(String);

impl MediaId {
    /// Parses and validates a media id.
    ///
    /// The accepted set is a non-empty string of ASCII alphanumerics plus `_` and `-`, matching
    /// the download manager's own check.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMediaId`] when the id is empty or contains any other character.
    pub fn parse(id: impl Into<String>) -> Result<Self, InvalidMediaId> {
        let id = id.into();

        // `is_identifier` already rejects the empty string; the explicit check keeps the
        // requirement visible here and preserves the accepted set if the helper changes.
        if id.is_empty() || !is_identifier(&id, ALLOWED) {
            return Err(InvalidMediaId(id));
        }

        Ok(Self(id))
    }

    /// Returns the id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MediaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for MediaId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for MediaId {
    type Err = InvalidMediaId;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        Self::parse(id)
    }
}

impl TryFrom<&str> for MediaId {
    type Error = InvalidMediaId;

    fn try_from(id: &str) -> Result<Self, Self::Error> {
        Self::parse(id)
    }
}

impl TryFrom<String> for MediaId {
    type Error = InvalidMediaId;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::parse(id)
    }
}

/// The error returned when a media id is empty or contains unsafe characters.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid media id: {0:?}")]
pub struct InvalidMediaId(String);

impl InvalidMediaId {
    /// Returns the rejected id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_accepts_identifier_characters() {
        for id in ["123", "pxtf7mx2xqzg1", "a-b_c", "ABC_123-xyz"] {
            let parsed = MediaId::parse(id).expect("the id should be valid");

            assert_eq!(parsed.as_str(), id);
            assert_eq!(parsed.to_string(), id);
            assert_eq!(parsed.as_ref(), id);
        }
    }

    #[test]
    fn test_parse_rejects_empty_and_unsafe_ids() {
        for id in ["", "../evil", "a/b", "a b", "ab%2Fcd", "a.b", "café"] {
            assert!(
                MediaId::parse(id).is_err(),
                "{id:?} should be rejected as a media id"
            );
        }
    }

    #[test]
    fn test_parse_rejects_non_identifier_characters() {
        let error = MediaId::parse("../evil").expect_err("the id is invalid");

        assert_eq!(error.as_str(), "../evil");
        assert_eq!(error.to_string(), "invalid media id: \"../evil\"");
    }

    #[test]
    fn test_try_from_matches_parse() {
        assert_eq!(MediaId::try_from("123").unwrap().as_str(), "123");
        assert_eq!(
            MediaId::try_from("123".to_string()).unwrap().as_str(),
            "123"
        );
        assert!(MediaId::try_from("").is_err());
        assert!(MediaId::try_from(String::from("../evil")).is_err());
    }
}
