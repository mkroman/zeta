// Copyright (c) 2015, Mikkel Kroman <mk@uplink.io>
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

use std::io;
use irc::client::data::Message;
use irc::client::data::command::Command;
use irc::client::server::IrcServer;

use server::Server;
use plugins::PluginManager;

/// Configuration data and internal state.
pub struct Zeta {
    server: Option<IrcServer>,
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
        plugins::register(&mut self.plugins);

        self
    }

    pub fn run(&self) -> Result<(), io::Error> {
        use irc::client::server::Server;

        let server = self.server.as_ref().unwrap();

        for message in server.iter() {
            match message {
                Ok(message) => {
                    for plugin in self.plugins.plugins() {
                        plugin.process(&server, &message);
                    }
                }
                Err(error) => {
                    println!("!! {}", error);
                }
            }
        }

        Ok(())
    }
}
