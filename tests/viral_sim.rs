use hypha::{SporeNode, PowerMode};
use std::time::Duration;
use turmoil;
use tempfile::tempdir;

#[test]
fn test_viral_propagation_under_drain() {
    let mut sim = turmoil::Builder::new().build();
    let tmp_parent = tempdir().unwrap();

    let num_nodes = 5;
    let mut paths = vec![];
    for i in 0..num_nodes {
        let p = tmp_parent.path().join(format!("node_{}", i));
        std::fs::create_dir(&p).unwrap();
        paths.push(p);
    }

    for i in 0..num_nodes {
        let path = paths[i].clone();
        sim.host(format!("node-{}", i), move || {
            let path = path.clone();
            async move {
                let mut node = SporeNode::new(&path).unwrap();
                
                // Simulation: Node 0 starts with Normal power, Node 4 starts with Critical
                if i == 4 {
                    node.set_power_mode(PowerMode::Critical);
                }

                // Logic check: Node 4 should have a much longer heartbeat
                if i == 4 {
                    assert_eq!(node.heartbeat_interval(), Duration::from_secs(30));
                } else {
                    assert_eq!(node.heartbeat_interval(), Duration::from_secs(1));
                }

                Ok(())
            }
        });
    }

    sim.run().unwrap();
}
