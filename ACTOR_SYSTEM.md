# Actor-Based Inter-Plugin Communication System

## 🎯 Overview

This implementation adds a sophisticated async inter-plugin communication system inspired by the Actor model to the Zeta IRC bot. Plugins can now send messages to each other asynchronously without tight coupling, enabling rich plugin interactions and data sharing.

## 🏗️ Architecture

### Core Components

1. **Actor Identity System**
   - `ActorId`: Unique identifier for each plugin actor
   - Each plugin automatically gets an actor ID based on its name

2. **Message System** 
   - `PluginMessage` trait: Base trait for all messages that can be sent between actors
   - `MessageEnvelope`: Wrapper containing routing information (from, to, correlation_id)
   - `MessageResponse`: Enum for actor message handling responses

3. **Message Bus**
   - `PluginBus`: Central message routing system
   - Handles both direct messaging and broadcasting
   - Maintains actor registry and subscription mappings
   - Thread-safe with async support

4. **Actor Trait**
   - `PluginActor`: Extends `NewPlugin` with actor capabilities
   - `handle_actor_message()`: Process incoming messages
   - `message_subscriptions()`: Define which message types to receive

## 📨 Message Types

The system includes several predefined message types in `messages.rs`:

- **`HealthCheckRequest/Response`**: Health monitoring between plugins
- **`TextMessage`**: Simple text communication with metadata
- **`CommandMessage`**: Execute commands on other plugins  
- **`DataMessage`**: Share structured data with TTL support
- **`EventMessage`**: Broadcast events with timestamps

All messages implement:
- `message_type()`: Type identifier for routing
- `clone_message()`: Enable broadcasting to multiple subscribers
- `as_any()`: Runtime type identification for downcasting
- `serialize()`: Optional persistence/logging support

## 🔄 Communication Patterns

### 1. Direct Messaging
Send a message to a specific plugin:

```rust
bus.send_to(
    ActorId::new("calculator"),
    ActorId::new("health"),
    Box::new(health_request)
).await?;
```

### 2. Broadcasting
Send a message to all subscribers of a message type:

```rust
bus.broadcast(
    ActorId::new("calculator"),
    Box::new(calculation_event)
).await?;
```

### 3. Subscription-Based Delivery
Plugins declare which message types they want to receive:

```rust
fn message_subscriptions(&self) -> Vec<&'static str> {
    vec!["health_check_request", "calculation_event"]
}
```

## 🔌 Plugin Integration

### Basic Plugin (IRC-only)
Plugins that only handle IRC messages inherit default actor behavior:
```rust
impl NewPlugin for MyPlugin { ... }
// Automatically gets DynPlugin implementation with no-op actor methods
```

### Actor-Enabled Plugin
Plugins that want inter-plugin communication:
```rust
#[async_trait]
impl PluginActor for MyPlugin {
    async fn handle_actor_message(&self, envelope: MessageEnvelope) -> MessageResponse {
        // Handle incoming messages
    }
    
    fn message_subscriptions(&self) -> Vec<&'static str> {
        vec!["health_check_request"]
    }
}
```

## 🚀 Examples

### Health Plugin as Actor
The health plugin responds to health check requests:
```rust
async fn handle_actor_message(&self, envelope: MessageEnvelope) -> MessageResponse {
    if let Some(request) = envelope.message.as_any().downcast_ref::<HealthCheckRequest>() {
        let response = self.create_health_response(request);
        return MessageResponse::Reply(Box::new(response));
    }
    MessageResponse::NotHandled
}
```

### Calculator Plugin Broadcasting Events
The calculator broadcasts calculation events:
```rust
async fn send_calculation_event(&self, query: &str, result: &str) {
    if let Some(bus) = &self.bus {
        let event = EventMessage {
            event_type: "calculation_performed".to_string(),
            source: Self::NAME.to_string(),
            timestamp: now(),
            data: json!({ "query": query, "result": result }),
        };
        
        let _ = bus.broadcast(ActorId::new(Self::NAME), Box::new(event)).await;
    }
}
```

## ⚡ Features

### ✅ Implemented
- **Type-safe messaging** with compile-time guarantees
- **Async message delivery** with tokio integration  
- **Subscription-based routing** for efficient delivery
- **Message cloning** for broadcast scenarios
- **Actor lifecycle management** with automatic cleanup
- **Error isolation** - plugin failures don't crash others
- **Rich message types** with metadata support
- **Direct and broadcast** communication patterns

### 🔄 Runtime Behavior
- Messages are delivered asynchronously via unbounded channels
- Each actor runs in its own tokio task for true parallelism
- Message bus handles actor registration/deregistration automatically
- Subscription changes are handled dynamically

## 🛠️ Technical Implementation

### Dynamic Plugin System Integration
The actor system seamlessly integrates with the existing dynamic plugin loading:

1. **Registry Enhancement**: `PluginBus` integrated into `Registry`
2. **Actor Setup**: Automatic actor registration during plugin loading
3. **Message Loop**: Each plugin gets its own async message processing task
4. **Bus Access**: Plugins can access the shared message bus for sending

### Memory Safety
- All messages use `Box<dyn PluginMessage>` for heap allocation
- Actor IDs use reference counting for efficient sharing
- Message bus uses `Arc<RwLock<>>` for thread-safe access
- Automatic cleanup when plugins are unloaded

## 🎉 Benefits

1. **Loose Coupling**: Plugins don't need direct references to each other
2. **Async Performance**: Non-blocking message delivery  
3. **Type Safety**: Compile-time message type validation
4. **Extensibility**: Easy to add new message types
5. **Monitoring**: Built-in health checking and metrics
6. **Scalability**: Each plugin processes messages independently
7. **Fault Tolerance**: Actor isolation prevents cascading failures

This Actor-based system transforms the plugin architecture from a collection of independent components into a collaborative network of communicating actors, enabling sophisticated plugin interactions while maintaining the benefits of loose coupling and fault isolation.