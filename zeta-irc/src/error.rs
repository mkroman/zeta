use std::{error, fmt};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// Indicates that there was an error during parsing.
    ///
    /// The `usize` is the byte offset in the input stream where parsing failed.
    ParseError(usize),
    /// Indicates that the length of the input is longer than is supported by the IRC protocol
    LengthError,
    /// Indicates that there was an encoding error, typically during message tags parsing as that
    /// is the only part of the spec that defines a specific encoding
    EncodingError(std::str::Utf8Error),
}

impl Error {
    /// Returns whether this is a `ParseError`
    pub fn is_parse_error(&self) -> bool {
        match *self {
            Error::ParseError(_) => true,
            _ => false,
        }
    }

    /// Returns true if this is an encoding error
    pub fn is_encoding_error(&self) -> bool {
        match *self {
            Error::EncodingError(_) => true,
            _ => false,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::ParseError(size) => write!(f, "failed to parse message at byte offset {}", size),
            Error::LengthError => write!(f, "the length exceeds what is supported by the protocol"),
            Error::EncodingError(ref err) => write!(f, "encoding error: {}", err),
        }
    }
}

impl error::Error for Error {}

impl From<std::str::Utf8Error> for Error {
    fn from(err: std::str::Utf8Error) -> Error {
        Error::EncodingError(err)
    }
}
