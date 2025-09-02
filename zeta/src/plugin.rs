use std::{collections::HashMap, fmt::Display, sync::Arc, time::Duration};

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::Message;
use reqwest::redirect::Policy;
use serde::de::DeserializeOwned;
use tokio::sync::{mpsc, RwLock, oneshot};
use tracing::{debug, warn, error};
use uuid::Uuid;
use once_cell::sync::Lazy;

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
pub mod choices_v2;
pub mod counter;
pub mod dig;
pub mod echo;
pub mod geoip;
pub mod google_search;
pub mod health;
pub mod youtube;
pub mod messages;
pub mod actor_example;
pub mod macros;
pub mod auto;
pub mod simple_echo;
pub mod weather;
pub mod minimal;
pub mod typed_messages;
pub mod typed;

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

impl Display for ActorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
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

// PluginActor trait removed - all functionality moved to Plugin trait

/// Context provided to plugins with access to the message bus and other services
#[derive(Clone)]
pub struct PluginContext {
    pub bus: PluginBus,
    pub typed_bus: typed::TypedMessageBus,
    pub actor_id: ActorId,
}

impl PluginContext {
    pub fn new(bus: PluginBus, actor_id: ActorId) -> Self {
        Self { 
            bus, 
            typed_bus: typed::TypedMessageBus::new(),
            actor_id 
        }
    }
    
    pub fn with_typed_bus(bus: PluginBus, typed_bus: typed::TypedMessageBus, actor_id: ActorId) -> Self {
        Self { bus, typed_bus, actor_id }
    }
    
