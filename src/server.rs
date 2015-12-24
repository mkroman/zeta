// Copyright (C) 2015 Mikkel Kroman <mk@maero.dk>
// All rights reserved.

use std::io;
use irc::client::server::{IrcServer, NetIrcServer};
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
    pub fn connect(&'a mut self) -> Result<NetIrcServer, io::Error> {
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
        if let Err(error) = server.identify() {
            return Err(error);
        }

        Ok(server)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it_should_add_channel() {
        let server = Server::new("irc.test.org", 6667).channel("#test").connect();
    }
}
