//! Client building for anti-bot-protected sites.
//!
//! These are fetched with [`wreq`] browser emulation: plain `reqwest` gets TLS-fingerprinted by
//! them regardless of headers. Built behind the `emulated` feature, which the anti-bot-protected
//! plugins enable.

use wreq::header::{ACCEPT_ENCODING, HeaderMap, HeaderValue, InvalidHeaderValue, USER_AGENT};

use crate::WreqError;

/// A failed request through the browser-emulated client.
///
/// The transport variant wraps [`WreqError`], which redacts the request URI: these pages can
/// be fetched through a metadata proxy whose URL carries a credential in its query.
#[derive(Debug, thiserror::Error)]
pub enum ResponseError {
    /// The request failed, or its body could not be read.
    #[error("request error: {0}")]
    Request(#[from] WreqError),
    /// The server responded with an unsuccessful status code.
    #[error("request failed with status {0}")]
    Status(wreq::StatusCode),
}

impl From<wreq::Error> for ResponseError {
    fn from(error: wreq::Error) -> Self {
        Self::Request(error.into())
    }
}

/// Returns a `wreq` client builder emulating Firefox 142, layered with the headers the
/// emulation must not omit.
///
/// `Accept-Encoding` must be set explicitly: like reqwest, wreq does not advertise the
/// header itself even though it decompresses responses — and its absence is enough to get
/// flagged.
///
/// The profile's user agent is overridden with `user_agent` when given. This skews with the
/// emulated Firefox 142 fingerprint, and that is deliberate: the profile defaults to macOS —
/// which anti-bot systems score far more aggressively when requests originate from
/// datacenter networks — and the matching Linux Firefox 142 user agent is rejected by
/// DataDome outright, while Firefox 151 passes.
///
/// # Errors
///
/// Returns an error if `user_agent` cannot be used as a header value.
pub fn builder(user_agent: Option<&str>) -> Result<wreq::ClientBuilder, InvalidHeaderValue> {
    let mut headers = HeaderMap::new();

    headers.insert(
        ACCEPT_ENCODING,
        HeaderValue::from_static("gzip, deflate, br, zstd"),
    );

    if let Some(user_agent) = user_agent {
        let value = HeaderValue::from_str(user_agent)?;

        headers.insert(USER_AGENT, value);
    }

    Ok(wreq::Client::builder()
        .emulation(wreq_util::Emulation::Firefox142)
        .default_headers(headers))
}

/// Tests for the `wreq` wrapper: `emulated`-only builds (the titles plugin) reach them without
/// the `http` feature that gates the `send` and `parse_response` tests.
#[cfg(test)]
mod tests {
    use zeta_test_support::{assert_redactions, refused_address};

    #[tokio::test]
    async fn emulated_request_errors_redact_the_uri_but_keep_the_cause() {
        let address = refused_address();
        let error = wreq::Client::builder()
            .build()
            .expect("wreq client")
            .get(format!("http://{address}/?key=secret"))
            .send()
            .await
            .expect_err("nothing listens on that address");

        // Sanity: wreq attaches the request URI to the error — redacting it is on us.
        assert!(error.uri().is_some(), "wreq attaches the request uri");

        let error = crate::WreqError::from(error);
        let full = error.full();

        assert_redactions(&error.to_string(), full, &format!("{error:?}"), &address);

        // `wreq` renders its first source itself; walking the chain again would print that
        // link twice.
        assert_eq!(full.matches("client error").count(), 1, "{full}");

        assert!(std::error::Error::source(&error).is_none(), "{full}");
    }
}
