//! Simplified typed message system for type-safe plugin communication

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use serde::{Serialize, Deserialize};
use crate::Error;

/// Core trait for typed messages between plugins
pub trait TypedMessage: Send + Sync + Debug + Clone + 'static {
    /// The response type for this message
    type Response: Send + Sync + Debug + Clone + 'static;
    
    /// Message type identifier for routing
    fn type_name() -> &'static str where Self: Sized;
}

/// Envelope for typed messages with routing information
#[derive(Debug)]
pub struct TypedEnvelope {
    pub from: String,
    pub to: String,
    pub message: Box<dyn Any + Send + Sync>,
    pub message_type: TypeId,
    pub response_channel: Option<oneshot::Sender<Result<Box<dyn Any + Send + Sync>, String>>>,
    pub correlation_id: String,
}

/// Handler trait for plugins to handle typed messages
#[async_trait]
pub trait TypedHandler<M: TypedMessage>: Send + Sync + 'static {
    async fn handle(&self, message: M) -> Result<M::Response, Error>;
}

/// Typed message bus for inter-plugin communication
#[derive(Default, Clone)]
pub struct TypedMessageBus {
    /// Plugin message channels
    senders: Arc<tokio::sync::RwLock<HashMap<String, mpsc::UnboundedSender<TypedEnvelope>>>>,
    /// Message type handlers per plugin
    handlers: Arc<tokio::sync::RwLock<HashMap<String, Vec<TypeId>>>>,
}

impl TypedMessageBus {
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Register a plugin with the typed message bus
    pub async fn register_plugin(
        &self,
        plugin_name: String,
        sender: mpsc::UnboundedSender<TypedEnvelope>,
        supported_types: Vec<TypeId>,
    ) {
        {
            let mut senders = self.senders.write().await;
            senders.insert(plugin_name.clone(), sender);
        }
        {
            let mut handlers = self.handlers.write().await;
            handlers.insert(plugin_name, supported_types);
        }
    }
    
    /// Send a typed message and wait for response
    pub async fn send_message<M: TypedMessage>(
        &self,
        from: &str,
        to: &str,
        message: M,
    ) -> Result<M::Response, Error> {
        let correlation_id = uuid::Uuid::new_v4().to_string();
        let (response_tx, response_rx) = oneshot::channel();
        
        let envelope = TypedEnvelope {
            from: from.to_string(),
            to: to.to_string(),
            message: Box::new(message),
            message_type: TypeId::of::<M>(),
            response_channel: Some(response_tx),
            correlation_id,
        };
        
        let senders = self.senders.read().await;
        if let Some(sender) = senders.get(to) {
            sender.send(envelope).map_err(|_| {
                Error::ConfigurationError(format!("Failed to send message to {}", to))
            })?;
            
            // Wait for response
            match response_rx.await {
                Ok(Ok(response_any)) => {
                    if let Ok(response) = response_any.downcast::<M::Response>() {
                        Ok(*response)
                    } else {
                        Err(Error::ConfigurationError("Invalid response type".to_string()))
                    }
                }
                Ok(Err(e)) => Err(Error::ConfigurationError(e)),
                Err(_) => Err(Error::ConfigurationError("Response channel closed".to_string())),
            }
        } else {
            Err(Error::ConfigurationError(format!("Plugin not found: {}", to)))
        }
    }
    
    /// Send a message without waiting for response (fire-and-forget)
    pub async fn send_event<M: TypedMessage>(
        &self,
        from: &str,
        to: &str,
        message: M,
    ) -> Result<(), Error> {
        let envelope = TypedEnvelope {
            from: from.to_string(),
            to: to.to_string(),
            message: Box::new(message),
            message_type: TypeId::of::<M>(),
            response_channel: None,
            correlation_id: uuid::Uuid::new_v4().to_string(),
        };
        
        let senders = self.senders.read().await;
        if let Some(sender) = senders.get(to) {
            sender.send(envelope).map_err(|_| {
                Error::ConfigurationError(format!("Failed to send event to {}", to))
            })?;
            Ok(())
        } else {
            Err(Error::ConfigurationError(format!("Plugin not found: {}", to)))
        }
    }
    
