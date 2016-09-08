// Copyright (c) 2016, Mikkel Kroman <mk@uplink.io>
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are met:
//
// * Redistributions of source code must retain the above copyright notice, this
//   list of conditions and the following disclaimer.
//
// * Redistributions in binary form must reproduce the above copyright notice,
//   this list of conditions and the following disclaimer in the documentation
//   and/or other materials provided with the distribution.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
// AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
// FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
// CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
// OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use std::fmt;
use std::any::Any;
use irc::client::data::Message;
use irc::client::server::IrcServer;

use semver::Version;
use ::libloading as lib;

pub mod prelude {
    pub use irc::client::data::{Message, Command};
    pub use irc::client::server::IrcServer;
    pub use irc::client::server::utils::ServerExt;
    pub use super::{Plugin, PluginDescription, PluginManager};
}

/// Thread-safe plugin instantiator and manager.
#[derive(Debug)]
pub struct PluginManager {
    plugins: Vec<Box<Plugin>>,
    library: Option<::libloading::Library>,
}

impl PluginManager {
    /// Create and return a new PluginManager with an empty list of plugins.
    pub fn new() -> PluginManager {
        PluginManager {
            plugins: vec![],
            library: None,
        }
    }

    /// Registers a new plugin of type T that implements Plugin.
    /// If the plugin is successfully registered, this will returns a box with the instance plugin.
    pub fn register<'a, T>(&'a mut self) -> Result<&Box<Plugin>, ()>
        where T: Plugin {
        let plugin = Box::new(T::new());

        self.plugins.push(plugin);

        // Is there a better way to get a reference to the boxed plugin once
        // has been moved?
        let plugin_ref = &self.plugins[self.plugins.len() - 1];

        Ok(plugin_ref)
    }

    /// Return a borrowed reference to the list of plugins.
    pub fn plugins(&self) -> &Vec<Box<Plugin>> {
        &self.plugins
    }

    /// Load the plugins library and register all plugins.
    pub fn load(&mut self) -> Result<(), ()> {
        let plugin_lib = lib::Library::new("libzeta_plugins.so").unwrap();

        unsafe {
            let register_plugins: lib::Symbol<extern fn(&mut PluginManager)> = 
                plugin_lib.get(b"register_plugins\0").unwrap();

            register_plugins(self);
        }

        self.library = Some(plugin_lib);

        println!("Plugins: {:?}", self.plugins);

        Ok(())
    }

    /// Unload any loaded plugins and close the handle to the plugins library.
    pub fn unload(&mut self) -> Result<(), ()> {
        self.plugins.clear();
        self.library = None;

        debug!("{:?}", self);

        Ok(())
    }

    /// Reload all plugins.
    pub fn reload(&mut self) -> Result<(), ()> {
        try!(self.unload());
        try!(self.load());

        Ok(())
    }
}

impl fmt::Debug for Plugin {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Plugin {{ name: {}, author: {}, version: {} }}", self.name(), self.authors(),
            self.version())
    }
}

/// This is the plugin trait that all plugins must implement.
pub trait Plugin: PluginDescription + Any + Send + Sync {
    fn new() -> Self where Self: Sized;

    /// Process an incoming IRC message.
    fn process(&self, _: &IrcServer, _: &Message) -> Result<(), ()> {
        Ok(())
    }
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
#[macro_export]
macro_rules! plugin {
    ( $t:ty, $n:expr, $v: expr, $d:expr, $($a:expr),+ ) => {
        use std::fmt;
        use $crate::semver::Version;
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

        impl fmt::Debug for $t {
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
    use irc::client::data::Message;
    use irc::client::server::IrcServer;

    struct SomePlugin;

    impl Plugin for SomePlugin {
        fn new() -> SomePlugin {
            SomePlugin
        }

        fn process(&self, _: &IrcServer, _: &Message) -> Result<(), ()> {
            Ok(())
        }
    }

    #[test]
    fn register_returns_ok() {
        plugin!(SomePlugin, "some_plugin", "1.0", "John Doe", "hello");

        let mut plugins = PluginManager::new();
        let result = plugins.register::<SomePlugin>();

        assert!(result.is_ok());
    }
}
