use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use log::{debug, info};
use tokio_stream::StreamExt;

mod channel;
pub mod config;
mod error;
mod user;

pub use channel::Channel;
pub use config::{Config, NetworkConfig};
pub use error::Error;
pub use user::User;

pub struct Network {
    config: NetworkConfig,
}

#[derive(Default)]
pub struct Core {
    networks: Vec<Network>,
    channels: HashMap<String, Arc<RwLock<Channel>>>,
    users: HashMap<String, Arc<RwLock<User>>>,
}

// const GIT_COMMIT_HASH: &str = include_str!(concat!(env!("OUT_DIR"), "/git_commit"));

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
            ..Default::default()
        }
    }

    /// Returns the number of networks.
    pub fn num_networks(&self) -> usize {
        self.networks.len()
    }

    /// Adds a new network to the core, automatically connecting and managing the connection
    pub fn add_network(&mut self, config: config::NetworkConfig) -> Result<(), Error> {
        let network = Network::from_config(config)?;

        self.networks.push(network);

        Ok(())
    }

    /// Continually polls for new IRC messages
    pub async fn poll(&mut self) -> Result<(), Error> {
        loop {}

        Ok(())
    }
}
