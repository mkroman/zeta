//! Error types for the Danish Dictionary crate.

use thiserror::Error;

/// Re-export of the shared transport error, which redacts the request URL before anything
/// formats it.
#[cfg(feature = "client")]
pub use zeta_http::RequestError;

/// The primary error type for this crate.
///
/// This enum consolidates all possible failures, including I/O, HTTP request issues, and HTML
/// parsing errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Occurs when the HTTP client fails to be constructed.
    ///
    /// This error is typically raised by `reqwest` during client initialization and is only
    /// available when the `client` feature is enabled.
    #[cfg(feature = "client")]
    #[error("could not construct http client: {0}")]
    BuildClient(#[source] RequestError),
    /// Represents an error that occurred while sending an HTTP request or reading its body.
    ///
    /// This could be due to a network issue, a timeout, or other `reqwest` internal errors.
    /// This variant is only available when the `client` feature is enabled.
    #[cfg(feature = "client")]
    #[error("request error: {0}")]
    Request(#[source] RequestError),
    /// The server responded with a non-success status code, e.g. `404 Not Found`.
    #[cfg(feature = "client")]
    #[error("the server responded with status {0}")]
    Status(reqwest::StatusCode),
    /// The dictionary page could not be parsed.
    ///
    /// Parsing runs on a blocking thread; this reports that task failing — e.g. a panic in the
    /// HTML parser — rather than a page that merely lacks the expected structure.
    #[cfg(feature = "client")]
    #[error("could not parse the dictionary page: {0}")]
    Parse(String),
    /// Indicates that a required HTML element could not be found during parsing.
    ///
    /// This error is returned when a CSS selector does not match any element in the document,
    /// preventing the extraction of necessary data. The contained `String` provides context about
    /// what element was being sought.
    #[error("could not find element using selector: {0}")]
    MissingElement(String),
}

impl Error {
    /// Returns `true` when the dictionary has no entry for the queried word.
    ///
    /// A missing page element and a `404 Not Found` response both mean the word has no entry;
    /// every other error is a real failure.
    #[cfg(feature = "client")]
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        match self {
            Self::MissingElement(_) => true,
            Self::Status(status) => *status == reqwest::StatusCode::NOT_FOUND,
            _ => false,
        }
    }
}

#[cfg(all(test, feature = "client"))]
mod tests {
    use reqwest::StatusCode;

    use super::*;

    #[test]
    fn is_not_found_matches_a_missing_entry() {
        assert!(Error::MissingElement("head".to_owned()).is_not_found());
    }

    #[test]
    fn is_not_found_matches_a_404() {
        assert!(Error::Status(StatusCode::NOT_FOUND).is_not_found());
        assert!(!Error::Status(StatusCode::INTERNAL_SERVER_ERROR).is_not_found());
    }

    #[test]
    fn is_not_found_rejects_internal_errors() {
        assert!(!Error::Parse("boom".to_owned()).is_not_found());
    }
}
