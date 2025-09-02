//! Typed message system for type-safe inter-plugin communication

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;
use crate::Error;

/// Trait for messages that can be sent between plugins with full type safety
pub trait TypedMessage: Send + Sync + Debug + 'static {
    /// The response type for this message (use () for no response)
    type Response: Send + Sync + Debug + 'static;
    
    /// Unique message type identifier
    fn message_type_id() -> TypeId where Self: Sized {
        TypeId::of::<Self>()
    }
    
    /// Human-readable message type name for debugging
    fn message_type_name() -> &'static str where Self: Sized;
}

/// Wrapper for typed messages that can be sent over channels
#[derive(Debug)]
pub struct MessageEnvelope<M: TypedMessage> {
    pub from: String,
    pub to: String,
    pub message: M,
    pub correlation_id: Option<String>,
    pub response_channel: Option<oneshot::Sender<M::Response>>,
}

/// Response wrapper for typed message responses
#[derive(Debug)]
pub struct MessageResponse<R> {
    pub correlation_id: String,
    pub response: Result<R, String>,
}

/// Type-safe message handler trait
#[async_trait]
pub trait TypedMessageHandler<M: TypedMessage>: Send + Sync {
    async fn handle_message(&self, message: M) -> Result<M::Response, Error>;
}

/// Registry for typed message handlers
pub struct TypedMessageRegistry {
    /// Map of TypeId to handler functions
    handlers: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// Map of plugin names to their message senders
    senders: HashMap<String, TypedMessageSender>,
}

impl TypedMessageRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            senders: HashMap::new(),
        }
    }
    
    /// Register a typed message handler
    pub fn register_handler<M: TypedMessage, H: TypedMessageHandler<M> + 'static>(
        &mut self,
        plugin_name: String,
        handler: H,
    ) {
        let handler_box = Box::new(handler);
        self.handlers.insert(TypeId::of::<M>(), handler_box);
    }
    
    /// Register a plugin's message sender
    pub fn register_sender(&mut self, plugin_name: String, sender: TypedMessageSender) {
        self.senders.insert(plugin_name, sender);
    }
    
    /// Send a typed message to a plugin
    pub async fn send_message<M: TypedMessage>(
        &self,
        from: &str,
        to: &str,
        message: M,
    ) -> Result<M::Response, Error> {
        if let Some(sender) = self.senders.get(to) {
            sender.send_typed_message(from, message).await
        } else {
            Err(Error::ConfigurationError(format!("Plugin not found: {}", to)))
        }
    }
    
    /// Broadcast a typed message to all plugins that handle this message type
    pub async fn broadcast_message<M: TypedMessage>(
        &self,
        from: &str,
        message: M,
    ) -> Vec<Result<M::Response, Error>> {
        let mut results = Vec::new();
        
        for (plugin_name, sender) in &self.senders {
            if plugin_name != from && sender.can_handle::<M>() {
                let result = sender.send_typed_message(from, message.clone()).await;
                results.push(result);
            }
        }
        
        results
    }
}

/// Type-safe message sender for a specific plugin
#[derive(Clone)]
pub struct TypedMessageSender {
    /// Channel for sending any typed message
    sender: mpsc::UnboundedSender<Box<dyn Any + Send>>,
    /// Set of message types this plugin can handle
    supported_types: Arc<Vec<TypeId>>,
}

impl TypedMessageSender {
    pub fn new<T: 'static>(
        sender: mpsc::UnboundedSender<T>,
        supported_types: Vec<TypeId>,
    ) -> Self {
        // Wrap the typed sender in a type-erased sender
        let (any_sender, mut any_receiver) = mpsc::unbounded_channel::<Box<dyn Any + Send>>();
        
        // Spawn a task to convert Any messages back to typed messages
        tokio::spawn(async move {
            while let Some(any_msg) = any_receiver.recv().await {
                if let Ok(typed_msg) = any_msg.downcast::<T>() {
                    let _ = sender.send(*typed_msg);
                }
            }
        });
        
        Self {
            sender: any_sender,
            supported_types: Arc::new(supported_types),
        }
    }
    
    pub fn can_handle<M: TypedMessage>(&self) -> bool {
        self.supported_types.contains(&TypeId::of::<M>())
    }
    
    pub async fn send_typed_message<M: TypedMessage + Clone>(
        &self,
        from: &str,
        message: M,
    ) -> Result<M::Response, Error> {
        if !self.can_handle::<M>() {
            return Err(Error::ConfigurationError(
                "Plugin cannot handle this message type".to_string()
            ));
        }
        
        // For messages that expect a response, we need a different approach
        // This is a simplified version - in practice, you'd use response channels
        self.sender
            .send(Box::new(message))
            .map_err(|_| Error::ConfigurationError("Failed to send message".to_string()))?;
        
        // For now, return a default response - this would be replaced with actual response handling
        todo!("Implement proper response handling")
    }
}