    /// Broadcast a message to all plugins that support it
    pub async fn broadcast<M: TypedMessage>(
        &self,
        from: &str,
        message: M,
    ) -> Result<Vec<String>, Error> {
        let message_type = TypeId::of::<M>();
        let handlers = self.handlers.read().await;
        let senders = self.senders.read().await;
        
        let mut sent_to = Vec::new();
        
        for (plugin_name, supported_types) in handlers.iter() {
            if plugin_name != from && supported_types.contains(&message_type) {
                if let Some(sender) = senders.get(plugin_name) {
                    let envelope = TypedEnvelope {
                        from: from.to_string(),
                        to: plugin_name.clone(),
                        message: Box::new(message.clone()),
                        message_type,
                        response_channel: None,
                        correlation_id: uuid::Uuid::new_v4().to_string(),
                    };
                    
                    if sender.send(envelope).is_ok() {
                        sent_to.push(plugin_name.clone());
                    }
                }
            }
        }
        
        Ok(sent_to)
    }
}

// Built-in typed messages

/// Health check request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckRequest {
    pub requester: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResponse {
    pub plugin_name: String,
    pub status: HealthStatus,
    pub memory_mb: f64,
    pub uptime_seconds: u64,
    pub custom_metrics: HashMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

impl TypedMessage for HealthCheckRequest {
    type Response = HealthCheckResponse;
    fn type_name() -> &'static str { "HealthCheckRequest" }
}

/// Generic event notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventNotification {
    pub event_type: String,
    pub source: String,
    pub data: serde_json::Value,
    pub timestamp: u64,
}

impl TypedMessage for EventNotification {
    type Response = (); // Events don't return responses
    fn type_name() -> &'static str { "EventNotification" }
}

/// Google search request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleSearchRequest {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleSearchResponse {
    pub results: Vec<SearchResult>,
    pub total_results: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

impl TypedMessage for GoogleSearchRequest {
    type Response = GoogleSearchResponse;
    fn type_name() -> &'static str { "GoogleSearchRequest" }
}

/// Calculator evaluation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculationRequest {
    pub expression: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculationResponse {
    pub result: f64,
    pub formatted: String,
    pub expression: String,
}

impl TypedMessage for CalculationRequest {
    type Response = CalculationResponse;
    fn type_name() -> &'static str { "CalculationRequest" }
}

/// DNS lookup request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsLookupRequest {
    pub domain: String,
    pub record_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsLookupResponse {
    pub domain: String,
    pub record_type: String,
    pub records: Vec<String>,
    pub ttl: Option<u32>,
}

impl TypedMessage for DnsLookupRequest {
    type Response = DnsLookupResponse;
    fn type_name() -> &'static str { "DnsLookupRequest" }
}

/// GeoIP lookup request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoIpRequest {
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoIpResponse {
    pub ip: String,
    pub country: String,
    pub region: String,
    pub city: String,
    pub asn: String,
    pub asn_name: String,
}

impl TypedMessage for GeoIpRequest {
    type Response = GeoIpResponse;
    fn type_name() -> &'static str { "GeoIpRequest" }
}

/// Weather information request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherRequest {
    pub location: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherResponse {
    pub location: String,
    pub temperature_c: f64,
    pub description: String,
    pub humidity: Option<u32>,
    pub wind_speed: Option<f64>,
}

impl TypedMessage for WeatherRequest {
    type Response = WeatherResponse;
    fn type_name() -> &'static str { "WeatherRequest" }
}

// Macros for easy typed message definition

/// Define a simple request-response message pair
#[macro_export]
macro_rules! define_typed_message {
    (
        $request:ident {
            $($req_field:ident: $req_type:ty),* $(,)?
        } => $response:ident {
            $($resp_field:ident: $resp_type:ty),* $(,)?
        }
    ) => {
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        pub struct $request {
            $(pub $req_field: $req_type,)*
        }
        
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        pub struct $response {
            $(pub $resp_field: $resp_type,)*
        }
        
        impl $crate::plugin::typed::TypedMessage for $request {
            type Response = $response;
            fn type_name() -> &'static str { stringify!($request) }
        }
    };
}

/// Define an event message (no response)
#[macro_export]
macro_rules! define_typed_event {
    (
        $event:ident {
            $($field:ident: $type:ty),* $(,)?
        }
    ) => {
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        pub struct $event {
            $(pub $field: $type,)*
        }
        
        impl $crate::plugin::typed::TypedMessage for $event {
            type Response = ();
            fn type_name() -> &'static str { stringify!($event) }
        }
    };
}