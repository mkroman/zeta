//! The error type a failed command argument parse returns.
//!
//! [`ArgsError`] keeps the two failure modes apart: tokenization that never reached `clap`
//! ([`ArgsError::Quoting`]) and a rejected parse whose rendered usage or help output is replied
//! to verbatim ([`ArgsError::Usage`]). Both carry no internal detail, so the error is cheap to
//! clone, compare, and render into a channel.

use thiserror::Error;

/// An error returned when command arguments could not be parsed.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArgsError {
    /// The arguments could not be tokenized (e.g. unbalanced quotes).
    #[error("could not parse arguments")]
    Quoting,
    /// The arguments were rejected; the string contains the generated usage or help output.
    #[error("{0}")]
    Usage(String),
}
