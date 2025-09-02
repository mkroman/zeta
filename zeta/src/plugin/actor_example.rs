//! Example demonstrating the Actor-based inter-plugin communication system

use tokio::time::{sleep, Duration};
use crate::plugin::{
    PluginBus, ActorId, MessageEnvelope,
    messages::{HealthCheckRequest, EventMessage, TextMessage},
};

/// Example showing how plugins can communicate using the actor system
pub async fn demo_actor_communication() {
    println!("🎭 Actor-based Plugin Communication Demo");
    
    // Create the message bus
    let bus = PluginBus::new();
    
    // Example 1: Direct message sending
    println!("\n📨 Direct Message Example:");
    
    // Create a health check request
    let health_request = HealthCheckRequest {
        requester: "demo".to_string(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    };
    
    // Send directly to health plugin
    if let Err(e) = bus.send_to(
        ActorId::new("demo"),
        ActorId::new("health"), 
        Box::new(health_request)
    ).await {
        println!("❌ Failed to send health check: {}", e);
    } else {
        println!("✅ Health check request sent to health plugin");
    }
    
    // Example 2: Broadcasting events
    println!("\n📢 Broadcast Example:");
    
    let mut event_data = serde_json::Map::new();
    event_data.insert("demo_data".to_string(), serde_json::Value::String("Hello from actor system!".to_string()));
    
    let event = EventMessage {
        event_type: "demo_event".to_string(),
        source: "demo".to_string(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        data: serde_json::Value::Object(event_data),
    };
    
    if let Err(e) = bus.broadcast(
        ActorId::new("demo"),
        Box::new(event)
    ).await {
        println!("❌ Failed to broadcast event: {}", e);
    } else {
        println!("✅ Event broadcasted to all subscribers");
    }
    
    // Example 3: Text message communication
    println!("\n💬 Text Message Example:");
    
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("priority".to_string(), "high".to_string());
    metadata.insert("category".to_string(), "notification".to_string());
    
    let text_msg = TextMessage {
        content: "This is a demonstration of the actor-based messaging system".to_string(),
        metadata,
    };
    
    if let Err(e) = bus.broadcast(
        ActorId::new("demo"),
        Box::new(text_msg)
    ).await {
        println!("❌ Failed to send text message: {}", e);
    } else {
        println!("✅ Text message sent to all text message subscribers");
    }
    
    sleep(Duration::from_millis(100)).await;
    println!("\n🎉 Actor communication demo complete!");
}

/// Example of a simple message handler
pub async fn handle_message_example(envelope: MessageEnvelope) {
    println!("📥 Received message:");
    println!("  From: {}", envelope.from.as_str());
    println!("  To: {}", envelope.to.as_str());
    println!("  Type: {}", envelope.message.message_type());
    println!("  Message: {:?}", envelope.message);
    
    // Handle different message types
    match envelope.message.message_type() {
        "health_check_request" => {
            if let Some(request) = envelope.message.as_any().downcast_ref::<HealthCheckRequest>() {
                println!("  Health check from: {}", request.requester);
            }
        }
        "event" => {
            if let Some(event) = envelope.message.as_any().downcast_ref::<EventMessage>() {
                println!("  Event type: {}", event.event_type);
                println!("  Source: {}", event.source);
            }
        }
        "text_message" => {
            if let Some(text) = envelope.message.as_any().downcast_ref::<TextMessage>() {
                println!("  Content: {}", text.content);
                println!("  Metadata: {:?}", text.metadata);
            }
        }
        _ => {
            println!("  Unknown message type");
        }
    }
}