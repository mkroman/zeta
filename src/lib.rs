#![feature(associated_consts)]

pub extern crate irc;
pub extern crate semver;
pub extern crate env_logger;
#[macro_use] extern crate log;
extern crate libloading;
#[macro_use] extern crate quick_error;

use std::io;
use std::path::Path;
use std::cell::RefCell;
use irc::client::data::Config;
use irc::client::data::command::Command;
use irc::client::server::{Server, IrcServer};
use irc::client::server::utils::ServerExt;

pub mod plugin;
pub mod error;

use plugin::PluginManager;
use error::{Error, ConfigError};

/// Configuration data and internal state.
pub struct Zeta {
    server: Option<IrcServer>,
    config: Config,
    plugins: RefCell<PluginManager>,
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
            plugins: RefCell::new(PluginManager::new()),
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

    /// Run the main event-loop and share all incoming messages with all the
    /// initialized plugins.
    pub fn run(&mut self) -> Result<(), io::Error> {
        use std::{thread, time};

        let server = self.server.as_ref().unwrap();

        for message in server.iter() {
            match message {
                Ok(ref message) => {
                    println!("{:?}", message);

                    match message.command {
                        Command::PRIVMSG(_, ref msg) => {
                            if msg == ".load" {
                                self.plugins.borrow_mut().load().unwrap();
                            }
                            else if msg == ".reload" {
                                self.plugins.borrow_mut().reload().unwrap();
                            }
                            else if msg == ".unload" {
                                self.plugins.borrow_mut().unload().unwrap();
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

        Ok(())
    }

    pub fn load_plugins(&mut self) -> Result<(), ()> {
        self.plugins.borrow_mut().load()
    }
}
