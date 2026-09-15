use std::fmt;

use serde::{Deserialize, Serialize};

/// The settings type for plugins that have no configuration.
///
/// Used as the settings type of plugins that do not read any values from their
/// `[plugins.<name>]` configuration section. Only the host-managed `enabled` key is accepted;
/// any other key is rejected as a likely typo.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NoSettings {}

/// A newtype wrapper for plugin metadata strings.
macro_rules! metadata_type {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Eq, PartialEq, Hash)]
        pub struct $name(String);

        impl $name {
            /// Creates a new instance from a string.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Returns the inner string value.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&'static str> for $name {
            fn from(value: &'static str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

pub struct Metadata {
    /// Name of the plugin.
    pub name: Name,
    /// List of authors that maintains or contributes to the plugin.
    pub authors: Vec<Author>,
}

metadata_type!(Name, "Name of a plugin");
metadata_type!(Author, "Author of a plugin");
