use std::fmt;

use crate::{EventType, UrlFilter};

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

/// Plugin descriptor that contains metadata about a plugin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Descriptor {
    /// Name of the plugin.
    pub name: Name,
    /// List of authors of the plugin.
    pub authors: Vec<Author>,
    /// List of events that the plugin cares about.
    pub events: Vec<EventType>,
    /// List of URL events that the plugin wants to receive.
    pub url_matchers: Vec<UrlFilter>,
}

pub struct Metadata {
    /// Name of the plugin.
    pub name: Name,
    /// List of authors that maintains or contributes to the plugin.
    pub authors: Vec<Author>,
}

metadata_type!(Name, "Name of a plugin");
metadata_type!(Author, "Author of a plugin");

impl Descriptor {
    pub fn new(name: impl Into<Name>) -> Descriptor {
        let name = name.into();

        Descriptor {
            name,
            authors: vec![],
            events: vec![],
            url_matchers: vec![],
        }
    }

    /// Adds the given `author` to the list of authors of this plugin.
    pub fn author(mut self, author: impl Into<Author>) -> Descriptor {
        self.authors.push(author.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_should_have_authors() {
        let descriptor = Descriptor::new("my_plugin")
            .author("John Doe <john.doe@example.com>")
            .author("Jane Doe <jane.doe@example.com>");

        assert_eq!(
            &descriptor.authors,
            &vec![
                Author::from("John Doe <john.doe@example.com>"),
                Author::from("Jane Doe <jane.doe@example.com>")
            ]
        );
    }
}