    /// Send a message to a specific plugin actor
    pub async fn send_to<M: PluginMessage + 'static>(&self, target: impl Into<ActorId>, message: M) -> Result<(), Error> {
        self.bus.send_to(self.actor_id.clone(), target.into(), Box::new(message)).await
    }
    
    /// Broadcast a message to all subscribers
    pub async fn broadcast<M: PluginMessage + 'static>(&self, message: M) -> Result<(), Error> {
        self.bus.broadcast(self.actor_id.clone(), Box::new(message)).await
    }
    
    // ========== TYPED MESSAGING METHODS ==========
    
    /// Send a typed message and wait for response
    pub async fn send_typed<M: typed::TypedMessage>(&self, target: &str, message: M) -> Result<M::Response, Error> {
        self.typed_bus.send_message(self.actor_id.as_str(), target, message).await
    }
    
    /// Send a typed event (no response expected)
    pub async fn send_typed_event<M: typed::TypedMessage>(&self, target: &str, message: M) -> Result<(), Error> {
        self.typed_bus.send_event(self.actor_id.as_str(), target, message).await
    }
    
    /// Broadcast a typed message to all subscribers
    pub async fn broadcast_typed<M: typed::TypedMessage>(&self, message: M) -> Result<Vec<String>, Error> {
        self.typed_bus.broadcast(self.actor_id.as_str(), message).await
    }
    
    // ========== TYPED CONVENIENCE METHODS (REPLACE JSON SERIALIZATION) ==========
    
    /// Search Google with type safety (replaces JSON method)
    pub async fn google_search_typed(&self, query: &str, limit: Option<usize>) -> Result<typed::GoogleSearchResponse, Error> {
        let request = typed::GoogleSearchRequest {
            query: query.to_string(),
            limit,
        };
        self.send_typed("google_search", request).await
    }
    
    /// Look up GeoIP information with type safety
    pub async fn geoip_lookup_typed(&self, target: &str) -> Result<typed::GeoIpResponse, Error> {
        let request = typed::GeoIpRequest {
            target: target.to_string(),
        };
        self.send_typed("geoip", request).await
    }
    
    /// Evaluate a mathematical expression with type safety
    pub async fn calculate_typed(&self, expression: &str) -> Result<typed::CalculationResponse, Error> {
        let request = typed::CalculationRequest {
            expression: expression.to_string(),
        };
        self.send_typed("calculator", request).await
    }
    
    /// Perform DNS lookup with type safety
    pub async fn dns_lookup_typed(&self, domain: &str, record_type: Option<&str>) -> Result<typed::DnsLookupResponse, Error> {
        let request = typed::DnsLookupRequest {
            domain: domain.to_string(),
            record_type: record_type.map(|s| s.to_string()),
        };
        self.send_typed("dig", request).await
    }
    
    /// Get weather information with type safety
    pub async fn weather_lookup_typed(&self, location: &str) -> Result<typed::WeatherResponse, Error> {
        let request = typed::WeatherRequest {
            location: location.to_string(),
        };
        self.send_typed("weather", request).await
    }
    
    /// Send a health check request
    pub async fn health_check_typed(&self, plugin: &str) -> Result<typed::HealthCheckResponse, Error> {
        let request = typed::HealthCheckRequest {
            requester: self.actor_id.as_str().to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };
        self.send_typed(plugin, request).await
    }
    
    /// Send a typed event notification
    pub async fn notify_event_typed(&self, event_type: &str, data: serde_json::Value) -> Result<Vec<String>, Error> {
        let event = typed::EventNotification {
            event_type: event_type.to_string(),
            source: self.actor_id.as_str().to_string(),
            data,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };
        self.broadcast_typed(event).await
    }
    
    /// Call a function on another plugin and wait for the response
    pub async fn call_function(
        &self,
        plugin_name: &str,
        function_name: String,
        args: serde_json::Value,
        timeout: Option<Duration>,
    ) -> Result<serde_json::Value, Error> {
        let target_actor = ActorId::new(plugin_name.to_string());
        self.bus.call_function(
            self.actor_id.clone(),
            target_actor,
            function_name,
            args,
            timeout,
        ).await
    }
    
    // ========== TYPED CONVENIENCE METHODS ==========
    
    /// Search Google with type safety
    pub async fn google_search_typed(&self, query: &str, limit: Option<usize>) -> Result<typed::GoogleSearchResponse, Error> {
        let request = typed::GoogleSearchRequest {
            query: query.to_string(),
            limit,
        };
        self.send_typed("google_search", request).await
    }
    
    /// Legacy method - Search Google via the google_search plugin
    pub async fn google_search(&self, query: &str, limit: Option<usize>) -> Result<Vec<crate::plugin::messages::GoogleSearchResult>, Error> {
        use crate::plugin::messages::GoogleSearchArgs;
        
        let args = GoogleSearchArgs {
            query: query.to_string(),
            limit,
        };
        
        let result = self.call_function(
            "google_search",
            "search".to_string(),
            serde_json::to_value(args).unwrap(),
            None,
        ).await?;
        
        serde_json::from_value(result).map_err(|e| 
            Error::ConfigurationError(format!("Failed to deserialize Google search results: {}", e))
        )
    }
    
    /// Look up GeoIP information via the geoip plugin
    pub async fn geoip_lookup(&self, target: &str) -> Result<crate::plugin::messages::GeoIpResult, Error> {
        use crate::plugin::messages::GeoIpArgs;
        
        let args = GeoIpArgs {
            target: target.to_string(),
        };
        
        let result = self.call_function(
            "geoip",
            "lookup".to_string(),
            serde_json::to_value(args).unwrap(),
            None,
        ).await?;
        
        serde_json::from_value(result).map_err(|e| 
            Error::ConfigurationError(format!("Failed to deserialize GeoIP result: {}", e))
        )
    }
    
    /// Evaluate a mathematical expression via the calculator plugin
    pub async fn calculate(&self, expression: &str) -> Result<crate::plugin::messages::CalculatorResult, Error> {
        use crate::plugin::messages::CalculatorArgs;
        
        let args = CalculatorArgs {
            expression: expression.to_string(),
        };
        
        let result = self.call_function(
            "calculator",
            "evaluate".to_string(),
            serde_json::to_value(args).unwrap(),
            None,
        ).await?;
        
        serde_json::from_value(result).map_err(|e| 
            Error::ConfigurationError(format!("Failed to deserialize calculator result: {}", e))
        )
    }
    
    /// Perform DNS lookup via the dig plugin
    pub async fn dns_lookup(&self, domain: &str, record_type: Option<&str>) -> Result<crate::plugin::messages::DigResult, Error> {
        use crate::plugin::messages::DigArgs;
        
        let args = DigArgs {
            domain: domain.to_string(),
            record_type: record_type.map(|s| s.to_string()),
        };
        
        let result = self.call_function(
            "dig",
            "lookup".to_string(),
            serde_json::to_value(args).unwrap(),
            None,
        ).await?;
        
        serde_json::from_value(result).map_err(|e| 
            Error::ConfigurationError(format!("Failed to deserialize DNS lookup result: {}", e))
        )
    }
    
    /// Get YouTube video information via the youtube plugin
    pub async fn youtube_info(&self, video_id: &str) -> Result<crate::plugin::messages::YouTubeVideoResult, Error> {
        use crate::plugin::messages::YouTubeSearchArgs;
        
        let args = YouTubeSearchArgs {
            query: video_id.to_string(), // For getting info by video ID
            limit: Some(1),
        };
        
        let result = self.call_function(
            "youtube",
            "get_video_info".to_string(),
            serde_json::to_value(args).unwrap(),
            None,
        ).await?;
        
        serde_json::from_value(result).map_err(|e| 
            Error::ConfigurationError(format!("Failed to deserialize YouTube video info: {}", e))
        )
    }
}

