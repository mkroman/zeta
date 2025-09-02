use std::{collections::HashMap, fmt::Display, sync::Arc};

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::Message;
use reqwest::redirect::Policy;
use serde::de::DeserializeOwned;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, warn};

use crate::{Error, consts};

/// The name of a plugin.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Name(&'static str);
/// The author of a plugin.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Author(&'static str);
/// The version of a plugin.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Version(&'static str);

pub mod calculator;
pub mod choices;
pub mod dig;
pub mod geoip;
pub mod google_search;
pub mod health;
pub mod youtube;
pub mod messages;
pub mod actor_example;

/// A unique identifier for plugin actors
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ActorId(String);

impl ActorId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ActorId {
    fn from(id: &str) -> Self {
        Self(id.to_string())
    }
}

impl From<String> for ActorId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

/// A message that can be sent between plugin actors
pub trait PluginMessage: Send + Sync + std::fmt::Debug {
    /// The type name of this message for routing and debugging
    fn message_type(&self) -> &'static str;
    
    /// Clone this message for broadcasting
    fn clone_message(&self) -> Box<dyn PluginMessage>;
    
    /// Get access to the underlying type for downcasting
    fn as_any(&self) -> &dyn std::any::Any;
    
    /// Serialize the message for transmission (optional, for persistence/logging)
    fn serialize(&self) -> Result<Vec<u8>, Error> {
        Err(Error::ConfigurationError("Serialization not implemented".to_string()))
    }
}

/// Response from processing a plugin message
#[derive(Debug)]
pub enum MessageResponse {
    /// Message was handled successfully
    Handled,
    /// Message was handled with a response
    Reply(Box<dyn PluginMessage>),
    /// Message was not handled by this actor
    NotHandled,
    /// Error occurred while handling the message
    Error(Error),
}

/// Envelope containing a message and routing information
#[derive(Debug)]
pub struct MessageEnvelope {
    pub from: ActorId,
    pub to: ActorId,
    pub message: Box<dyn PluginMessage>,
    pub correlation_id: Option<String>,
}

/// Actor-based plugin trait extending NewPlugin with message handling
#[async_trait]
pub trait PluginActor: NewPlugin {
    /// Handle a message sent from another plugin actor
    async fn handle_actor_message(&self, envelope: MessageEnvelope) -> MessageResponse {
        warn!(
            plugin = Self::NAME,
            message_type = envelope.message.message_type(),
            "Unhandled actor message"
        );
        MessageResponse::NotHandled
    }
    
    /// Subscribe to message types this actor wants to receive
    fn message_subscriptions(&self) -> Vec<&'static str> {
        vec![]
    }
}

/// Message bus for inter-plugin communication
#[derive(Clone, Default)]
pub struct PluginBus {
    /// Map of actor IDs to their message channels
    actors: Arc<RwLock<HashMap<ActorId, mpsc::UnboundedSender<MessageEnvelope>>>>,
    /// Subscription map: message_type -> list of actor IDs
    subscriptions: Arc<RwLock<HashMap<String, Vec<ActorId>>>>,
}

