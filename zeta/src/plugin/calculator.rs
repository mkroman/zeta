use std::sync::Mutex;

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use serde::Deserialize;
use thiserror::Error;

use crate::Error as ZetaError;

use super::{Author, Version, NewPlugin, PluginActor, MessageEnvelope, MessageResponse, PluginBus, ActorId};
use super::messages::{EventMessage, HealthCheckRequest};

#[derive(Error, Debug)]
pub enum Error {
    #[error("evaluation error: {0}")]
    Evaluation(String),
    #[error("could not create rink context")]
    Context,
}

#[derive(Deserialize)]
pub struct CalculatorConfig {
    // No specific config needed for calculator, but we need the struct
}

pub struct Calculator {
    ctx: Mutex<rink_core::Context>,
    bus: Option<PluginBus>,
}

#[async_trait]
impl NewPlugin for Calculator {
    const NAME: &'static str = "calculator";
    const AUTHOR: Author = Author("Mikkel Kroman <mk@maero.dk>");
    const VERSION: Version = Version("0.1.0");

    type Err = Error;
    type Config = CalculatorConfig;

    fn with_config(_config: &Self::Config) -> Self {
        let ctx = rink_core::simple_context().expect("could not create rink-rs context");

        Calculator {
            ctx: Mutex::new(ctx),
            bus: None,
        }
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref inner_message) = message.command
            && let Some(query) = inner_message.strip_prefix(".r ")
        {
            match self.eval(query) {
                Ok(result) => {
                    client
                        .send_privmsg(channel, format!("\x0310> {result}"))
                        .map_err(ZetaError::IrcClientError)?;
                    
                    // Send calculation event to other plugins
                    self.send_calculation_event(query, &result).await;
                }
                Err(err) => {
                    client
                        .send_privmsg(channel, format!("\x0310> Error: {err}"))
                        .map_err(ZetaError::IrcClientError)?;
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl PluginActor for Calculator {
    async fn handle_actor_message(&self, envelope: MessageEnvelope) -> MessageResponse {
        // Calculator can respond to health check requests
        if envelope.message.message_type() == "health_check_request" {
            // Send a health check to the health plugin as a demonstration
            if let Some(bus) = &self.bus {
                let health_request = HealthCheckRequest {
                    requester: Self::NAME.to_string(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                };
                
                let _ = bus.send_to(
                    ActorId::new(Self::NAME),
                    ActorId::new("health"),
                    Box::new(health_request),
                ).await;
            }
            return MessageResponse::Handled;
        }
        
        MessageResponse::NotHandled
    }
    
    fn message_subscriptions(&self) -> Vec<&'static str> {
        vec!["health_check_request"]
    }
}

impl Calculator {
    pub fn eval(&self, line: &str) -> Result<String, Error> {
        let mut ctx = self.ctx.lock().unwrap();

        rink_core::one_line(&mut ctx, line).map_err(Error::Evaluation)
    }
    
    pub fn set_bus(&mut self, bus: PluginBus) {
        self.bus = Some(bus);
    }
    
    async fn send_calculation_event(&self, query: &str, result: &str) {
        if let Some(bus) = &self.bus {
            let mut event_data = serde_json::Map::new();
            event_data.insert("query".to_string(), serde_json::Value::String(query.to_string()));
            event_data.insert("result".to_string(), serde_json::Value::String(result.to_string()));
            
            let event = EventMessage {
                event_type: "calculation_performed".to_string(),
                source: Self::NAME.to_string(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                data: serde_json::Value::Object(event_data),
            };
            
            let _ = bus.broadcast(
                ActorId::new(Self::NAME),
                Box::new(event),
            ).await;
        }
    }
}