/// Message bus for inter-plugin communication
#[derive(Clone, Default)]
pub struct PluginBus {
    /// Map of actor IDs to their message channels
    actors: Arc<RwLock<HashMap<ActorId, mpsc::UnboundedSender<MessageEnvelope>>>>,
    /// Subscription map: message_type -> list of actor IDs
    subscriptions: Arc<RwLock<HashMap<String, Vec<ActorId>>>>,
    /// Pending function call responses: request_id -> response sender
    pending_calls: Arc<RwLock<HashMap<String, oneshot::Sender<Result<serde_json::Value, String>>>>>,
}

impl PluginBus {
    pub fn new() -> Self {
        Self {
            actors: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            pending_calls: Arc::new(RwLock::new(HashMap::new())),
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
    
    /// Call a function on another plugin and wait for the response
    pub async fn call_function(
        &self,
        from: ActorId,
        to: ActorId,
        function_name: String,
        args: serde_json::Value,
        timeout: Option<Duration>,
    ) -> Result<serde_json::Value, Error> {
        use crate::plugin::messages::FunctionCallRequest;
        
        let request_id = Uuid::new_v4().to_string();
        let timeout_duration = timeout.unwrap_or(Duration::from_secs(30));
        
        // Create a oneshot channel for the response
        let (response_tx, response_rx) = oneshot::channel();
        
        // Register the pending call
        {
            let mut pending = self.pending_calls.write().await;
            pending.insert(request_id.clone(), response_tx);
        }
        
        // Create the function call request
        let request = FunctionCallRequest {
            function_name,
            args,
            timeout_ms: Some(timeout_duration.as_millis() as u64),
            request_id: request_id.clone(),
        };
        
        // Send the request
        self.send_to(from, to, Box::new(request)).await?;
        
        // Wait for the response with timeout
        match tokio::time::timeout(timeout_duration, response_rx).await {
            Ok(Ok(result)) => result.map_err(|e| Error::ConfigurationError(e)),
            Ok(Err(_)) => Err(Error::ConfigurationError("Response channel closed".to_string())),
            Err(_) => {
                // Clean up the pending call on timeout
                let mut pending = self.pending_calls.write().await;
                pending.remove(&request_id);
                Err(Error::ConfigurationError("Function call timeout".to_string()))
            }
        }
    }
    
    /// Handle a function call response (internal method called by plugins)
    pub async fn handle_function_response(&self, response: crate::plugin::messages::FunctionCallResponse) {
        let mut pending = self.pending_calls.write().await;
        if let Some(response_tx) = pending.remove(&response.request_id) {
            let _ = response_tx.send(response.result);
        }
    }
}

/// The new simplified Plugin trait - plugins are actors that run as tasks
#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    /// The name of the plugin
    const NAME: &'static str;
    /// The author of the plugin
    const AUTHOR: &'static str;
    /// The version of the plugin
    const VERSION: &'static str;
    
    /// Configuration type for this plugin
    type Config: DeserializeOwned + Send + Sync;
    
    /// Create a new plugin instance
    async fn new(config: Self::Config, context: PluginContext) -> Result<Self, Error>
    where
        Self: Sized;
    
    /// Main plugin task loop - plugins run as independent actors
    async fn run(&mut self) -> Result<(), Error>;
    
    /// Handle IRC messages (optional)
    async fn handle_irc_message(&mut self, _message: &Message, _client: &Client) -> Result<(), Error> {
        Ok(())
    }
    
    /// Handle plugin messages (optional) - return true if handled
    async fn handle_plugin_message(&mut self, _envelope: MessageEnvelope) -> Result<bool, Error> {
        Ok(false)
    }
    
    /// Get message subscriptions for this plugin (optional)
    fn subscriptions() -> Vec<&'static str> where Self: Sized {
        vec![]
    }
    
