//! A small JSON DOM, parser and serializer for the shapes xtask exchanges.
//!
//! Gungraun summaries and the GitHub payloads xtask builds only need strings, numbers,
//! arrays and objects. Keeping that subset here avoids compiling serde and its supporting
//! crates for a build tool while retaining the RFC 8259 syntax needed to read real
//! summaries (escapes, unicode, floats and exponents).
//!
//! The parser and serializer live in private submodules and are re-exported here, so the
//! rest of the crate only depends on the [`Value`] model and the parse/serialize functions.

mod parse;
mod write;

use std::collections::BTreeMap;
use std::fmt;

pub use parse::parse;
pub use write::{to_string, to_string_pretty};

/// A JSON object. A map keeps keys deterministic and sorted, matching the payloads the
/// previous serializer produced.
pub type Object = BTreeMap<String, Value>;

/// A parsed or constructed JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    U64(u64),
    I64(i64),
    F64(f64),
    String(String),
    Array(Vec<Self>),
    Object(Object),
}

impl Value {
    /// Build an object from key-value pairs.
    pub fn object<K, I>(entries: I) -> Self
    where
        K: Into<String>,
        I: IntoIterator<Item = (K, Self)>,
    {
        Self::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    /// Build an array from values.
    pub fn array<I: IntoIterator<Item = Self>>(items: I) -> Self {
        Self::Array(items.into_iter().collect())
    }

    /// Look up a member of an object.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(object) => object.get(key),
            _ => None,
        }
    }

    /// Insert a member into an object; other values are left unchanged.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Self>) {
        if let Self::Object(object) = self {
            object.insert(key.into(), value.into());
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::U64(value) => Some(*value),
            Self::I64(value) => u64::try_from(*value).ok(),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_object(&self) -> Option<&Object> {
        match self {
            Self::Object(object) => Some(object),
            _ => None,
        }
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Self::String(value.clone())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}

impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Self::U64(u64::try_from(value).unwrap_or(u64::MAX))
    }
}

impl<T: Into<Self>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}

impl<T: Into<Self>> From<Vec<T>> for Value {
    fn from(values: Vec<T>) -> Self {
        Self::Array(values.into_iter().map(Into::into).collect())
    }
}

/// A parse failure with the byte offset that caused it.
#[derive(Debug)]
pub struct Error {
    message: String,
    index: usize,
}

impl Error {
    fn new(message: impl Into<String>, index: usize) -> Self {
        Self {
            message: message.into(),
            index,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.message, self.index)
    }
}

impl std::error::Error for Error {}
