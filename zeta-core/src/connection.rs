use crate::Error;

use rand::seq::SliceRandom;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::net::{self, TcpStream};
use tokio::sync::mpsc;
use tokio_native_tls::{native_tls, TlsStream};
use tracing::{debug, error, info, instrument, trace, warn, Instrument};

/// Attempts to resolve the given `host` and returns a list of addresses in random order on
/// success.
#[instrument]
async fn resolve(host: &str, port: u16) -> Result<Vec<std::net::SocketAddr>, Error> {
    let mut addrs = net::lookup_host((host, port))
        .await
        .map_err(Error::HostnameResolutionFailed)?
        .collect::<Vec<_>>();

    // Shuffle the addresses in-place in case there's no round-robin DNS
    addrs.shuffle(&mut rand::thread_rng());

    Ok(addrs)
}

#[derive(Debug)]
pub enum Connection {
    /// Wraps an insecure [`TcpStream`] connection.
    Plain(TcpStream),
    /// Wraps a secure [`TlsStream`] connection.
    Secure(TlsStream<TcpStream>),
}

pub struct Transport<T>
where
    T: AsyncWriteExt,
{
    inner: T,
}

impl Connection {
    /// Splits the connection into a mpsc sender and receiver of type `T`.
    pub fn split<T>(self) -> Result<(), Error> {
        let (tx, mut rx) = mpsc::unbounded_channel::<&[u8]>();
        let (tx2, mut rx2) = mpsc::unbounded_channel::<&[u8]>();

        trace!("Splitting socket");

        match self {
            Connection::Plain(conn) => {
                let (mut read, write) = tokio::io::split(conn);

                tx.send(b"NICK Hello\r\n").unwrap();
                tx.send(b"USER Hello hello hello hello\r\n").unwrap();

                tokio::spawn(
                    async move {
                        let mut writer = BufWriter::new(write);

                        while let Some(data) = rx.recv().await {
                            trace!(?data, "writing data");

                            writer.write(data).await.unwrap();
                            writer.flush().await.unwrap();
                        }

                        trace!("writer died");
                    }
                    .instrument(tracing::trace_span!("reader_task")),
                );

                tokio::spawn(async move {
                    while let Ok(data) = read.read_u8().await {
                        trace!(?data, "received data");
                    }

                    trace!("reader died");
                });
            }
            _ => unimplemented!(),
        }

        Ok(())
    }
}

impl Connection {
    /// Opens an unencrypted connection to the given `host` on the given `port`.
    ///
    /// If the host is DNS hostname, this will attempt to resolve it and try to connect to the
    /// resolved addresses in random order.
    #[instrument]
    pub async fn connect(host: &str, port: u16) -> Result<Connection, Error> {
        trace!("Resolving hostname");

        let addrs = resolve(host, port).await?;

        trace!(?addrs);

        for addr in &addrs {
            debug!(%addr, "Opening connection");

            let stream = match TcpStream::connect(&addr).await {
                Ok(stream) => stream,
                Err(err) => {
                    debug!(%addr, ?err, "Connection failed");

                    continue;
                }
            };

            info!(%addr, "Connection established");

            return Ok(Connection::Plain(stream));
        }

        error!(?addrs, "Unable to connect to any of the resolved addresses");

        Err(Error::ConnectionFailed)
    }

    /// Opens an encrypted connection to the given `host` on the given `port`.
    ///
    /// If the host is DNS hostname, this will attempt to resolve it and try to connect to the
    /// resolved addresses in random order.
    #[instrument]
    pub async fn connect_secure(host: &str, port: u16) -> Result<Connection, Error> {
        trace!("Resolving hostname");

        let addrs = resolve(host, port).await?;

        trace!(?addrs);

        for addr in &addrs {
            debug!(%addr, "Opening connection");

            let stream = match TcpStream::connect(&addr).await {
                Ok(stream) => stream,
                Err(err) => {
                    debug!(%addr, ?err, "Connection failed");

                    continue;
                }
            };

            info!(%addr, "Connection established");
            trace!(%addr, "Creating TLS session");

            let cx = native_tls::TlsConnector::builder().build()?;
            let cx = tokio_native_tls::TlsConnector::from(cx);

            let stream = match cx.connect(host, stream).await {
                Ok(stream) => stream,
                Err(err) => {
                    warn!(?err, %addr, "Could not establish TLS connection");
                    continue;
                }
            };

            trace!(?stream, "TLS connection established");

            return Ok(Connection::Secure(stream));
        }

        error!(?addrs, "Unable to connect to any of the resolved addresses");

        Err(Error::ConnectionFailed)
    }
}
