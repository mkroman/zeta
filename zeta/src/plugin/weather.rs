//! Weather plugin demonstrating service injection and advanced patterns

use serde::Deserialize;
use async_trait::async_trait;
use irc::proto::{Command, Message};
use irc::client::Client;
use crate::plugin::{Plugin, PluginContext, MessageEnvelope};
use crate::Error;

#[derive(Deserialize)]
pub struct WeatherConfig {
    pub api_key: String,
    pub default_location: Option<String>,
}

pub struct Weather {
    context: PluginContext,
    config: WeatherConfig,
    http: reqwest::Client,
}

#[async_trait]
impl Plugin for Weather {
    const NAME: &'static str = "weather";
    const AUTHOR: &'static str = "Zeta";
    const VERSION: &'static str = "1.0.0";
    
    type Config = WeatherConfig;
    
    async fn new(config: Self::Config, context: PluginContext) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .user_agent("Zeta Weather Bot/1.0")
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| Error::ConfigurationError(format!("HTTP client error: {}", e)))?;
            
        Ok(Weather {
            context,
            config,
            http,
        })
    }
    
    async fn run(&mut self) -> Result<(), Error> {
        // Could send periodic weather updates
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            // Send weather alerts or daily forecasts
        }
    }
    
    async fn handle_irc_message(&mut self, message: &Message, client: &Client) -> Result<(), Error> {
        if let Command::PRIVMSG(ref channel, ref msg) = message.command {
            if let Some(location) = msg.strip_prefix(".weather ") {
                match self.get_weather(location).await {
                    Ok(weather) => {
                        client
                            .send_privmsg(channel, weather)
                            .map_err(Error::IrcClientError)?;
                    }
                    Err(e) => {
                        client
                            .send_privmsg(channel, format!("⚠️ Weather error: {}", e))
                            .map_err(Error::IrcClientError)?;
                    }
                }
            }
        }
        Ok(())
    }
    
    async fn handle_plugin_message(&mut self, envelope: MessageEnvelope) -> Result<bool, Error> {
        // Could respond to location requests from other plugins
        Ok(false)
    }
}

impl Weather {
    async fn get_weather(&self, location: &str) -> Result<String, Error> {
        let url = format!(
            "https://api.openweathermap.org/data/2.5/weather?q={}&appid={}&units=metric",
            location, self.config.api_key
        );
        
        let response: serde_json::Value = self.http
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::ConfigurationError(format!("Request failed: {}", e)))?
            .json()
            .await
            .map_err(|e| Error::ConfigurationError(format!("JSON parse failed: {}", e)))?;
        
        if let Some(main) = response.get("main") {
            let temp = main.get("temp").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let desc = response
                .get("weather")
                .and_then(|w| w.get(0))
                .and_then(|w| w.get("description"))
                .and_then(|d| d.as_str())
                .unwrap_or("unknown");
                
            Ok(format!("🌤️ {}: {:.1}°C, {}", location, temp, desc))
        } else {
            Err(Error::ConfigurationError("Invalid weather data".to_string()))
        }
    }
}

// Auto-register the plugin
crate::auto_plugin!(
    Weather,
    name = "weather",
    author = "Zeta",
    version = "1.0.0",
    config = WeatherConfig
);