    /// Called when plugin should shut down
    async fn shutdown(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

/// Messages that can be sent to plugin tasks
pub enum PluginTaskMessage {
    /// IRC message to handle
    IrcMessage { message: Message, client_ref: *const Client },
    /// Inter-plugin message
    PluginMessage(MessageEnvelope),
    /// Shutdown signal
    Shutdown,
}

// Manual Debug impl since raw pointers don't derive Debug
impl std::fmt::Debug for PluginTaskMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginTaskMessage::IrcMessage { message, .. } => {
                f.debug_struct("IrcMessage").field("message", message).finish()
            }
            PluginTaskMessage::PluginMessage(envelope) => {
                f.debug_tuple("PluginMessage").field(envelope).finish()
            }
            PluginTaskMessage::Shutdown => f.debug_tuple("Shutdown").finish(),
        }
    }
}

/// A plugin task wrapper that manages the plugin lifecycle
pub struct PluginTask<P: Plugin> {
    plugin: P,
    receiver: mpsc::UnboundedReceiver<PluginTaskMessage>,
    context: PluginContext,
}

impl<P: Plugin> PluginTask<P> {
    pub fn new(
        plugin: P,
        receiver: mpsc::UnboundedReceiver<PluginTaskMessage>,
        context: PluginContext,
    ) -> Self {
        Self {
            plugin,
            receiver,
            context,
        }
    }
    
    /// Run the plugin task loop
    pub async fn run(mut self) -> Result<(), Error> {
        debug!(plugin = P::NAME, "Starting plugin task");
        
        // For simple plugins that only handle messages, we don't need to run the main loop
        // We just handle incoming messages
        while let Some(msg) = self.receiver.recv().await {
            match msg {
                PluginTaskMessage::IrcMessage { message, client } => {
                    if let Err(e) = self.plugin.handle_irc_message(&message, &client).await {
                        warn!(plugin = P::NAME, error = %e, "IRC message handling failed");
                    }
                }
                PluginTaskMessage::PluginMessage(envelope) => {
                    match self.plugin.handle_plugin_message(envelope).await {
                        Ok(handled) => {
                            if !handled {
                                debug!(plugin = P::NAME, "Message not handled");
                            }
                        }
                        Err(e) => {
                            warn!(plugin = P::NAME, error = %e, "Plugin message handling failed");
                        }
                    }
                }
                PluginTaskMessage::Shutdown => {
                    debug!(plugin = P::NAME, "Shutdown requested");
                    if let Err(e) = self.plugin.shutdown().await {
                        warn!(plugin = P::NAME, error = %e, "Plugin shutdown failed");
                    }
                    break;
                }
            }
        }
        
        debug!(plugin = P::NAME, "Plugin task stopped");
        Ok(())
    }
}

/// Handle to a running plugin task
pub struct PluginHandle {
    pub actor_id: ActorId,
    pub sender: mpsc::UnboundedSender<PluginTaskMessage>,
    pub task_handle: tokio::task::JoinHandle<Result<(), Error>>,
    pub subscriptions: Vec<&'static str>,
}

impl PluginHandle {
    pub async fn send_irc_message(&self, message: Message, client: Arc<Client>) -> Result<(), Error> {
        self.sender
            .send(PluginTaskMessage::IrcMessage { message, client })
            .map_err(|_| Error::ConfigurationError(format!("Failed to send IRC message to plugin {}", self.actor_id)))
    }
    
    pub async fn send_plugin_message(&self, envelope: MessageEnvelope) -> Result<(), Error> {
        self.sender
            .send(PluginTaskMessage::PluginMessage(envelope))
            .map_err(|_| Error::ConfigurationError(format!("Failed to send plugin message to {}", self.actor_id)))
    }
    
