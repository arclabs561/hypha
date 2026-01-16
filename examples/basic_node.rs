use vire::{SporeNode, PowerMode};
use tracing_subscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let mut node = SporeNode::new();
    node.set_power_mode(PowerMode::Normal);
    
    // In a real app, you might listen for battery events
    // node.set_power_mode(PowerMode::LowBattery);

    node.start().await?;

    Ok(())
}
