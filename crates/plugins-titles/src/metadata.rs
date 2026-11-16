//! Page metadata extraction.
//!
//! Fetches a page, streams its response through a push-based HTML tokenizer, and captures the
//! title and OpenGraph metadata of the document head. Tokenization stops as soon as the head has
//! ended, aborting the rest of the download.
//!
//! The module is split by concern: `sink` captures the document head as it is tokenized, `fetch`
//! streams the response through it and decodes the chunks, and `tests` covers both. This module
//! holds the [`PageMetadata`] model and the fetch [`Error`].

mod fetch;
mod sink;
#[cfg(test)]
mod tests;

use thiserror::Error;

use zeta_http::emulated::ResponseError;
use zeta_http::wreq;

pub(crate) use self::fetch::fetch_metadata;

/// Page title and OpenGraph metadata extracted from a document head.
#[derive(Debug, Default)]
pub(crate) struct PageMetadata {
    /// The contents of the first `<title>` element.
    pub title: Option<String>,
    /// The `content` of the first `og:title` meta tag.
    pub og_title: Option<String>,
    /// The `content` of the first `og:site_name` meta tag.
    pub site_name: Option<String>,
    /// The `content` of the first `og:description` meta tag.
    pub description: Option<String>,
}

/// Errors that can occur while fetching a page.
#[derive(Debug, Error)]
pub(crate) enum Error {
    /// The page request failed or returned an unsuccessful status.
    #[error(transparent)]
    Response(#[from] ResponseError),
    /// The tokenizer task failed.
    #[error("could not tokenize the response")]
    Task(#[from] tokio::task::JoinError),
}

impl From<wreq::Error> for Error {
    fn from(error: wreq::Error) -> Self {
        Self::Response(ResponseError::from(error))
    }
}