    pub async fn shutdown(&self) -> Result<(), Error> {
        self.sender
            .send(PluginTaskMessage::Shutdown)
            .map_err(|_| Error::ConfigurationError(format!("Failed to send shutdown to plugin {}", self.actor_id)))
    }
}

/// Legacy trait for backward compatibility
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

    /// Handle IRC messages. Context provides access to the message bus.
    async fn handle_message(&self, _message: &Message, _client: &Client, _ctx: &PluginContext) -> Result<(), Error> {
        Ok(())
    }
}

/// A trait for plugins that can be used as trait objects
#[async_trait]
pub trait DynPlugin: Send + Sync {
    /// Handle IRC messages with context
    async fn handle_message(&self, message: &Message, client: &Client, ctx: &PluginContext) -> Result<(), Error>;
    
    /// Handle actor messages (if plugin supports actor model)  
    async fn handle_actor_message(&self, envelope: MessageEnvelope, ctx: &PluginContext) -> MessageResponse {
        MessageResponse::NotHandled
    }
    
    /// Get message subscriptions for this plugin
    fn subscriptions(&self) -> Vec<&'static str> {
        vec![]
    }
    
    /// Get the actor ID for this plugin
    fn actor_id(&self) -> ActorId;
}

// Implement DynPlugin for all NewPlugin types
#[async_trait]
impl<T: NewPlugin> DynPlugin for T {
    async fn handle_message(&self, message: &Message, client: &Client, ctx: &PluginContext) -> Result<(), Error> {
        NewPlugin::handle_message(self, message, client, ctx).await
    }
    
    fn actor_id(&self) -> ActorId {
        ActorId::new(T::NAME)
    }
}


/// Wrapper that holds a plugin and its actor capabilities
pub struct PluginWrapper {
    plugin: Box<dyn DynPlugin>,
    subscriptions: Vec<&'static str>,
    context: PluginContext,
}

impl PluginWrapper {
    pub fn new(plugin: Box<dyn DynPlugin>, context: PluginContext, subscriptions: Vec<&'static str>) -> Self {
        Self {
            plugin,
            subscriptions,
            context,
        }
    }
    
    pub fn actor_id(&self) -> &ActorId {
        &self.context.actor_id
    }
    
    pub fn subscriptions(&self) -> &[&'static str] {
        &self.subscriptions
    }
    
    pub async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), Error> {
        self.plugin.handle_message(message, client, &self.context).await
    }
    
    pub async fn handle_actor_message(&self, envelope: MessageEnvelope) -> MessageResponse {
        self.plugin.handle_actor_message(envelope, &self.context).await
    }
}

type Plugins = Vec<PluginWrapper>;
type PluginHandles = Vec<PluginHandle>;

/// Factory trait for creating new-style plugin tasks
#[async_trait]
pub trait PluginTaskFactory: Send + Sync {
    fn name(&self) -> &'static str;
    fn subscriptions(&self) -> Vec<&'static str>;
    async fn create_task(
        &self, 
        config: &figment::value::Value, 
        bus: PluginBus
    ) -> Result<PluginHandle, Error>;
}

/// Factory for new-style plugin tasks
pub struct TaskPluginFactory<P: Plugin> {
    _phantom: std::marker::PhantomData<P>,
}

impl<P: Plugin> TaskPluginFactory<P> {
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<P: Plugin> PluginTaskFactory for TaskPluginFactory<P> {
    fn name(&self) -> &'static str {
        P::NAME
    }
    
    fn subscriptions(&self) -> Vec<&'static str> {
        P::subscriptions()
    }
    
    async fn create_task(
        &self,
        config_value: &figment::value::Value,
        bus: PluginBus,
    ) -> Result<PluginHandle, Error> {
        let config: P::Config = config_value.deserialize().map_err(|e| {
            Error::ConfigurationError(format!(
                "Failed to deserialize config for {}: {}",
                P::NAME,
                e
            ))
        })?;
        
        let actor_id = ActorId::new(P::NAME);
        let context = PluginContext::new(bus, actor_id.clone());
        let subscriptions = P::subscriptions();
        
        // Create the plugin instance
        let plugin = P::new(config, context.clone()).await?;
        
        // Create communication channel
        let (sender, receiver) = mpsc::unbounded_channel();
        
        // Create and spawn the plugin task
        let task = PluginTask::new(plugin, receiver, context);
        let task_handle = tokio::spawn(task.run());
        
        Ok(PluginHandle {
            actor_id,
            sender,
            task_handle,
            subscriptions,
        })
    }
}

