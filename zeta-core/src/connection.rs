use log::trace;
use tokio::net::{self, TcpStream, ToSocketAddrs};

use crate::config::NetworkConfig;

/// A connection to a remote server - this can be either an unencrypted connection or a TLS
/// encrypted connection
pub struct Connection<'a> {
    host: &'a str,
    port: u16,
}

impl Connection {
    /// Creates a new unconnected connection with a target `host` and `port`
    pub fn new(host: &str, port: u16) -> Connection {}

    pub async fn connect(&self) -> Result<TcpStream, Error> {
        trace!("Connecting to");
    }
}
