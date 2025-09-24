use thiserror::Error;

/// Error.
#[derive(Debug, Error)]
pub enum Error {
    #[cfg(feature = "client")]
    #[error("could not construct http client: {0}")]
    BuildClient(#[source] reqwest::Error),
    #[cfg(feature = "client")]
    #[error("request error: {0}")]
    Request(#[source] reqwest::Error),
    #[cfg(feature = "client")]
    #[error("could not find element using selector: {0}")]
    MissingElement(String),
}
