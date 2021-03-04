use crate::Error;

use log::{debug, error, info, trace, warn};
use rand::seq::SliceRandom;
use tokio::net::{self, TcpStream};

use tokio_native_tls::{native_tls, TlsStream};

/// Wraps an insecure [`TcpStream`] connection.
pub struct Connection(TcpStream);

/// Wraps a secure [`TlsStream`] connection.
pub struct TlsConnection(TlsStream<TcpStream>);

/// Attempts to resolve the given `host` and returns a list of addresses in random order on
/// success.
async fn resolve(host: &str, port: u16) -> Result<Vec<std::net::SocketAddr>, Error> {
    let mut addrs = net::lookup_host((host, port))
        .await
        .map_err(Error::ResolveError)?
        .collect::<Vec<_>>();

    // Shuffle the addresses in-place in case there's no round-robin DNS
    addrs.shuffle(&mut rand::thread_rng());

    Ok(addrs)
}

impl Connection {
    /// Opens an unencrypted connection to the given `host` on the given `port`.
    ///
    /// If the host is DNS hostname, this will attempt to resolve it and try to connect to the
    /// resolved addresses in random order.
    pub async fn connect(host: &str, port: u16) -> Result<Connection, Error> {
        trace!("Resolving host {}", host);

        let addrs = resolve(host, port).await?;

        trace!("Host resolved to {} addresses", addrs.len());

        for addr in addrs {
            debug!("Connecting to {}", &addr);

            let stream = match TcpStream::connect(&addr).await {
                Ok(stream) => stream,
                Err(e) => {
                    debug!("Could not connect to {}: {}", addr, e);

                    continue;
                }
            };

            info!("Connected to {}", &addr);

            return Ok(Connection(stream));
        }

        error!("Unable to connect to any of the resolved addresses");

        Err(Error::ConnectionFailed)
    }
}

impl TlsConnection {
    /// Opens an encrypted connection to the given `host` on the given `port`.
    ///
    /// If the host is DNS hostname, this will attempt to resolve it and try to connect to the
    /// resolved addresses in random order.
    pub async fn connect(host: &str, port: u16) -> Result<TlsConnection, Error> {
        trace!("Resolving host {}", host);

        let addrs = resolve(host, port).await?;

        trace!("Host resolved to {} addresses", addrs.len());

        for addr in addrs {
            debug!("Connecting to {}", &addr);

            let stream = match TcpStream::connect(&addr).await {
                Ok(stream) => stream,
                Err(e) => {
                    debug!("Could not connect to {}: {}", addr, e);

                    continue;
                }
            };

            info!("Connected to {}", &addr);

            trace!("Creating tls session");
            let cx = native_tls::TlsConnector::builder().build()?;
            let cx = tokio_native_tls::TlsConnector::from(cx);

            let stream = match cx.connect(host, stream).await {
                Ok(stream) => stream,
                Err(e) => {
                    warn!("Could not establish TLS connection with {}: {}", addr, e);
                    continue;
                }
            };

            return Ok(TlsConnection(stream));
        }

        error!("Unable to connect to any of the resolved addresses");

        Err(Error::ConnectionFailed)
    }
}