/// Legacy trait for creating plugins dynamically from configuration
pub trait PluginFactory: Send + Sync {
    fn name(&self) -> &'static str;
    fn create(&self, config: &figment::value::Value, bus: PluginBus) -> Result<PluginWrapper, Error>;
    fn subscriptions(&self) -> Vec<&'static str>;
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

    fn create(&self, config_value: &figment::value::Value, bus: PluginBus) -> Result<PluginWrapper, Error> {
        let config: P::Config = config_value.deserialize().map_err(|e| {
            Error::ConfigurationError(format!(
                "Failed to deserialize config for {}: {}",
                P::NAME,
                e
            ))
        })?;

        let plugin = P::with_config(&config);
        let actor_id = ActorId::new(P::NAME);
        let context = PluginContext::new(bus, actor_id);
        let subscriptions = self.subscriptions();
        
        Ok(PluginWrapper::new(Box::new(plugin), context, subscriptions))
    }
    
    fn subscriptions(&self) -> Vec<&'static str> {
        // Default implementation - no subscriptions
        vec![]
    }
}

// TypedActorPluginFactory removed - consolidated into TaskPluginFactory

#[derive(Default)]
pub struct Registry {
    pub plugins: Plugins,
    pub plugin_handles: PluginHandles,
    factories: HashMap<String, Box<dyn PluginFactory>>,
    task_factories: HashMap<String, Box<dyn PluginTaskFactory>>,
    pub bus: PluginBus,
    /// Task handles for plugin actor message loops
    actor_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl Registry {
    /// Constructs and returns a new, empty plugin registry.
    pub fn new() -> Registry {
        let mut registry = Registry {
            plugins: vec![],
            plugin_handles: vec![],
            factories: HashMap::new(),
            task_factories: HashMap::new(),
            bus: PluginBus::new(),
            actor_handles: vec![],
        };

        // Register all available plugin factories
        registry.register_factory(TypedPluginFactory::<dig::Dig>::new());
        registry.register_factory(TypedPluginFactory::<calculator::Calculator>::new());
        registry.register_factory(TypedPluginFactory::<choices::Choices>::new());
        registry.register_factory(TypedPluginFactory::<geoip::GeoIp>::new());
        registry.register_factory(TypedPluginFactory::<google_search::GoogleSearch>::new());
        registry.register_factory(TypedPluginFactory::<youtube::YouTube>::new());
        
        // Register legacy plugins
        registry.register_factory(TypedPluginFactory::<health::Health>::new());

        // Register new-style plugin tasks (these take priority over legacy plugins)
        registry.register_task_factory(TaskPluginFactory::<choices_v2::ChoicesV2>::new());
        registry.register_task_factory(TaskPluginFactory::<counter::Counter>::new());
        registry.register_task_factory(TaskPluginFactory::<echo::Echo>::new());
        registry.register_task_factory(TaskPluginFactory::<weather::Weather>::new());
        registry.register_task_factory(TaskPluginFactory::<minimal::Minimal>::new());
        registry.register_task_factory(TaskPluginFactory::<simple_echo::SimpleEcho>::new());
        
        // Auto-discover plugins using linkme (future enhancement)
        // registry.auto_discover_plugins();

        registry
    }

    /// Registers a plugin task factory
    pub fn register_task_factory<F: PluginTaskFactory + 'static>(&mut self, factory: F) {
        let name = factory.name().to_string();
        self.task_factories.insert(name, Box::new(factory));
    }

