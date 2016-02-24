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
use irc::client::server::IrcServer;
use irc::client::server::Server as IrcSrv;
use irc::client::server::utils::ServerExt;
use irc::client::data::config::Config;

/// Server builder structure.
pub struct Server<'a> {
    pub hostname: String,
    pub password: Option<&'a str>,
    pub port: u16,
    pub channels: Option<Vec<&'a str>>,
    pub nickname: &'a str,
    pub username: Option<&'a str>,
    pub realname: Option<&'a str>,
    pub use_ssl: bool,
}

impl<'a> Server<'a> {
    /// Create a new server with an initial hostname and port.
    pub fn new(host: &'a str, port: u16) -> Server<'a> {
        Server {
            hostname: host.to_string(),
            port: port,
            password: None,
            channels: None,
            username: None,
            realname: None,
            nickname: "zeta",
            use_ssl: false,
        }
    }

    /// Set the server password.
    pub fn password(&'a mut self, pass: Option<&'a str>) -> &'a mut Server {
        self.password = pass;
        self
    }

    /// Add a channel to automatically enter once the server connection is established.
    pub fn channel(&'a mut self, name: &'a str) -> &'a mut Server {
        match self.channels {
            Some(ref mut channels) => channels.push(name),
            None => self.channels = Some(vec![name])
        }

        self
    }

    /// Set whether or not to use SSL/TLS.
    pub fn ssl(&'a mut self, use_ssl: bool) -> &'a mut Server {
        self.use_ssl = use_ssl;
        self
    }

    /// Set the clients real name.
    pub fn real_name(&'a mut self, name: &'a str) -> &'a mut Server {
        self.realname = Some(name);
        self
    }

    /// Set the clients nickname.
    pub fn nick(&'a mut self, nick: &'a str) -> &'a mut Server {
        self.nickname = nick;
        self
    }

    /// Attempt to connect to the server, and return the server instance on success, otherwise
    /// return io::Error.
    pub fn connect(&'a mut self) -> Result<IrcServer, io::Error> {
        let channels = match self.channels {
            Some(ref channels) => Some(channels.iter().map(|s| s.to_string()).collect()),
            None => None 
        };

        let config = Config {
            server: Some(self.hostname.to_string()),
            port: Some(self.port),
            owners: Some(vec![format!("mk!mk@uplink.io")]),
            channels: channels,
            encoding: Some(format!("UTF-8")),
            nickname: Some(self.nickname.to_string()),
            username: self.username.and_then(|username| Some(username.to_string())),
            realname: self.realname.and_then(|realname| Some(realname.to_string())),
            use_ssl: Some(self.use_ssl), // Why is this an Option<bool>?
            .. Default::default()
        };

        let server = match IrcServer::from_config(config) {
            Ok(server) => server,
            Err(error) => return Err(error)
        };

        // Send NICK, USER and end capability negotiations.
        try!(server.identify());

        Ok(server)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

}
