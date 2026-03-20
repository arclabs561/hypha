use hypha::{PowerMode, SporeNode};
use tempfile::tempdir;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let tmp = tempdir()?;
    let mut node = SporeNode::new(tmp.path())?;
    node.set_power_mode(PowerMode::Normal);

    // In a real app, you might listen for battery events
    // node.set_power_mode(PowerMode::LowBattery);

    node.start().await?;

    Ok(())
}
