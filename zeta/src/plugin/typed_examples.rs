//! Example plugins demonstrating typed message system

use async_trait::async_trait;
use serde::Deserialize;
use irc::proto::{Command, Message};
use irc::client::Client;
use std::any::TypeId;
use tokio::sync::mpsc;
use crate::plugin::{Plugin, PluginContext, typed::{self, TypedMessage, TypedEnvelope}};
use crate::Error;

// ===== TYPED CALCULATOR PLUGIN =====

#[derive(Deserialize)]
pub struct TypedCalculatorConfig {}

pub struct TypedCalculator {
    context: PluginContext,
    typed_receiver: mpsc::UnboundedReceiver<TypedEnvelope>,
}

#[async_trait]
impl Plugin for TypedCalculator {
    const NAME: &'static str = "typed_calculator";
    const AUTHOR: &'static str = "Typed Demo";
    const VERSION: &'static str = "2.0.0";
    
    type Config = TypedCalculatorConfig;
    
    async fn new(_config: Self::Config, context: PluginContext) -> Result<Self, Error> {
        let (_sender, receiver) = mpsc::unbounded_channel();
        
        Ok(TypedCalculator {
            context,
            typed_receiver: receiver,
        })
    }
    
    async fn run(&mut self) -> Result<(), Error> {
        // Handle typed messages
        while let Some(envelope) = self.typed_receiver.recv().await {
            self.handle_typed_envelope(envelope).await;
        }
        Ok(())
    }
    
    async fn handle_irc_message(&mut self, message: &Message, client: &Client) -> Result<(), Error> {
        if let Command::PRIVMSG(ref channel, ref msg) = message.command {
            if let Some(expression) = msg.strip_prefix(".tcalc ") {
                // Use typed calculation internally
                match self.evaluate_typed(expression).await {
                    Ok(result) => {
                        client
                            .send_privmsg(channel, format!("🧮 {} = {}", expression, result.formatted))
                            .map_err(Error::IrcClientError)?;
                            
                        // Broadcast typed event
                        let _ = self.context.notify_event_typed(
                            "calculation_performed",
                            serde_json::json!({
                                "expression": expression,
                                "result": result.result,
                                "formatted": result.formatted
                            })
                        ).await;
                    }
                    Err(e) => {
                        client
                            .send_privmsg(channel, format!("❌ Error: {}", e))
                            .map_err(Error::IrcClientError)?;
                    }
                }
            }
        }
        Ok(())
    }
}

impl TypedCalculator {
    async fn handle_typed_envelope(&self, envelope: TypedEnvelope) {
        // Handle CalculationRequest messages
        if envelope.message_type == TypeId::of::<typed::CalculationRequest>() {
            if let Ok(request) = envelope.message.downcast_ref::<typed::CalculationRequest>() {
                let result = self.evaluate_typed(&request.expression).await;
                
                if let Some(response_channel) = envelope.response_channel {
                    match result {
                        Ok(calc_result) => {
                            let _ = response_channel.send(Ok(Box::new(calc_result)));
                        }
                        Err(e) => {
                            let _ = response_channel.send(Err(format!("Calculation error: {}", e)));
                        }
                    }
                }
            }
        }
    }
    
    async fn evaluate_typed(&self, expression: &str) -> Result<typed::CalculationResponse, Error> {
        // Simple math evaluation (in real implementation, use a proper math library)
        let result = match expression {
            "2+2" => 4.0,
            "10*5" => 50.0,
            "100/4" => 25.0,
            _ => {
                return Err(Error::ConfigurationError("Unsupported expression".to_string()));
            }
        };
        
        Ok(typed::CalculationResponse {
            result,
            formatted: format!("{:.2}", result),
            expression: expression.to_string(),
        })
    }
}

// ===== TYPED HEALTH MONITOR PLUGIN =====

#[derive(Deserialize)]
pub struct TypedHealthConfig {}

pub struct TypedHealthMonitor {
    context: PluginContext,
    typed_receiver: mpsc::UnboundedReceiver<TypedEnvelope>,
}

#[async_trait]
impl Plugin for TypedHealthMonitor {
    const NAME: &'static str = "typed_health";
    const AUTHOR: &'static str = "Typed Demo";
    const VERSION: &'static str = "2.0.0";
    
    type Config = TypedHealthConfig;
    
    async fn new(_config: Self::Config, context: PluginContext) -> Result<Self, Error> {
        let (_sender, receiver) = mpsc::unbounded_channel();
        
        Ok(TypedHealthMonitor {
            context,
            typed_receiver: receiver,
        })
    }
    
