use hypha::{PowerMode, SporeNode};
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn test_heartbeat_interval_by_power_mode() {
    let tmp = tempdir().unwrap();
    let mut node = SporeNode::new(tmp.path()).unwrap();

    node.set_power_mode(PowerMode::Normal);
    assert_eq!(node.heartbeat_interval(), Duration::from_secs(1));

    node.set_power_mode(PowerMode::LowBattery);
    assert_eq!(node.heartbeat_interval(), Duration::from_secs(10));

    node.set_power_mode(PowerMode::Critical);
    assert_eq!(node.heartbeat_interval(), Duration::from_secs(60));
}
