use slab::Slab;
use tracing::{debug, trace};

mod channel;
pub mod config;
mod connection;
mod error;
mod user;

pub use channel::Channel;
pub use config::{Config, NetworkConfig};
pub use connection::Connection;
pub use error::Error;
pub use user::User;

/// The maximum number of connections to have active at once.
pub const NUM_MAX_CONNECTIONS: usize = 32;

pub struct Network {
    config: NetworkConfig,
}

#[derive(Default)]
pub struct Core {
    networks: Slab<Network>,
    // channels: HashMap<String, Arc<RwLock<Channel>>>,
    // users: HashMap<String, Arc<RwLock<User>>>,
}

impl Network {
    /// Constructs a new Network based on the given network `config`
    pub fn from_config(config: config::NetworkConfig) -> Result<Network, Error> {
        let network = Network { config };

        Ok(network)
    }
}

impl Core {
    /// Creates a new core reactor
    pub fn new() -> Core {
        Core {
            networks: Slab::with_capacity(NUM_MAX_CONNECTIONS),
        }
    }

    /// Returns the number of networks.
    pub fn num_networks(&self) -> usize {
        self.networks.len()
    }

    /// Adds a new network to the core, automatically connecting and managing the connection
    pub fn add_network(&mut self, config: config::NetworkConfig) -> Result<(), Error> {
        let network = Network::from_config(config)?;
        let network_id = self.networks.insert(network);

        debug!(?network_id);

        Ok(())
    }

    /// Continually polls for new IRC messages
    pub async fn poll(&mut self) -> Result<(), Error> {
        for (id, network) in &self.networks {
            let url = &network.config.url;

            trace!(%id, "Creating connection to network {}", &url);

            let host = url.host_str().unwrap_or("");
            let port = url.port().unwrap_or(6667);

            let connection = if url.scheme().eq_ignore_ascii_case("ircs") {
                Connection::connect_secure(host, port).await?
            } else {
                Connection::connect(host, port).await?
            };

            connection.split::<u64>();

            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            //let (tx, mut rx) = mpsc::channel(32);
        }

        trace!("Done connecting to networks");

        Ok(())
    }
}