impl PluginBus {
    pub fn new() -> Self {
        Self {
            actors: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Register an actor with the bus
    pub async fn register_actor(
        &self,
        actor_id: ActorId,
        sender: mpsc::UnboundedSender<MessageEnvelope>,
        subscriptions: Vec<&'static str>,
    ) {
        // Register the actor's message channel
        {
            let mut actors = self.actors.write().await;
            actors.insert(actor_id.clone(), sender);
        }
        
        // Register subscriptions
        {
            let mut subs = self.subscriptions.write().await;
            for message_type in subscriptions {
                subs.entry(message_type.to_string())
                    .or_insert_with(Vec::new)
                    .push(actor_id.clone());
            }
        }
        
        debug!(actor_id = %actor_id.as_str(), "Actor registered with message bus");
    }
    
    /// Send a message to a specific actor
    pub async fn send_to(
        &self,
        from: ActorId,
        to: ActorId,
        message: Box<dyn PluginMessage>,
    ) -> Result<(), Error> {
        let envelope = MessageEnvelope {
            from,
            to: to.clone(),
            message,
            correlation_id: None,
        };
        
        let actors = self.actors.read().await;
        if let Some(sender) = actors.get(&to) {
            sender.send(envelope).map_err(|_| {
                Error::ConfigurationError(format!("Failed to send message to actor: {}", to.as_str()))
            })?;
            Ok(())
        } else {
            Err(Error::ConfigurationError(format!("Actor not found: {}", to.as_str())))
        }
    }
    
    /// Broadcast a message to all subscribers of a message type
    pub async fn broadcast(
        &self,
        from: ActorId,
        message: Box<dyn PluginMessage>,
    ) -> Result<(), Error> {
        let message_type = message.message_type().to_string();
        let subscriptions = self.subscriptions.read().await;
        
        if let Some(subscribers) = subscriptions.get(&message_type) {
            let actors = self.actors.read().await;
            let mut sent_count = 0;
            
            for actor_id in subscribers {
                if let Some(sender) = actors.get(actor_id) {
                    let envelope = MessageEnvelope {
                        from: from.clone(),
                        to: actor_id.clone(),
                        message: message.clone_message(),
                        correlation_id: None,
                    };
                    
                    if let Err(_) = sender.send(envelope) {
                        warn!(
                            actor_id = %actor_id.as_str(),
                            "Failed to send broadcast message to actor"
                        );
                    } else {
                        sent_count += 1;
                    }
                }
            }
            
            debug!(
                message_type = message_type,
                subscribers = sent_count,
                "Broadcast message sent"
            );
        }
        
        Ok(())
    }
    
    /// Remove an actor from the bus
    pub async fn unregister_actor(&self, actor_id: &ActorId) {
        // Remove from actors map
        {
            let mut actors = self.actors.write().await;
            actors.remove(actor_id);
        }
        
        // Remove from subscriptions
        {
            let mut subs = self.subscriptions.write().await;
            for (_, subscribers) in subs.iter_mut() {
                subscribers.retain(|id| id != actor_id);
            }
        }
        
        debug!(actor_id = %actor_id.as_str(), "Actor unregistered from message bus");
    }
}

#[async_trait]
pub trait NewPlugin: Send + Sync {
    /// The name of the plugin.
    const NAME: &'static str;
    /// The author of the plugin.
    const AUTHOR: Author;
    /// The version of the plugin.
    const VERSION: Version;

    type Err: std::error::Error;
    type Config: DeserializeOwned;

    /// The constructor for a new plugin.
    fn with_config(config: &Self::Config) -> Self
    where
        Self: Sized;

    async fn handle_message(&self, _message: &Message, _client: &Client) -> Result<(), Error> {
        Ok(())
    }
}

/// A trait for plugins that can be used as trait objects
#[async_trait]
pub trait DynPlugin: Send + Sync {
    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), Error>;
    
    /// Handle actor messages (if plugin supports actor model)
    async fn handle_actor_message(&self, envelope: MessageEnvelope) -> MessageResponse {
        MessageResponse::NotHandled
    }
    
    /// Get message subscriptions for this plugin
    fn message_subscriptions(&self) -> Vec<&'static str> {
        vec![]
    }
    
    /// Get the actor ID for this plugin
    fn actor_id(&self) -> ActorId;
}

// Implement DynPlugin for all NewPlugin types
#[async_trait]
impl<T: NewPlugin> DynPlugin for T {
    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), Error> {
        NewPlugin::handle_message(self, message, client).await
    }
    
    async fn handle_actor_message(&self, _envelope: MessageEnvelope) -> MessageResponse {
        MessageResponse::NotHandled
    }
    
    fn message_subscriptions(&self) -> Vec<&'static str> {
        vec![]
    }
    
    fn actor_id(&self) -> ActorId {
        ActorId::new(T::NAME)
    }
}


type Plugins = Vec<Box<dyn DynPlugin>>;

/// A trait for creating plugins dynamically from configuration
pub trait PluginFactory: Send + Sync {
    fn name(&self) -> &'static str;
    fn create(&self, config: &figment::value::Value) -> Result<Box<dyn DynPlugin>, Error>;
}

/// A factory for creating plugins of a specific type
pub struct TypedPluginFactory<P: NewPlugin + 'static> {
    _phantom: std::marker::PhantomData<P>,
}

impl<P: NewPlugin + 'static> TypedPluginFactory<P> {
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<P: NewPlugin + 'static> PluginFactory for TypedPluginFactory<P> {
    fn name(&self) -> &'static str {
        P::NAME
    }

    fn create(&self, config_value: &figment::value::Value) -> Result<Box<dyn DynPlugin>, Error> {
        let config: P::Config = config_value.deserialize().map_err(|e| {
            Error::ConfigurationError(format!(
                "Failed to deserialize config for {}: {}",
                P::NAME,
                e
            ))
        })?;

        let plugin = P::with_config(&config);
        Ok(Box::new(plugin))
    }
}

