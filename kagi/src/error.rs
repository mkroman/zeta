//! Error types for the Kagi client.

/// Errors that can occur while searching with Kagi.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The search request could not be sent.
    #[error("unable to send search request")]
    SearchRequest,
    /// The response body of the search request could not be read.
    #[error("could not read response body of search request")]
    SearchRequestBody,
    /// The nonce request could not be sent.
    #[error("could not send nonce request")]
    RequestNonce(#[source] reqwest::Error),
    /// The nonce response could not be read.
    #[error("could not read nonce response")]
    ReadNonce(#[source] reqwest::Error),
    /// The session request could not be sent.
    #[error("could not send session request")]
    RequestSession(#[source] reqwest::Error),
    /// The response did not include session cookies, indicating an invalid login token.
    #[error("response did not include session valid cookies - is the login token valid?")]
    SessionCookies,
    /// The response did not include a nonce.
    #[error("response did not include a nonce")]
    Nonce,
}
