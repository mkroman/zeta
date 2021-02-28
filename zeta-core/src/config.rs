//! This module implements serializable and deserializable structs that represent the configuration
//! of the client

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The configuration file consists of a map where each key is the name of an environment and the
/// value is a `ConfigMap` which configures the core in that particular environment
pub type Config = BTreeMap<String, ConfigMap>;

/// Configuration map for a specific environment
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct ConfigMap {
    /// List of networks that are available in this environment
    pub networks: Vec<NetworkConfig>,
}

/// Network configuration
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct NetworkConfig {
    /// The URL of the server to connect to (e.g. `ircs://irc.freenode.net/` where the `ircs`
    /// scheme means that the server is using SSL and when no port is given, it defaults to `6667`)
    pub url: url::Url,
    /// The nickname to use on this network
    nickname: String,
    /// The username to use. If not set, this will default to the nickname
    username: Option<String>,
    /// The `real name` to use. If not set, this will default to the username
    realname: Option<String>,
    /// The password to send if the server is password-protected
    password: Option<String>,
    /// List of channels to join once connection has been established
    channels: Option<Vec<String>>,
}
