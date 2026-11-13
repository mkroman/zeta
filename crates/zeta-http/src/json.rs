//! JSON response parsing shared by the API client plugins.
//!
//! Bodies are parsed with [`serde_path_to_error`] so parse failures report the path to the
//! offending part of the document, and the offending body is logged.

use serde::de::DeserializeOwned;
use tracing::Level;
use zeta_utils::text::TruncatableExt;

use crate::response::LOGGED_BODY_LENGTH;

/// The error returned when a JSON body fails to parse.
pub type Error = serde_path_to_error::Error<serde_json::Error>;

/// Logs a failed JSON parse with the bounded body.
///
/// Outlined and non-generic: [`from_str_with_level`] is instantiated once per response type, and
/// this failure branch is identical for all of them. A tracing callsite needs a constant level,
/// so the dynamic one is mapped to the matching macro.
#[cold]
#[inline(never)]
fn log_parse_failure(text: &str, error: &Error, level: Level) {
    let body = text.truncate_within(LOGGED_BODY_LENGTH, "…");

    match level {
        Level::ERROR => tracing::error!(
            exception.message = %error,
            http.response.body = %body,
            "could not deserialize json response"
        ),
        Level::WARN => tracing::warn!(
            exception.message = %error,
            http.response.body = %body,
            "could not deserialize json response"
        ),
        Level::INFO => tracing::info!(
            exception.message = %error,
            http.response.body = %body,
            "could not deserialize json response"
        ),
        Level::DEBUG => tracing::debug!(
            exception.message = %error,
            http.response.body = %body,
            "could not deserialize json response"
        ),
        _ => tracing::trace!(
            exception.message = %error,
            http.response.body = %body,
            "could not deserialize json response"
        ),
    }
}

/// Deserializes a value of type `T` from a JSON string.
///
/// The offending body is logged at error level, truncated to a bounded length, if parsing fails.
///
/// # Errors
///
/// Returns the parse error, including the path to the offending part of the document, if
/// `text` is not valid JSON for `T`.
pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
    from_str_with_level(text, Level::ERROR)
}

/// Deserializes a value of type `T` from a JSON string, logging a failure at `level`.
///
/// [`from_str`] is the error-level entry point; a caller that treats a malformed response as
/// per-operation detail passes [`Level::DEBUG`]. The offending body is logged, truncated to a
/// bounded length.
///
/// # Errors
///
/// Returns the parse error, including the path to the offending part of the document, if
/// `text` is not valid JSON for `T`.
pub fn from_str_with_level<T: DeserializeOwned>(text: &str, level: Level) -> Result<T, Error> {
    let deserializer = &mut serde_json::Deserializer::from_str(text);

    serde_path_to_error::deserialize(deserializer)
        .inspect_err(|error| log_parse_failure(text, error, level))
}
