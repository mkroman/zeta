use std::sync::Mutex;
use std::time::Instant;

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use serde::Deserialize;
use thiserror::Error;

use crate::Error as ZetaError;

use super::{Author, Version, NewPlugin, MessageEnvelope, MessageResponse, PluginContext};
use super::messages::EventMessage;

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
        }
    }

    async fn handle_message(&self, message: &Message, client: &Client, ctx: &PluginContext) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref inner_message) = message.command
            && let Some(query) = inner_message.strip_prefix(".r ")
        {
            match self.eval(query) {
                Ok(result) => {
                    client
                        .send_privmsg(channel, format!("\x0310> {result}"))
                        .map_err(ZetaError::IrcClientError)?;
                    
                    // Send calculation event to other plugins (now very simple!)
                    self.send_calculation_event(query, &result, ctx).await;
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
    async fn handle_actor_message(&self, envelope: MessageEnvelope, _ctx: &PluginContext) -> MessageResponse {
        use crate::plugin::messages::{FunctionCallRequest, FunctionCallResponse, CalculatorArgs, CalculatorResult};
        
        // Handle function call requests
        if let Some(request) = envelope.message.as_any().downcast_ref::<FunctionCallRequest>() {
            let start_time = Instant::now();
            
            let result = match request.function_name.as_str() {
                "evaluate" => {
                    // Parse arguments
                    match serde_json::from_value::<CalculatorArgs>(request.args.clone()) {
                        Ok(args) => {
                            // Perform the calculation
                            match self.eval(&args.expression) {
                                Ok(result_string) => {
                                    // Try to parse the result as a float
                                    let numeric_result = result_string
                                        .split_whitespace()
                                        .next()
                                        .and_then(|s| s.parse::<f64>().ok())
                                        .unwrap_or(0.0);
                                    
                                    let calculator_result = CalculatorResult {
                                        result: numeric_result,
                                        expression: args.expression.clone(),
                                    };
                                    
                                    Ok(serde_json::to_value(calculator_result).unwrap())
                                }
                                Err(e) => Err(format!("Calculation failed: {}", e))
                            }
                        }
                        Err(e) => Err(format!("Invalid arguments for evaluate: {}", e))
                    }
                }
                _ => Err(format!("Unknown function: {}", request.function_name))
            };
            
            let duration = start_time.elapsed();
            let response = FunctionCallResponse {
                request_id: request.request_id.clone(),
                result,
                duration_ms: duration.as_millis() as u64,
            };
            
            return MessageResponse::Reply(Box::new(response));
        }
        
        MessageResponse::NotHandled
    }
    
    fn subscriptions() -> Vec<&'static str> {
        vec!["function_call_request"]
    }
}

impl Calculator {
    pub fn eval(&self, line: &str) -> Result<String, Error> {
        let mut ctx = self.ctx.lock().unwrap();

        rink_core::one_line(&mut ctx, line).map_err(Error::Evaluation)
    }
    
    async fn send_calculation_event(&self, query: &str, result: &str, ctx: &PluginContext) {
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
        
        // Broadcasting is now super simple with context!
        let _ = ctx.broadcast(event).await;
    }
}