// Built-in typed messages

/// Request for plugin health information
#[derive(Debug, Clone)]
pub struct HealthRequest {
    pub requester: String,
}

impl TypedMessage for HealthRequest {
    type Response = HealthResponse;
    
    fn message_type_name() -> &'static str {
        "HealthRequest"
    }
}

/// Health information response
#[derive(Debug, Clone)]
pub struct HealthResponse {
    pub plugin_name: String,
    pub status: HealthStatus,
    pub memory_mb: f64,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Event notification with typed data
#[derive(Debug, Clone)]
pub struct EventNotification<T: Send + Sync + Debug + Clone + 'static> {
    pub event_type: String,
    pub source: String,
    pub data: T,
    pub timestamp: u64,
}

impl<T: Send + Sync + Debug + Clone + 'static> TypedMessage for EventNotification<T> {
    type Response = (); // Events don't need responses
    
    fn message_type_name() -> &'static str {
        "EventNotification"
    }
}

/// Function call request with typed parameters
#[derive(Debug, Clone)]
pub struct FunctionCall<Args: Send + Sync + Debug + Clone + 'static> {
    pub function_name: String,
    pub args: Args,
    pub timeout_ms: Option<u64>,
}

impl<Args: Send + Sync + Debug + Clone + 'static> TypedMessage for FunctionCall<Args> {
    type Response = serde_json::Value; // Functions can return any JSON value
    
    fn message_type_name() -> &'static str {
        "FunctionCall"
    }
}

/// Command message with typed arguments
#[derive(Debug, Clone)]
pub struct Command<Args: Send + Sync + Debug + Clone + 'static> {
    pub command: String,
    pub args: Args,
}

impl<Args: Send + Sync + Debug + Clone + 'static> TypedMessage for Command<Args> {
    type Response = CommandResult;
    
    fn message_type_name() -> &'static str {
        "Command"
    }
}

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub success: bool,
    pub output: String,
}

// Specific typed messages for common plugin interactions

/// Google search request
#[derive(Debug, Clone)]
pub struct GoogleSearchRequest {
    pub query: String,
    pub limit: Option<usize>,
}

impl TypedMessage for GoogleSearchRequest {
    type Response = GoogleSearchResponse;
    
    fn message_type_name() -> &'static str {
        "GoogleSearchRequest"
    }
}

#[derive(Debug, Clone)]
pub struct GoogleSearchResponse {
    pub results: Vec<GoogleSearchResult>,
}

#[derive(Debug, Clone)]
pub struct GoogleSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Calculator evaluation request
#[derive(Debug, Clone)]
pub struct CalculationRequest {
    pub expression: String,
}

impl TypedMessage for CalculationRequest {
    type Response = CalculationResponse;
    
    fn message_type_name() -> &'static str {
        "CalculationRequest"
    }
}

#[derive(Debug, Clone)]
pub struct CalculationResponse {
    pub result: f64,
    pub formatted: String,
}

/// DNS lookup request
#[derive(Debug, Clone)]
pub struct DnsLookupRequest {
    pub domain: String,
    pub record_type: Option<String>,
}

impl TypedMessage for DnsLookupRequest {
    type Response = DnsLookupResponse;
    
    fn message_type_name() -> &'static str {
        "DnsLookupRequest"
    }
}

#[derive(Debug, Clone)]
pub struct DnsLookupResponse {
    pub records: Vec<String>,
    pub ttl: Option<u32>,
}

/// GeoIP lookup request
#[derive(Debug, Clone)]
pub struct GeoIpRequest {
    pub target: String, // IP or domain
}

impl TypedMessage for GeoIpRequest {
    type Response = GeoIpResponse;
    
    fn message_type_name() -> &'static str {
        "GeoIpRequest"
    }
}

#[derive(Debug, Clone)]
pub struct GeoIpResponse {
    pub ip: String,
    pub country: String,
    pub region: String,
    pub city: String,
    pub asn: String,
}

// Macros for easy message definition

/// Macro to define a simple typed message
#[macro_export]
macro_rules! define_message {
    (
        $name:ident => $response:ty,
        $($field:ident: $type:ty),*
    ) => {
        #[derive(Debug, Clone)]
        pub struct $name {
            $(pub $field: $type,)*
        }
        
        impl $crate::plugin::typed_messages::TypedMessage for $name {
            type Response = $response;
            
            fn message_type_name() -> &'static str {
                stringify!($name)
            }
        }
    };
}

/// Macro to define an event message (no response)
#[macro_export]
macro_rules! define_event {
    (
        $name:ident,
        $($field:ident: $type:ty),*
    ) => {
        define_message!($name => (), $($field: $type),*);
    };
}