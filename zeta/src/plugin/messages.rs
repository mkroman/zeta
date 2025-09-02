//! Common message types for inter-plugin communication

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::plugin::PluginMessage;

/// A simple text message that can be sent between plugins
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextMessage {
    pub content: String,
    pub metadata: HashMap<String, String>,
}

impl PluginMessage for TextMessage {
    fn message_type(&self) -> &'static str {
        "text_message"
    }
    
    fn clone_message(&self) -> Box<dyn PluginMessage> {
        Box::new(self.clone())
    }
    
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    
    fn serialize(&self) -> Result<Vec<u8>, crate::Error> {
        serde_json::to_vec(self).map_err(|e| {
            crate::Error::ConfigurationError(format!("Failed to serialize TextMessage: {}", e))
        })
    }
}

/// A request for health information from plugins
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckRequest {
    pub requester: String,
    pub timestamp: u64,
}

impl PluginMessage for HealthCheckRequest {
    fn message_type(&self) -> &'static str {
        "health_check_request"
    }
    
    fn clone_message(&self) -> Box<dyn PluginMessage> {
        Box::new(self.clone())
    }
    
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Response to a health check request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResponse {
    pub plugin_name: String,
    pub status: HealthStatus,
    pub uptime_seconds: u64,
    pub memory_usage_mb: f64,
    pub custom_metrics: HashMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

impl PluginMessage for HealthCheckResponse {
    fn message_type(&self) -> &'static str {
        "health_check_response"
    }
    
    fn clone_message(&self) -> Box<dyn PluginMessage> {
        Box::new(self.clone())
    }
    
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A command message to execute an action on another plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandMessage {
    pub command: String,
    pub args: Vec<String>,
    pub reply_to: Option<String>,
}

impl PluginMessage for CommandMessage {
    fn message_type(&self) -> &'static str {
        "command"
    }
    
    fn clone_message(&self) -> Box<dyn PluginMessage> {
        Box::new(self.clone())
    }
    
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A data sharing message for plugins to exchange information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataMessage {
    pub data_type: String,
    pub payload: serde_json::Value,
    pub ttl_seconds: Option<u64>,
}

impl PluginMessage for DataMessage {
    fn message_type(&self) -> &'static str {
        "data_message"
    }
    
    fn clone_message(&self) -> Box<dyn PluginMessage> {
        Box::new(self.clone())
    }
    
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Event notification message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMessage {
    pub event_type: String,
    pub source: String,
    pub timestamp: u64,
    pub data: serde_json::Value,
}

impl PluginMessage for EventMessage {
    fn message_type(&self) -> &'static str {
        "event"
    }
    
    fn clone_message(&self) -> Box<dyn PluginMessage> {
        Box::new(self.clone())
    }
    
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Function call request message for inter-plugin function calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallRequest {
    pub function_name: String,
    pub args: serde_json::Value,
    pub timeout_ms: Option<u64>,
    pub request_id: String,
}

impl PluginMessage for FunctionCallRequest {
    fn message_type(&self) -> &'static str {
        "function_call_request"
    }
    
    fn clone_message(&self) -> Box<dyn PluginMessage> {
        Box::new(self.clone())
    }
    
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    
    fn serialize(&self) -> Result<Vec<u8>, crate::Error> {
        serde_json::to_vec(self).map_err(|e| {
            crate::Error::ConfigurationError(format!("Failed to serialize FunctionCallRequest: {}", e))
        })
    }
}

/// Function call response message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallResponse {
    pub request_id: String,
    pub result: Result<serde_json::Value, String>,
    pub duration_ms: u64,
}

impl PluginMessage for FunctionCallResponse {
    fn message_type(&self) -> &'static str {
        "function_call_response"
    }
    
    fn clone_message(&self) -> Box<dyn PluginMessage> {
        Box::new(self.clone())
    }
    
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    
    fn serialize(&self) -> Result<Vec<u8>, crate::Error> {
        serde_json::to_vec(self).map_err(|e| {
            crate::Error::ConfigurationError(format!("Failed to serialize FunctionCallResponse: {}", e))
        })
    }
}

// Convenience types for specific function calls

/// Google search request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleSearchArgs {
    pub query: String,
    pub limit: Option<usize>,
}

/// Google search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// YouTube search request parameters  
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YouTubeSearchArgs {
    pub query: String,
    pub limit: Option<usize>,
}

/// YouTube video info result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YouTubeVideoResult {
    pub title: String,
    pub channel: String,
    pub view_count: u64,
    pub category: String,
    pub video_id: String,
}

/// GeoIP lookup request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoIpArgs {
    pub target: String, // IP address or domain
}

/// GeoIP lookup result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoIpResult {
    pub ip: String,
    pub country: String,
    pub region: String,
    pub city: String,
    pub asn: String,
    pub asn_name: String,
}

/// Calculator evaluation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculatorArgs {
    pub expression: String,
}

/// Calculator evaluation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculatorResult {
    pub result: f64,
    pub expression: String,
}

/// DNS dig request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigArgs {
    pub domain: String,
    pub record_type: Option<String>,
}

/// DNS dig result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigResult {
    pub domain: String,
    pub record_type: String,
    pub records: Vec<String>,
    pub ttl: Option<u32>,
}