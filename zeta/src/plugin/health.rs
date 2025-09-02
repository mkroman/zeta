use std::fmt::Display;

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use psutil::process::Process;
use serde::Deserialize;
use thiserror::Error;
use tokio::runtime::Handle;

use crate::Error as ZetaError;

use super::{Author, Version, NewPlugin, MessageEnvelope, MessageResponse, PluginContext};
use super::messages::{HealthCheckRequest, HealthCheckResponse, HealthStatus};

#[derive(Error, Debug)]
pub enum Error {
    #[error("health check failed")]
    HealthCheck,
}

#[derive(Deserialize)]
pub struct HealthConfig {
    // No specific config needed for health, but we need the struct
}

pub struct Health;

#[async_trait]
impl NewPlugin for Health {
    const NAME: &'static str = "health";
    const AUTHOR: Author = Author("Mikkel Kroman <mk@maero.dk>");
    const VERSION: Version = Version("0.1.0");

    type Err = Error;
    type Config = HealthConfig;

    fn with_config(_config: &Self::Config) -> Self {
        Health
    }

    async fn handle_message(&self, message: &Message, client: &Client, _ctx: &PluginContext) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref message) = message.command
            && message.starts_with(".health")
        {
            client.send_privmsg(channel, self.to_string())?;
        }

        Ok(())
    }
}

#[async_trait]
impl PluginActor for Health {
    async fn handle_actor_message(&self, envelope: MessageEnvelope, _ctx: &PluginContext) -> MessageResponse {
        // Handle health check requests from other plugins
        if let Some(health_request) = envelope.message.as_any().downcast_ref::<HealthCheckRequest>() {
            let response = self.create_health_response(health_request);
            return MessageResponse::Reply(Box::new(response));
        }
        
        MessageResponse::NotHandled
    }
    
    fn subscriptions() -> Vec<&'static str> {
        vec!["health_check_request"]
    }
}

impl Health {
    fn create_health_response(&self, _request: &HealthCheckRequest) -> HealthCheckResponse {
        let (memory_usage, status) = if let Ok(proc) = Process::current()
            && let Ok(memory) = proc.memory_info()
        {
            let rss_mib = memory.rss() as f64 / 1024. / 1024.;
            (rss_mib, HealthStatus::Healthy)
        } else {
            (0.0, HealthStatus::Unhealthy)
        };

        let metrics = Handle::current().metrics();
        let mut custom_metrics = std::collections::HashMap::new();
        custom_metrics.insert("num_workers".to_string(), metrics.num_workers() as f64);
        custom_metrics.insert("alive_tasks".to_string(), metrics.num_alive_tasks() as f64);
        custom_metrics.insert("global_queue_depth".to_string(), metrics.global_queue_depth() as f64);

        HealthCheckResponse {
            plugin_name: Self::NAME.to_string(),
            status,
            uptime_seconds: 0, // TODO: track actual uptime
            memory_usage_mb: memory_usage,
            custom_metrics,
        }
    }
}

impl Display for Health {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "\x0310>\x02\x03 Health:\x02\x0310 ")?;

        if let Ok(proc) = Process::current()
            && let Ok(memory) = proc.memory_info()
        {
            let rss_mib = memory.rss() as f64 / 1024. / 1024.;
            let vms_mib = memory.vms() as f64 / 1024. / 1024.;
            let shared_mib = memory.shared() as f64 / 1024. / 1024.;

            write!(
                fmt,
                "Memory usage:\x0f {rss_mib:.2} MiB\x0310 (VMS:\x0f {vms_mib:.2} MiB\x0310 Shared:\x0f {shared_mib:.2} MiB\x0310)",
            )?;
        }

        let metrics = Handle::current().metrics();
        let num_workers = metrics.num_workers();
        let num_alive_tasks = metrics.num_alive_tasks();
        let global_queue_depth = metrics.global_queue_depth();

        write!(fmt, "Workers:\x0f {num_workers}\x0310 ")?;
        write!(fmt, "Tasks:\x0f {num_alive_tasks}\x0310 ")?;
        write!(fmt, "(\x0f{global_queue_depth}\x0310 scheduled) ")?;

        Ok(())
    }
}