    async fn run(&mut self) -> Result<(), Error> {
        // Send periodic health checks to other plugins
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // Check health of calculator plugin
                    match self.context.health_check_typed("typed_calculator").await {
                        Ok(health) => {
                            println!("🏥 Health check - {}: {:?} ({}MB)", 
                                health.plugin_name, health.status, health.memory_mb);
                        }
                        Err(e) => {
                            println!("❌ Health check failed: {}", e);
                        }
                    }
                }
                Some(envelope) = self.typed_receiver.recv() => {
                    self.handle_typed_envelope(envelope).await;
                }
            }
        }
    }
    
    async fn handle_irc_message(&mut self, message: &Message, client: &Client) -> Result<(), Error> {
        if let Command::PRIVMSG(ref channel, ref msg) = message.command {
            if msg == ".thealth" {
                // Show health of all plugins
                match self.context.health_check_typed("typed_calculator").await {
                    Ok(health) => {
                        client
                            .send_privmsg(channel, format!("🏥 {}: {:?} - {:.1}MB", 
                                health.plugin_name, health.status, health.memory_mb))
                            .map_err(Error::IrcClientError)?;
                    }
                    Err(e) => {
                        client
                            .send_privmsg(channel, format!("❌ Health check failed: {}", e))
                            .map_err(Error::IrcClientError)?;
                    }
                }
            }
        }
        Ok(())
    }
}

impl TypedHealthMonitor {
    async fn handle_typed_envelope(&self, envelope: TypedEnvelope) {
        // Handle HealthCheckRequest messages
        if envelope.message_type == TypeId::of::<typed::HealthCheckRequest>() {
            if let Ok(_request) = envelope.message.downcast_ref::<typed::HealthCheckRequest>() {
                let response = typed::HealthCheckResponse {
                    plugin_name: Self::NAME.to_string(),
                    status: typed::HealthStatus::Healthy,
                    memory_mb: 5.2,
                    uptime_seconds: 3600,
                    custom_metrics: std::collections::HashMap::new(),
                };
                
                if let Some(response_channel) = envelope.response_channel {
                    let _ = response_channel.send(Ok(Box::new(response)));
                }
            }
        }
    }
}

// ===== TYPED AGGREGATOR PLUGIN =====

#[derive(Deserialize)]
pub struct TypedAggregatorConfig {}

pub struct TypedAggregator {
    context: PluginContext,
    typed_receiver: mpsc::UnboundedReceiver<TypedEnvelope>,
    calculation_count: u64,
}

#[async_trait]
impl Plugin for TypedAggregator {
    const NAME: &'static str = "typed_aggregator";
    const AUTHOR: &'static str = "Typed Demo";
    const VERSION: &'static str = "2.0.0";
    
    type Config = TypedAggregatorConfig;
    
    async fn new(_config: Self::Config, context: PluginContext) -> Result<Self, Error> {
        let (_sender, receiver) = mpsc::unbounded_channel();
        
        Ok(TypedAggregator {
            context,
            typed_receiver: receiver,
            calculation_count: 0,
        })
    }
    
    async fn run(&mut self) -> Result<(), Error> {
        while let Some(envelope) = self.typed_receiver.recv().await {
            self.handle_typed_envelope(envelope).await;
        }
        Ok(())
    }
    
    async fn handle_irc_message(&mut self, message: &Message, client: &Client) -> Result<(), Error> {
        if let Command::PRIVMSG(ref channel, ref msg) = message.command {
            if msg == ".tstats" {
                client
                    .send_privmsg(channel, format!("📊 Total calculations: {}", self.calculation_count))
                    .map_err(Error::IrcClientError)?;
            } else if let Some(expression) = msg.strip_prefix(".tmath ") {
                // Request calculation from typed calculator
                match self.context.calculate_typed(expression).await {
                    Ok(result) => {
                        client
                            .send_privmsg(channel, format!("🧮 {} = {} (via typed messaging!)", 
                                expression, result.formatted))
                            .map_err(Error::IrcClientError)?;
                    }
                    Err(e) => {
                        client
                            .send_privmsg(channel, format!("❌ Math error: {}", e))
                            .map_err(Error::IrcClientError)?;
                    }
                }
            }
        }
        Ok(())
    }
}

impl TypedAggregator {
    async fn handle_typed_envelope(&self, envelope: TypedEnvelope) {
        // Listen for EventNotification messages about calculations
        if envelope.message_type == TypeId::of::<typed::EventNotification>() {
            if let Ok(event) = envelope.message.downcast_ref::<typed::EventNotification>() {
                if event.event_type == "calculation_performed" {
                    println!("📊 Calculation event received from {}: {:?}", event.source, event.data);
                    // In real implementation, would increment counter
                }
            }
        }
    }
}

// ===== CUSTOM TYPED MESSAGE EXAMPLES =====

// Define a custom message using the macro
crate::define_typed_message! {
    CustomRequest {
        action: String,
        parameters: serde_json::Value,
    } => CustomResponse {
        success: bool,
        data: serde_json::Value,
    }
}

// Define an event using the macro  
crate::define_typed_event! {
    UserActivity {
        user: String,
        action: String,
        timestamp: u64,
    }
}

// Usage in a plugin would be:
// let response = ctx.send_typed("target_plugin", CustomRequest { ... }).await?;
// ctx.broadcast_typed(UserActivity { ... }).await?;