#![feature(associated_consts)]

pub extern crate irc;
pub extern crate semver;
pub extern crate env_logger;
#[macro_use] extern crate log;
extern crate libloading as lib;
#[macro_use] extern crate quick_error;

use std::io;
use std::path::Path;
use irc::client::data::Config;
use irc::client::data::command::Command;
use irc::client::server::{Server, IrcServer};
use irc::client::server::utils::ServerExt;

pub mod plugin;
pub mod error;

use error::{Error, ConfigError};

/// Configuration data and internal state.
pub struct Zeta {
    server: Option<IrcServer>,
    config: Config,
    plugins: plugin::PluginManager,
    plugins_handle: Option<lib::Library>,
}

impl Zeta {
    /// Create and return a new instance of Zeta.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Zeta, Error> {
        let config = match Config::load(path) {
            Ok(config) => config,
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    return Err(ConfigError::NotFound.into());
                } else {
                    return Err(ConfigError::Io(e).into());
                }
            }
        };

        Ok(Zeta {
            server: None,
            config: config,
            plugins: plugin::PluginManager::new(),
            plugins_handle: None,
        })
    }

    /// Connect to the preconfigured IRC network.
    pub fn connect(&mut self) -> Result<(), io::Error> {
        self.server = Some(try!(IrcServer::from_config(self.config.clone())));

        if let Some(ref server) = self.server {
            try!(server.identify());
        }

        Ok(())
    }

    pub fn load_plugins(&mut self) -> Result<(), ()> {
        use lib::Symbol;

        let plugin_lib = lib::Library::new("libzeta_plugins.so").unwrap();

        unsafe {
            let register_plugins: Symbol<extern fn(&mut plugin::PluginManager)> = 
                plugin_lib.get(b"register_plugins\0").unwrap();

            register_plugins(&mut self.plugins);
        }

        self.plugins_handle = Some(plugin_lib);

        Ok(())
    }

    pub fn unload_plugins(&mut self) -> Result<(), ()> {
        self.plugins.clear();
        self.plugins_handle = None;

        Ok(())
    }

    pub fn reload_plugins(&mut self) -> Result<(), ()> {
        try!(self.unload_plugins());
        self.load_plugins();

        Ok(())
    }

    /// Run the main event-loop and share all incoming messages with all the
    /// initialized plugins.
    pub fn run(&mut self) -> Result<(), io::Error> {
        let server = self.server.as_ref().unwrap();

        self.process_commands(server);

        Ok(())
    }

    fn process_commands(&mut self, server: &IrcServer) {
        for message in server.iter() {
            match message {
                Ok(ref message) => {
                    println!("{:?}", message);

                    match message.command {
                        Command::PRIVMSG(ref target, ref msg) => {
                            if msg == ".reload" {
                                self.reload_plugins();
                            }
                        },
                        _ => {}
                    }
                }
                Err(e) => {
                    println!("!! {}", e);
                }
            }
        }

    }
}
