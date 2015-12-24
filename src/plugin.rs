// Copyright (C) 2015 Mikkel Kroman <mk@maero.dk>
// All rights reserved.

use std::any::Any;
use irc::client::data::Command;
use irc::client::server::NetIrcServer;
use semver::Version;

pub mod prelude {
    pub use irc::client::data::{Message, Command};
    pub use irc::client::server::NetIrcServer;
    pub use irc::client::server::utils::ServerExt;
    pub use super::{Plugin, PluginDescription};
}


/// Plugin error type and their associated meanings.
pub enum PluginError {
    InternalError,
}

/// Thread-safe plugin instantiator and manager.
pub struct PluginManager {
    pub plugins: Vec<Box<Plugin>>,
}

impl PluginManager {
    /// Create and return a new PluginManager with an empty list of plugins.
    pub fn new() -> PluginManager {
        PluginManager {
            plugins: vec![]
        }
    }

    /// Register a new plugin of type T that implements Plugin.
    pub fn register<'a, T>(&'a mut self) -> Result<(), ()>
        where T: Plugin {
        let plugin = Box::new(T::new());

        self.plugins.push(plugin);

        // FIXME: Remove this or return something meaningful
        Ok(())
    }

    /// Return a borrowed reference to the list of plugins.
    pub fn plugins(&self) -> &Vec<Box<Plugin>> {
        &self.plugins
    }
}

/// This is the plugin trait that all plugins must implement.
pub trait Plugin: PluginDescription + Any + Send + Sync {
    fn new() -> Self where Self: Sized;
    fn process<'a>(&self, server: &'a NetIrcServer, cmd: &Command) -> Result<(), ()>;
}

pub struct Description {
    pub name: &'static str,
    pub authors: &'static str,
    pub version: &'static str,
    pub description: &'static str,
}

pub trait PluginDescription {
    /// The internal structure containing the plugins description.
    const DESCRIPTION: Description;

    /// Get a human-readable name for the plugin.
    fn name(&self) -> &str;
    /// Get a human-readable description of the plugin.
    fn description(&self) -> &str;
    /// Get a human-readable list of authors of the plugin.
    fn authors(&self) -> &str;
    /// Get the version of the plugin.
    fn version(&self) -> Version;
}

/// Macro helper for creating a new plugin.
/// 
/// # Examples
/// ```
/// plugin!(Context, "Google Search", "0.1", "Allows a user to search google", "Mikkel Kroman <mk@maero.dk>");
/// // Where Context implements Plugin.
/// ```
macro_rules! plugin {
    ( $t:ty, $n:expr, $v: expr, $d:expr, $($a:expr),+ ) => {
        use std::fmt;
        use semver::Version;
        use $crate::plugin::Description;

        impl PluginDescription for $t {
            const DESCRIPTION: Description = Description {
                name: $n,
                description: $d,
                authors: "d",
                version: $v,
            };

            /// Get the plugin name.
            fn name(&self) -> &str {
                Self::DESCRIPTION.name
            }

            /// Get the plugin description.
            fn description(&self) -> &str {
                Self::DESCRIPTION.description
            }

            /// Get the list of authors.
            fn authors(&self) -> &str {
                Self::DESCRIPTION.authors
            }

            /// Parse the version and return it as a semver::Version type.
            fn version(&self) -> Version {
                let version = Self::DESCRIPTION.version;

                match Version::parse(version) {
                    Ok(v) => v,
                    Err(_) => {
                        Version {
                            major: 0, minor: 0, patch: 0,
                            pre: vec![],
                            build: vec![]
                        }
                    }
                }
            }
        }

        impl fmt::Debug for Context {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "Plugin {{ name: {:?}, version: {}, author: {:?}, description: {:?} }}",
                       self.name(), self.version(), self.authors(), self.description())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
}
