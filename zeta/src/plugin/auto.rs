//! Automatic plugin registration and advanced macros using third-party crates

use std::collections::HashMap;
use std::any::TypeId;
use figment::value::Value;
use crate::{Error, plugin::{Plugin, PluginContext, PluginBus, ActorId, TaskPluginFactory}};

/// Plugin descriptor for automatic registration
pub struct PluginDescriptor {
    pub name: &'static str,
    pub author: &'static str,
    pub version: &'static str,
    pub subscriptions: &'static [&'static str],
    pub factory_fn: fn() -> Box<dyn PluginFactoryTrait>,
}

/// Trait for plugin factories that can be stored in the inventory
pub trait PluginFactoryTrait: Send + Sync {
    fn name(&self) -> &'static str;
    fn subscriptions(&self) -> Vec<&'static str>;
    fn create_plugin(&self, config: &Value, bus: PluginBus) -> Result<Box<dyn std::any::Any + Send>, Error>;
}

/// Use linkme for distributed plugin collection
#[linkme::distributed_slice]
pub static PLUGIN_REGISTRY: [PluginDescriptor];

/// Generic plugin factory that works for any Plugin type
pub struct AutoPluginFactory<P: Plugin> {
    _phantom: std::marker::PhantomData<P>,
}

impl<P: Plugin> AutoPluginFactory<P> {
    pub fn new() -> Self {
        Self { _phantom: std::marker::PhantomData }
    }
}

impl<P: Plugin> PluginFactoryTrait for AutoPluginFactory<P> {
    fn name(&self) -> &'static str {
        P::NAME
    }
    
    fn subscriptions(&self) -> Vec<&'static str> {
        P::subscriptions()
    }
    
    fn create_plugin(&self, config: &Value, bus: PluginBus) -> Result<Box<dyn std::any::Any + Send>, Error> {
        let config: P::Config = config.deserialize().map_err(|e| {
            Error::ConfigurationError(format!(
                "Failed to deserialize config for {}: {}",
                P::NAME,
                e
            ))
        })?;
        
        let actor_id = ActorId::new(P::NAME);
        let context = PluginContext::new(bus, actor_id);
        
        // We need to do this in an async context, but this trait is sync
        // This is a limitation - we'll need to handle this differently
        Ok(Box::new(TaskPluginFactory::<P>::new()))
    }
}

/// Ultra-simple plugin registration macro using paste and inventory
#[macro_export]
macro_rules! auto_plugin {
    (
        $struct_name:ident,
        name = $name:literal,
        author = $author:literal,
        version = $version:literal,
        config = $config:ty
        $(, subscriptions = [$($sub:literal),*])?
    ) => {
        // Auto-submit to linkme registry
        #[linkme::distributed_slice($crate::plugin::auto::PLUGIN_REGISTRY)]
        static __PLUGIN_DESCRIPTOR: $crate::plugin::auto::PluginDescriptor = 
            $crate::plugin::auto::PluginDescriptor {
                name: $name,
                author: $author,
                version: $version,
                subscriptions: &[$($($sub),*)?],
                factory_fn: || Box::new($crate::plugin::auto::AutoPluginFactory::<$struct_name>::new()),
            };
    };
}

/// Even simpler macro for IRC-only plugins
#[macro_export] 
macro_rules! simple_irc_plugin {
    (
        $struct_name:ident,
        name = $name:literal,
        author = $author:literal,
        config = $config:ty,
        
        fn new(config: $config_param:ident) -> $ret:ty $new_body:block
        
        async fn handle_irc($message_param:ident: &Message, $client_param:ident: &Client) -> Result<(), Error> $handle_body:block
    ) => {
        pub struct $struct_name {
            _config: $config,
        }
        
        #[async_trait::async_trait]
        impl $crate::plugin::Plugin for $struct_name {
            const NAME: &'static str = $name;
            const AUTHOR: &'static str = $author;
            const VERSION: &'static str = "1.0.0";
            
            type Config = $config;
            
            async fn new(config: Self::Config, _context: $crate::plugin::PluginContext) -> Result<Self, $crate::Error> {
                let $config_param = config;
                let result: $ret = $new_body;
                Ok(result)
            }
            
            async fn run(&mut self) -> Result<(), $crate::Error> {
                // IRC-only plugins just wait
                use tokio::time::{sleep, Duration};
                loop {
                    sleep(Duration::from_secs(60)).await;
                }
            }
            
            async fn handle_irc_message(&mut self, $message_param: &Message, $client_param: &Client) -> Result<(), $crate::Error> {
                $handle_body
            }
        }
        
        $crate::plugin!(
            $struct_name,
            name = $name,
            author = $author,
            version = "1.0.0",
            config = $config
        );
    };
}

/// Service injection traits to reduce boilerplate
pub trait HttpService {
    fn http_client(&self) -> &reqwest::Client;
}

pub trait DatabaseService {
    fn db_pool(&self) -> &sqlx::PgPool;
}

/// Context with injected services
pub struct EnhancedPluginContext {
    pub base: PluginContext,
    pub http: Option<&'static reqwest::Client>,
    pub db: Option<&'static sqlx::PgPool>,
}

impl EnhancedPluginContext {
    pub fn new(base: PluginContext) -> Self {
        Self {
            base,
            http: None,
            db: None,
        }
    }
    
    /// Get HTTP client or create default
    pub fn http(&self) -> reqwest::Client {
        self.http.cloned().unwrap_or_else(|| {
            reqwest::Client::builder()
                .user_agent("Zeta IRC Bot")
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client")
        })
    }
}

/// Auto-configure plugins with common dependencies
#[macro_export]
macro_rules! plugin_with_services {
    (
        $struct_name:ident,
        name = $name:literal,
        services = [$($service:ident),*],
        
        impl {
            $($method:item)*
        }
    ) => {
        pub struct $struct_name {
            context: $crate::plugin::auto::EnhancedPluginContext,
            $($service: $service,)*
        }
        
        $($method)*
        
        $crate::plugin!(
            $struct_name,
            name = $name,
            author = "Generated",
            version = "1.0.0",
            config = ()
        );
    };
}

pub use {auto_plugin, simple_irc_plugin, plugin_with_services};