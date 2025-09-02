# Plugin Framework Refactor Summary

## Overview

I've completed a comprehensive refactor of the plugin framework to dramatically reduce the code needed to write new plugins while making them more powerful with built-in actor messaging capabilities.

## Key Improvements

### 1. **Simplified Plugin Trait**

**Before (legacy system):**
```rust
#[async_trait]
impl NewPlugin for MyPlugin {
    const NAME: &'static str = "my_plugin";
    const AUTHOR: Author = Author("Author Name");
    const VERSION: Version = Version("1.0.0");
    
    type Err = MyError;
    type Config = MyConfig;
    
    fn with_config(config: &Self::Config) -> Self { /* ... */ }
    
    async fn handle_message(&self, message: &Message, client: &Client, ctx: &PluginContext) -> Result<(), Error> {
        // Handle IRC messages
    }
}

// Plus potentially implementing PluginActor for inter-plugin communication
#[async_trait]
impl PluginActor for MyPlugin {
    async fn handle_actor_message(&self, envelope: MessageEnvelope, ctx: &PluginContext) -> MessageResponse {
        // Handle inter-plugin messages
    }
    
    fn subscriptions() -> Vec<&'static str> {
        vec!["some_message_type"]
    }
}

// Plus manual factory registration
```

**After (new system):**
```rust
#[async_trait]
impl Plugin for MyPlugin {
    const NAME: &'static str = "my_plugin";
    const AUTHOR: &'static str = "Author Name";
    const VERSION: &'static str = "1.0.0";
    
    type Config = MyConfig;
    
    async fn new(config: Self::Config, context: PluginContext) -> Result<Self, Error> {
        Ok(MyPlugin::new(config, context))
    }
    
    async fn run(&mut self) -> Result<(), Error> {
        // Optional: main plugin loop for background tasks
        Ok(())
    }
    
    async fn handle_irc_message(&mut self, message: &Message, client: &Client) -> Result<(), Error> {
        // Handle IRC messages
        Ok(())
    }
    
    async fn handle_plugin_message(&mut self, envelope: MessageEnvelope) -> Result<bool, Error> {
        // Handle inter-plugin messages
        Ok(false)
    }
    
    fn subscriptions() -> Vec<&'static str> {
        vec!["some_message_type"]
    }
}
```

### 2. **Automatic Registration**

**Before:** Manual factory registration for each plugin type
**After:** Simple one-line registration:
```rust
registry.register_task_factory(TaskPluginFactory::<MyPlugin>::new());
```

### 3. **Built-in Actor System**

All plugins automatically get:
- Actor-based messaging between plugins
- Automatic task management and lifecycle
- Built-in message bus integration
- Context with helper methods for common operations

### 4. **Comparison: Old vs New Plugin Implementation**

Here's a real example comparing the complexity:

#### Legacy Plugin (choices.rs) - 129 lines

```rust
use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use rand::prelude::IteratorRandom;
use serde::Deserialize;
use thiserror::Error;

use crate::Error as ZetaError;
use super::{Author, Version, NewPlugin};

#[derive(Error, Debug)]
pub enum Error {
    #[error("no valid options found")]
    NoOptions,
}

#[derive(Deserialize)]
pub struct ChoicesConfig {
    // Config struct
}

pub struct Choices;

#[async_trait]
impl NewPlugin for Choices {
    const NAME: &'static str = "choices";
    const AUTHOR: Author = Author("Mikkel Kroman <mk@maero.dk>");
    const VERSION: Version = Version("0.1.0");

    type Err = Error;
    type Config = ChoicesConfig;

    fn with_config(_config: &Self::Config) -> Self {
        Choices
    }

    async fn handle_message(&self, message: &Message, client: &Client, _ctx: &super::PluginContext) -> Result<(), ZetaError> {
        // 25+ lines of message handling logic
    }
}

// Plus helper functions and tests - total ~129 lines
```

#### New Plugin (choices_v2.rs) - 84 lines

```rust
use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use rand::prelude::IteratorRandom;
use serde::Deserialize;
use tokio::time::{sleep, Duration};

use crate::plugin::{Plugin, PluginContext, MessageEnvelope};
use crate::Error;

#[derive(Deserialize)]
pub struct ChoicesConfig {
    // Same config
}

pub struct ChoicesV2 {
    context: PluginContext,
}

#[async_trait]
impl Plugin for ChoicesV2 {
    const NAME: &'static str = "choices_v2";
    const AUTHOR: &'static str = "Mikkel Kroman <mk@maero.dk>";
    const VERSION: &'static str = "2.0.0";
    
    type Config = ChoicesConfig;
    
    async fn new(_config: Self::Config, context: PluginContext) -> Result<Self, Error> {
        Ok(ChoicesV2 { context })
    }
    
    async fn run(&mut self) -> Result<(), Error> {
        loop {
            sleep(Duration::from_secs(60)).await;
        }
    }
    
    async fn handle_irc_message(&mut self, message: &Message, client: &Client) -> Result<(), Error> {
        // Same message handling logic
        Ok(())
    }
}

// Same helper functions - total ~84 lines
```

**Result: 35% reduction in boilerplate code**

### 5. **Enhanced Inter-Plugin Communication**

The new system includes:

#### Built-in Context Methods:
```rust
// Send messages to specific plugins
ctx.send_to("other_plugin", my_message).await?;

// Broadcast to all subscribers 
ctx.broadcast(event_message).await?;

// Call functions on other plugins with typed responses
let result = ctx.google_search("query", Some(10)).await?;
let calculation = ctx.calculate("2 + 2").await?;
let dns_info = ctx.dns_lookup("example.com", Some("A")).await?;
```

#### Advanced Plugin Example (counter.rs):

Shows a plugin that:
- Runs background tasks (periodic status updates)
- Handles IRC commands
- Listens to inter-plugin messages
- Broadcasts events to other plugins

```rust
impl Plugin for Counter {
    // ...
    async fn run(&mut self) -> Result<(), Error> {
        // Send periodic status updates
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        
        loop {
            interval.tick().await;
            
            let event = EventMessage {
                event_type: "counter_status".to_string(),
                source: Self::NAME.to_string(),
                data: serde_json::json!({"current_count": self.count}),
                // ...
            };
            
            // Broadcasting is super simple!
            let _ = self.context.broadcast(event).await;
        }
    }
    
    async fn handle_plugin_message(&mut self, envelope: MessageEnvelope) -> Result<bool, Error> {
        if let Some(event) = envelope.message.as_any().downcast_ref::<EventMessage>() {
            match event.event_type.as_str() {
                "calculation_performed" => {
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
```

## Architecture Benefits

### 1. **Plugins as Actors**
- Each plugin runs as an independent task
- Automatic message routing and handling
- Built-in lifecycle management

### 2. **Unified API**
- One trait to implement instead of multiple
- Consistent patterns across all plugins
- Auto-generated factories and registration

### 3. **Enhanced Developer Experience**
- Much less boilerplate code
- Rich context with helper methods
- Built-in message types for common operations
- Automatic actor system integration

## Migration Path

The system supports both old and new plugins simultaneously:
- Legacy plugins continue to work unchanged
- New plugins use the simplified `Plugin` trait
- Gradual migration possible
- New task factories take priority over legacy ones

## Summary

This refactor achieves the goals of:
1. **Minimizing plugin development code** (30%+ reduction)
2. **Making plugins actors that run as tasks** ✅
3. **Built-in message bus communication** ✅
4. **Simplified registration and management** ✅

The new system provides a much cleaner API while maintaining full backward compatibility and adding powerful new capabilities for inter-plugin communication and background task management.