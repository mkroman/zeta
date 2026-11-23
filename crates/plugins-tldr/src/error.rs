//! Errors that can occur while summarizing.

use zeta_http::{ApiError, RequestError, reqwest};

/// Errors that can occur while summarizing a page.
///
/// [`Display`](std::fmt::Display) is reply-grade: [`RequestError`] and [`ApiError`] already
/// render reply-grade (and keep their operator detail in `source()`), and the own variants name
/// the condition without internal text. The reply a channel sees comes from [`Error::reply`].
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The request could not be sent, or the response body could not be read.
    #[error(transparent)]
    Request(#[from] RequestError),
    /// The endpoint answered with an error status, or an unparseable body.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// The response carried no summary markdown.
    #[error("kagi returned no summary")]
    NoOutput,
    /// A configured header value was invalid (the session token or accept-language).
    #[error("invalid header value")]
    InvalidHeader(#[from] reqwest::header::InvalidHeaderValue),
}

impl Error {
    /// The reply-grade sentence the channel sees for this error.
    ///
    /// A `401` names the token, because that is the one failure the invoker can fix; everything
    /// else falls back to the operation context. Never interpolated, so no source text or URL
    /// can leak into a reply.
    pub(crate) fn reply(&self) -> &'static str {
        match self {
            Self::Api(ApiError::Status { status, .. }) if *status == 401 => {
                "invalid kagi session token"
            }
            Self::NoOutput => "kagi returned no summary",
            Self::Request(_) | Self::Api(_) | Self::InvalidHeader(_) => {
                "could not summarize the page"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_is_a_literal_per_variant() {
        assert_eq!(Error::NoOutput.reply(), "kagi returned no summary");
        assert_eq!(
            Error::Api(ApiError::Status {
                status: reqwest::StatusCode::UNAUTHORIZED,
                body: "invalid token".to_owned(),
            })
            .reply(),
            "invalid kagi session token"
        );
        assert_eq!(
            Error::Api(ApiError::Status {
                status: reqwest::StatusCode::BAD_GATEWAY,
                body: "upstream".to_owned(),
            })
            .reply(),
            "could not summarize the page"
        );
    }
}
