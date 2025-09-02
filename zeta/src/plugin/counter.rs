use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use serde::Deserialize;
use tokio::time::{sleep, Duration};

use crate::plugin::{Plugin, PluginContext, MessageEnvelope};
use crate::plugin::messages::{EventMessage, TextMessage};
use crate::Error;

#[derive(Deserialize)]
pub struct CounterConfig {
    pub reset_command: Option<String>,
}

pub struct Counter {
    context: PluginContext,
    count: u64,
    reset_command: String,
}

#[async_trait]
impl Plugin for Counter {
    const NAME: &'static str = "counter";
    const AUTHOR: &'static str = "Mikkel Kroman <mk@maero.dk>";
    const VERSION: &'static str = "1.0.0";
    
    type Config = CounterConfig;
    
    async fn new(config: Self::Config, context: PluginContext) -> Result<Self, Error> {
        Ok(Counter {
            context,
            count: 0,
            reset_command: config.reset_command.unwrap_or_else(|| ".reset".to_string()),
        })
    }
    
    async fn run(&mut self) -> Result<(), Error> {
        // Send periodic status updates
        let mut interval = tokio::time::interval(Duration::from_secs(300)); // Every 5 minutes
        
        loop {
            interval.tick().await;
            
            // Broadcast current count as an event
            let mut event_data = serde_json::Map::new();
            event_data.insert("current_count".to_string(), serde_json::Value::Number(self.count.into()));
            
            let event = EventMessage {
                event_type: "counter_status".to_string(),
                source: Self::NAME.to_string(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                data: serde_json::Value::Object(event_data),
            };
            
            // Broadcasting with the new system is super simple!
            let _ = self.context.broadcast(event).await;
        }
    }
    
    async fn handle_irc_message(&mut self, message: &Message, client: &Client) -> Result<(), Error> {
        if let Command::PRIVMSG(ref channel, ref msg) = message.command {
            if msg.starts_with(".count") {
                self.count += 1;
                client
                    .send_privmsg(channel, format!("Count: {}", self.count))
                    .map_err(Error::IrcClientError)?;
            } else if msg.starts_with(&self.reset_command) {
                let old_count = self.count;
                self.count = 0;
                
                client
                    .send_privmsg(channel, format!("Counter reset! (was {})", old_count))
                    .map_err(Error::IrcClientError)?;
                
                // Notify other plugins about the reset
                let mut metadata = std::collections::HashMap::new();
                metadata.insert("previous_count".to_string(), old_count.to_string());
                
                let text_msg = TextMessage {
                    content: "Counter was reset".to_string(),
                    metadata,
                };
                
                let _ = self.context.broadcast(text_msg).await;
            }
        }
        Ok(())
    }
    
    async fn handle_plugin_message(&mut self, envelope: MessageEnvelope) -> Result<bool, Error> {
        // Listen for events from other plugins
        if let Some(event) = envelope.message.as_any().downcast_ref::<EventMessage>() {
            match event.event_type.as_str() {
                "calculation_performed" => {
                    // Increment counter when someone does a calculation
                    self.count += 1;
                    return Ok(true);
                }
                _ => {}
            }
        }
        
        Ok(false)
    }
    
    fn subscriptions() -> Vec<&'static str> {
        vec!["event", "text_message"]
    }
}