    /// Registers a legacy plugin factory
    pub fn register_factory<F: PluginFactory + 'static>(&mut self, factory: F) {
        let name = factory.name().to_string();
        self.factories.insert(name, Box::new(factory));
    }

    pub async fn load_plugins(
        &mut self,
        configs: &HashMap<String, figment::value::Value>,
    ) -> Result<(), Error> {
        // Clean up existing actors and tasks
        self.shutdown_actors().await;
        self.shutdown_plugin_tasks().await;
        self.plugins.clear();
        self.plugin_handles.clear();

        debug!("registering plugins");

        // Load each plugin based on its configuration
        for (plugin_name, config_value) in configs {
            // Try new-style task factory first
            if let Some(task_factory) = self.task_factories.get(plugin_name) {
                match task_factory.create_task(config_value, self.bus.clone()).await {
                    Ok(plugin_handle) => {
                        debug!(name = plugin_name, "successfully registered plugin task");
                        
                        // Register with message bus if plugin has subscriptions
                        if !plugin_handle.subscriptions.is_empty() {
                            // Create a bridge between MessageEnvelope and PluginTaskMessage
                            let (envelope_sender, mut envelope_receiver) = mpsc::unbounded_channel::<MessageEnvelope>();
                            let task_sender = plugin_handle.sender.clone();
                            
                            // Spawn task to convert MessageEnvelope to PluginTaskMessage
                            let converter_task = tokio::spawn(async move {
                                while let Some(envelope) = envelope_receiver.recv().await {
                                    let _ = task_sender.send(PluginTaskMessage::PluginMessage(envelope));
                                }
                            });
                            
                            self.actor_handles.push(converter_task);
                            
                            self.bus.register_actor(
                                plugin_handle.actor_id.clone(),
                                envelope_sender,
                                plugin_handle.subscriptions.clone(),
                            ).await;
                        }
                        
                        self.plugin_handles.push(plugin_handle);
                    }
                    Err(err) => {
                        debug!(name = plugin_name, error = %err, "failed to register plugin task");
                        return Err(err);
                    }
                }
            }
            // Fall back to legacy plugin factory
            else if let Some(factory) = self.factories.get(plugin_name) {
                match factory.create(config_value, self.bus.clone()) {
                    Ok(plugin_wrapper) => {
                        debug!(name = plugin_name, "successfully registered legacy plugin");
                        
                        // Set up actor message handling if plugin has subscriptions
                        let subscriptions = plugin_wrapper.subscriptions();
                        if !subscriptions.is_empty() {
                            self.setup_plugin_actor(&plugin_wrapper).await?;
                        }
                        
                        self.plugins.push(plugin_wrapper);
                    }
                    Err(err) => {
                        debug!(name = plugin_name, error = %err, "failed to register legacy plugin");
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
    async fn setup_plugin_actor(&mut self, plugin_wrapper: &PluginWrapper) -> Result<(), Error> {
        let actor_id = plugin_wrapper.actor_id().clone();
        let subscriptions = plugin_wrapper.subscriptions().iter().map(|&s| s).collect();
        
        // Create message channel for this plugin
        let (sender, mut receiver) = mpsc::unbounded_channel::<MessageEnvelope>();
        
        // Register with bus
        self.bus.register_actor(actor_id.clone(), sender, subscriptions).await;
        
        // Note: For a complete implementation, we'd need to spawn actor message handling tasks
        // This is simplified for now as we'd need to handle plugin lifetimes more carefully
        debug!(actor_id = %actor_id.as_str(), "Actor registered for message handling");
        
        Ok(())
    }
    
    /// Shutdown all plugin tasks
    async fn shutdown_plugin_tasks(&mut self) {
        for plugin_handle in self.plugin_handles.drain(..) {
            let _ = plugin_handle.shutdown().await;
            plugin_handle.task_handle.abort();
            let _ = plugin_handle.task_handle.await;
        }
    }
    
    /// Shutdown all actor tasks
    async fn shutdown_actors(&mut self) {
        for handle in self.actor_handles.drain(..) {
            handle.abort();
        }
    }
    
    /// Send IRC message to all plugins
    pub async fn handle_irc_message(&self, message: &Message, client: &Client) -> Result<(), Error> {
        // Send to new-style plugin tasks
        let client_arc = Arc::new((*client).clone());
        for plugin_handle in &self.plugin_handles {
            let _ = plugin_handle.send_irc_message(message.clone(), client_arc.clone()).await;
        }
        
        // Send to legacy plugins
        for plugin_wrapper in &self.plugins {
            if let Err(e) = plugin_wrapper.handle_message(message, client).await {
                warn!(plugin = ?plugin_wrapper.actor_id(), error = %e, "Plugin IRC message handling failed");
            }
        }
        
        Ok(())
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
