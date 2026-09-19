use thiserror::Error;

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
    BuildClient(#[source] reqwest::Error),
    /// Represents an error that occurred while sending an HTTP request or reading its body.
    ///
    /// This could be due to a network issue, a timeout, or other `reqwest` internal errors.
    /// This variant is only available when the `client` feature is enabled.
    #[cfg(feature = "client")]
    #[error("request error: {0}")]
    Request(#[source] reqwest::Error),
    /// The server responded with a non-success status code, e.g. `404 Not Found`.
    #[cfg(feature = "client")]
    #[error("the server responded with status {0}")]
    Status(reqwest::StatusCode),
    /// Indicates that a required HTML element could not be found during parsing.
    ///
    /// This error is returned when a CSS selector does not match any element in the document,
    /// preventing the extraction of necessary data. The contained `String` provides context about
    /// what element was being sought.
    #[error("could not find element using selector: {0}")]
    MissingElement(String),
}
