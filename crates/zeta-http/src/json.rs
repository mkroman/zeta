//! JSON response parsing shared by the API client plugins.
//!
//! Parse failures report the offending line and column, and the offending body is logged.

use serde::de::DeserializeOwned;
use tracing::error;
use zeta_utils::text::Truncatable;

use crate::response::LOGGED_BODY_LENGTH;

/// The error returned when a JSON body fails to parse.
pub type Error = serde_json::Error;

/// Deserializes a value of type `T` from a JSON string.
///
/// The offending body is logged, truncated to a bounded length, if parsing fails.
///
/// # Errors
///
/// Returns the parse error, including the line and column of the offending part of the
/// document, if `text` is not valid JSON for `T`.
pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
    serde_json::from_str(text).inspect_err(|error| {
        let body = text.truncate_within(LOGGED_BODY_LENGTH, "…");

        error!(
            exception.message = %error,
            http.response.body = %body,
            "could not deserialize json response"
        );
    })
}
