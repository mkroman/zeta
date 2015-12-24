// Copyright (C) 2015 Mikkel Kroman <mk@maero.dk>
// All rights reserved.

use std::io;
use irc::client::server::{IrcServer, NetIrcServer};

use server::Server;
use plugin::PluginManager;
use plugins;

/// Configuration data and internal state.
pub struct Zeta {
    server: Option<NetIrcServer>,
    plugins: PluginManager,
}

impl Zeta {
    /// Create and return a new instance of Zeta.
    pub fn new() -> Zeta {
        Zeta {
            server: None,
            plugins: PluginManager::new(),
        }
    }

    /// Connect to the network.
    pub fn connect(&mut self) -> Result<(), io::Error> {
        self.server = Some(Server::new("irc.uplink.io", 6667)
                                      .ssl(true)
                                      .channel("#test")
                                      .connect()
                                      .unwrap());

        Ok(())
    }

    pub fn initialize_plugins(&mut self) -> &mut Zeta {
        self.plugins.register::<plugins::google_search::Context>().unwrap();
        self
    }

    pub fn run(&self) -> Result<(), io::Error> {
        use irc::client::server::Server;

        if let Some(ref server) = self.server {
            for cmd in server.iter_cmd() {
                match cmd {
                    Ok(cmd) => {
                        for plugin in self.plugins.plugins() {
                            plugin.process(&server, &cmd);
                        }
                        println!("{:?}", cmd);
                    },
                    Err(_) => {}
                }
            }
        }

        Ok(())
    }
}
