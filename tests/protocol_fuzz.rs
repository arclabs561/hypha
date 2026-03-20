use hypha::mesh::{MeshConfig, MeshControl, TopicMesh};
use hypha::{Bid, Capability, SporeNode, Task};
use proptest::prelude::*;
use std::time::Duration;
use tempfile::tempdir;

proptest! {
    #[test]
    fn test_process_task_bundle_fuzz(
        task_id in "\\PC*",
        source_id in "\\PC*",
        priority in any::<u8>(),
        reach_intensity in -100.0f32..100.0f32, // Intentional range outside [0,1]
        token in prop::option::of("\\PC*"),
        energy_score in -10.0f32..10.0f32,
        cost in -1000.0f32..1000.0f32,
        req_cap_val in any::<u32>(),
    ) {
        let dir = tempdir().unwrap();
        // Setup node (expensive, but proptest handles it)
        let mut node = SporeNode::new(dir.path()).unwrap();
        // Give node the capability so it might bid
        node.add_capability(Capability::Compute(req_cap_val));

        let task = Task {
            id: task_id.clone(),
            required_capability: Capability::Compute(req_cap_val),
            priority,
            reach_intensity,
            source_id,
            auth_token: token,
        };

        let mut known_bids = vec![
            Bid {
                task_id: task_id.clone(),
                bidder_id: "other".to_string(),
                energy_score,
                cost_mah: cost,
            }
        ];

        // This should never panic, even with weird floats
        let _ = node.process_task_bundle(&task, &mut known_bids);
    }

    #[test]
    fn test_task_diffusion_fuzz(
        conductivity in -10.0f32..10.0f32,
        neighbor_energy in -10.0f32..10.0f32,
        neighbor_pressure in -10.0f32..10.0f32,
        reach in -10.0f32..10.0f32,
    ) {
        let task = Task {
            id: "t".into(),
            required_capability: Capability::Compute(1),
            priority: 1,
            reach_intensity: reach,
            source_id: "s".into(),
            auth_token: None,
        };

        let _new_reach = task.diffuse(conductivity, neighbor_energy, neighbor_pressure);
    }

    #[test]
    fn test_topic_mesh_state_machine_fuzz(
        // Op: 0=Heartbeat, 1=Graft, 2=Prune, 3=Spike, 4=AddPeer
        ops in prop::collection::vec(
            (0..5u8, "[a-z]{1,5}", 0.0f32..1.0f32),
            1..50
        )
    ) {
        let config = MeshConfig::default();
        let mut mesh = TopicMesh::new("fuzz".to_string(), config);

        for (op_type, id, val) in ops {
            match op_type {
                0 => {
                    let _ = mesh.heartbeat();
                },
                1 => {
                    let _ = mesh.handle_control(&id, MeshControl::Graft { topic: "fuzz".to_string() });
                },
                2 => {
                    let _ = mesh.handle_control(&id, MeshControl::Prune { topic: "fuzz".to_string(), backoff: Duration::from_secs(10) });
                },
                3 => {
                    // Spike intensity from float 0..1 mapped to 0..255
                    let intensity = (val * 255.0) as u8;
                    mesh.handle_spike(&id, intensity);
                },
                4 => {
                    mesh.add_peer(id, val);
                },
                _ => unreachable!(),
            }

            // Invariants
            for peer in &mesh.mesh_peers {
                assert!(!mesh.backoff.contains_key(peer), "Backoff peer {} found in mesh", peer);
                assert!(mesh.known_peers.contains_key(peer), "Mesh peer {} missing from known", peer);
                assert!(mesh.known_peers.get(peer).unwrap().in_mesh, "Mesh peer {} sync error", peer);
            }
            assert!(!mesh.local_pressure.is_nan());
        }
    }
}
