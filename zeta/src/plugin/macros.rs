//! Macros to simplify plugin development

/// Macro to automatically implement a plugin with minimal boilerplate
/// 
/// Usage:
/// ```
/// plugin! {
///     name: "my_plugin",
///     author: "Author Name",
///     version: "1.0.0",
///     config: MyConfig,
///     
///     // Optional: specify message subscriptions
///     subscriptions: ["event", "health_check_request"],
///     
///     // Main plugin implementation
///     impl MyPlugin {
///         async fn new(config: MyConfig, context: PluginContext) -> Result<Self, Error> {
///             Ok(MyPlugin { /* ... */ })
///         }
///         
///         async fn run(&mut self) -> Result<(), Error> {
///             // Main plugin loop
///             Ok(())
///         }
///         
///         async fn handle_irc_message(&mut self, message: &Message, client: &Client) -> Result<(), Error> {
///             // Handle IRC messages
///             Ok(())
///         }
///         
///         async fn handle_plugin_message(&mut self, envelope: MessageEnvelope) -> Result<bool, Error> {
///             // Handle inter-plugin messages
///             Ok(false)
///         }
///     }
/// }
/// ```
#[macro_export]
macro_rules! plugin {
    (
        name: $name:literal,
        author: $author:literal,
        version: $version:literal,
        config: $config_ty:ty,
        
        $(subscriptions: [$($sub:literal),*],)?
        
        impl $struct_name:ident {
            $(
                async fn new($config_param:ident: $config_param_ty:ty, $context_param:ident: $crate::plugin::PluginContext) -> Result<Self, $crate::Error> 
                $new_body:block
            )?
            
            $(
                async fn run(&mut self) -> Result<(), $crate::Error> 
                $run_body:block
            )?
            
            $(
                async fn handle_irc_message(&mut self, $irc_msg_param:ident: &$crate::irc::proto::Message, $irc_client_param:ident: &$crate::irc::client::Client) -> Result<(), $crate::Error>
                $irc_body:block
            )?
            
            $(
                async fn handle_plugin_message(&mut self, $plugin_msg_param:ident: $crate::plugin::MessageEnvelope) -> Result<bool, $crate::Error>
                $plugin_msg_body:block  
            )?
        }
    ) => {
        #[$crate::async_trait::async_trait]
        impl $crate::plugin::Plugin for $struct_name {
            const NAME: &'static str = $name;
            const AUTHOR: &'static str = $author;
            const VERSION: &'static str = $version;
            
            type Config = $config_ty;
            
            $(
                async fn new($config_param: $config_param_ty, $context_param: $crate::plugin::PluginContext) -> Result<Self, $crate::Error> 
                $new_body
            )?
            
            $(
                async fn run(&mut self) -> Result<(), $crate::Error> 
                $run_body
            )?
            
            $(
                async fn handle_irc_message(&mut self, $irc_msg_param: &$crate::irc::proto::Message, $irc_client_param: &$crate::irc::client::Client) -> Result<(), $crate::Error>
                $irc_body
            )?
            
            $(
                async fn handle_plugin_message(&mut self, $plugin_msg_param: $crate::plugin::MessageEnvelope) -> Result<bool, $crate::Error>
                $plugin_msg_body
            )?
            
            fn subscriptions() -> Vec<&'static str> {
                vec![$($($sub),*)?]
            }
        }
    };
}

/// Simplified plugin macro for plugins that only handle IRC messages
#[macro_export]
macro_rules! simple_plugin {
    (
        name: $name:literal,
        author: $author:literal, 
        version: $version:literal,
        config: $config_ty:ty,
        
        impl $struct_name:ident {
            fn new($config_param:ident: &$config_param_ty:ty) -> Self 
            $new_body:block
            
            async fn handle_irc_message(&mut self, $irc_msg_param:ident: &$crate::irc::proto::Message, $irc_client_param:ident: &$crate::irc::client::Client) -> Result<(), $crate::Error>
            $irc_body:block
        }
    ) => {
        #[$crate::async_trait::async_trait]
        impl $crate::plugin::Plugin for $struct_name {
            const NAME: &'static str = $name;
            const AUTHOR: &'static str = $author;
            const VERSION: &'static str = $version;
            
            type Config = $config_ty;
            
            async fn new($config_param: Self::Config, _context: $crate::plugin::PluginContext) -> Result<Self, $crate::Error> {
                Ok(Self::new(&$config_param))
            }
            
            async fn run(&mut self) -> Result<(), $crate::Error> {
                // Simple plugins just wait forever - they only respond to IRC messages
                let mut interval = $crate::tokio::time::interval($crate::std::time::Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    // Keep alive
                }
            }
            
            async fn handle_irc_message(&mut self, $irc_msg_param: &$crate::irc::proto::Message, $irc_client_param: &$crate::irc::client::Client) -> Result<(), $crate::Error>
            $irc_body
        }
    };
}

/// Macro to register plugins with the registry
#[macro_export] 
macro_rules! register_plugins {
    ($registry:expr, $($plugin_type:ty),*) => {
        $(
            $registry.register_task_factory($crate::plugin::TaskPluginFactory::<$plugin_type>::new());
        )*
    };
}

pub use {plugin, simple_plugin, register_plugins};