#[derive(Default)]
pub struct Registry {
    pub plugins: Plugins,
    factories: HashMap<String, Box<dyn PluginFactory>>,
    pub bus: PluginBus,
    /// Task handles for plugin actor message loops
    actor_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl Registry {
    /// Constructs and returns a new, empty plugin registry.
    pub fn new() -> Registry {
        let mut registry = Registry {
            plugins: vec![],
            factories: HashMap::new(),
            bus: PluginBus::new(),
            actor_handles: vec![],
        };

        // Register all available plugin factories
        registry.register_factory(TypedPluginFactory::<dig::Dig>::new());
        registry.register_factory(TypedPluginFactory::<calculator::Calculator>::new());
        registry.register_factory(TypedPluginFactory::<choices::Choices>::new());
        registry.register_factory(TypedPluginFactory::<geoip::GeoIp>::new());
        registry.register_factory(TypedPluginFactory::<google_search::GoogleSearch>::new());
        registry.register_factory(TypedPluginFactory::<health::Health>::new());
        registry.register_factory(TypedPluginFactory::<youtube::YouTube>::new());

        registry
    }

    /// Registers a plugin factory
    pub fn register_factory<F: PluginFactory + 'static>(&mut self, factory: F) {
        let name = factory.name().to_string();
        self.factories.insert(name, Box::new(factory));
    }

    pub async fn load_plugins(
        &mut self,
        configs: &HashMap<String, figment::value::Value>,
    ) -> Result<(), Error> {
        // Clean up existing actors
        self.shutdown_actors().await;
        self.plugins.clear();

        debug!("registering plugins");

        // Load each plugin based on its configuration
        for (plugin_name, config_value) in configs {
            if let Some(factory) = self.factories.get(plugin_name) {
                match factory.create(config_value) {
                    Ok(plugin) => {
                        debug!(name = plugin_name, "successfully registered plugin");
                        
                        // Set up actor message handling if plugin supports it
                        let subscriptions = plugin.message_subscriptions();
                        if !subscriptions.is_empty() {
                            self.setup_plugin_actor(plugin.as_ref()).await?;
                        }
                        
                        self.plugins.push(plugin);
                    }
                    Err(err) => {
                        debug!(name = plugin_name, error = %err, "failed to register plugin");
                        return Err(err);
                    }
                }
            } else {
                debug!(name = plugin_name, "unknown plugin, skipping");
            }
        }

        Ok(())
    }
    
    /// Set up actor message handling for a plugin
    async fn setup_plugin_actor(&mut self, plugin: &dyn DynPlugin) -> Result<(), Error> {
        let actor_id = plugin.actor_id();
        let subscriptions = plugin.message_subscriptions();
        
        // Create message channel for this plugin
        let (sender, mut receiver) = mpsc::unbounded_channel::<MessageEnvelope>();
        
        // Register with bus
        self.bus.register_actor(actor_id.clone(), sender, subscriptions).await;
        
        // Spawn message handling task
        // Note: This is a simplified approach. In a real implementation, 
        // we'd need to handle plugin lifetimes more carefully
        let handle = tokio::spawn(async move {
            while let Some(envelope) = receiver.recv().await {
                debug!(
                    actor_id = %envelope.to.as_str(),
                    message_type = envelope.message.message_type(),
                    "Processing actor message"
                );
                
                // In a real implementation, we'd call plugin.handle_actor_message(envelope)
                // but we can't move the plugin into the async task easily
                // This would require a more sophisticated actor system architecture
            }
        });
        
        self.actor_handles.push(handle);
        Ok(())
    }
    
    /// Shutdown all actor tasks
    async fn shutdown_actors(&mut self) {
        for handle in self.actor_handles.drain(..) {
            handle.abort();
        }
    }
}

impl Display for Author {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "{}", self.0)
    }
}

impl Display for Version {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "{}", self.0)
    }
}

/// Returns a default HTTP client.
pub fn build_http_client() -> reqwest::Client {
    http_client_builder()
        .build()
        .expect("could not build http client")
}

/// Returns a default HTTP client builder.
pub fn http_client_builder() -> reqwest::ClientBuilder {
    reqwest::ClientBuilder::new()
        .user_agent(consts::HTTP_USER_AGENT)
        .redirect(Policy::none())
        .timeout(consts::HTTP_TIMEOUT)
}
