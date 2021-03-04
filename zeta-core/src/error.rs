use std::io;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    /// Indicates that the client has not been connected
    #[error("Client not initialized or connected")]
    ClientNotConnectedError,
    #[error("Could not resolve the hostname")]
    ResolveError(#[source] io::Error),
    #[error("Connection error")]
    ConnectionError(#[source] io::Error),
    #[error("TLS error")]
    TlsError(#[from] tokio_native_tls::native_tls::Error),
    #[error("Could not find a host to connect to")]
    ConnectionFailed,
}
