use hypha::SporeNode;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn test_mains_power_overrides_energy_score() {
    let tmp = tempdir().unwrap();
    let node = SporeNode::new(tmp.path()).unwrap();

    // Force "exhausted" values; mains power should still pin to 1.0.
    {
        let mut meta = node.metabolism.lock().unwrap();
        if let Some(batt) = meta.as_any().downcast_mut::<hypha::BatteryMetabolism>() {
            batt.voltage = 3.2;
            batt.mah_remaining = 0.0;
            batt.is_mains = true;
        }
    }

    assert_eq!(node.energy_score(), 1.0);
    assert_eq!(node.heartbeat_interval(), Duration::from_secs(1));
}
