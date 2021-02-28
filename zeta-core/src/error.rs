use std::io;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    /// Indicates that the client has not been connected
    #[error("Client not initialized or connected")]
    ClientNotConnected,
    #[error("Could not resolve the hostname")]
    HostnameResolutionFailed(#[source] io::Error),
    #[error("Connection error")]
    ConnectionError(#[source] io::Error),
    #[error("TLS error")]
    TlsError(#[from] tokio_native_tls::native_tls::Error),
    #[error("Could not find a host to connect to")]
    ConnectionFailed,
    #[error("Could not add additional network - the current implentation only supports 1 network")]
    NetworkLimitError,
}
