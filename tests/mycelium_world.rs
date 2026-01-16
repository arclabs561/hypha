use hypha::SporeNode;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn test_hypha_mycelium_world() {
    let mut sim = turmoil::Builder::new().build();
    let tmp_root = tempdir().unwrap();

    // Spore A: Mains powered, High availability
    let path_a = tmp_root.path().join("spore_a");
    std::fs::create_dir(&path_a).unwrap();
    sim.host("spore-a", move || {
        let path = path_a.clone();
        async move {
            let node = SporeNode::new(&path).unwrap();
            assert_eq!(node.heartbeat_interval(), Duration::from_secs(1));
            Ok(())
        }
    });

    // Spore B: Solar powered, Critical energy
    let path_b = tmp_root.path().join("spore_b");
    std::fs::create_dir(&path_b).unwrap();
    sim.host("spore-b", move || {
        let path = path_b.clone();
        async move {
            let node = SporeNode::new(&path).unwrap();

            // Artificial drain
            {
                let mut state = node.physical_state.lock().unwrap();
                state.voltage = 3.2;
            }

            // Should be in Critical pulse
            assert_eq!(node.heartbeat_interval(), Duration::from_secs(60));
            Ok(())
        }
    });

    sim.run().unwrap();
}
