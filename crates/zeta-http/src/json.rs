//! JSON response parsing shared by the API client plugins.
//!
//! Bodies are parsed with [`serde_path_to_error`] so parse failures report the path to the
//! offending part of the document, and the offending body is logged.

use serde::de::DeserializeOwned;
use tracing::error;
use zeta_utils::text::Truncatable;

use crate::LOGGED_BODY_LENGTH;

/// The error returned when a JSON body fails to parse.
pub type Error = serde_path_to_error::Error<serde_json::Error>;

/// Deserializes a value of type `T` from a JSON string.
///
/// The offending body is logged, truncated to a bounded length, if parsing fails.
///
/// # Errors
///
/// Returns the parse error, including the path to the offending part of the document, if
/// `text` is not valid JSON for `T`.
pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
    let deserializer = &mut serde_json::Deserializer::from_str(text);

    serde_path_to_error::deserialize(deserializer).inspect_err(|error| {
        let body = text.truncate_within(LOGGED_BODY_LENGTH, "…");

        error!(?error, body = %body, "could not deserialize json response");
    })
}
