//! Gossip Mesh Management for Hypha
//!
//! Implements gossipsub-style mesh management with energy-aware peer scoring.
//! Key concepts:
//!
//! - **D parameters**: Target mesh degree (D=6), bounds (D_low=4, D_high=12)
//! - **Peer scoring**: Energy scores influence mesh membership
//! - **Opportunistic grafting**: Recover from degraded mesh states
//! - **Flood publishing**: Own messages bypass mesh for eclipse resistance
//!
//! This module provides a simulation-friendly mesh layer that can be evaluated
//! without running a full libp2p swarm.

pub use crate::core::mesh::{MeshConfig, MeshControl, MeshPeer, MeshStats, TopicMesh};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mesh_graft_below_d_low() {
        let mut mesh = TopicMesh::new("test".to_string(), MeshConfig::default());

        // Add 10 peers
        for i in 0..10 {
            mesh.add_peer(format!("peer-{}", i), 0.5 + (i as f32 * 0.05));
        }

        // Initial mesh is empty
        assert_eq!(mesh.mesh_size(), 0);

        // Heartbeat should graft peers up to D_low
        let _ = mesh.heartbeat();

        // Should have grafted D_low (4) peers
        assert!(mesh.mesh_size() >= mesh.config.d_low);
    }

    #[test]
    fn test_mesh_prune_above_d_high() {
        let mut mesh = TopicMesh::new("test".to_string(), MeshConfig::default());

        // Add 15 peers directly to mesh (exceeds D_high=12)
        for i in 0..15 {
            let id = format!("peer-{}", i);
            mesh.add_peer(id.clone(), 0.5);
            mesh.mesh_peers.insert(id);
        }

        assert_eq!(mesh.mesh_size(), 15);

        // Heartbeat should prune down to D_high
        let _ = mesh.heartbeat();

        assert!(mesh.mesh_size() <= mesh.config.d_high);
    }

    #[test]
    fn test_opportunistic_grafting() {
        let config = MeshConfig {
            d: 6,
            d_low: 4,
            d_high: 12,
            opportunistic_graft_threshold: 0.5,
            ..Default::default()
        };
        let mut mesh = TopicMesh::new("test".to_string(), config);

        // Add low-scoring peers to mesh
        for i in 0..6 {
            let id = format!("low-{}", i);
            mesh.add_peer(id.clone(), 0.2); // Below threshold
            mesh.mesh_peers.insert(id);
        }

        // Add high-scoring peers outside mesh
        for i in 0..4 {
            mesh.add_peer(format!("high-{}", i), 0.8);
        }

        // Median score is low
        assert!(mesh.mesh_median_score() < 0.5);

        // Heartbeat should opportunistically graft high-scoring peers
        let _ = mesh.heartbeat();

        let has_high = mesh.mesh_peers.iter().any(|id| id.starts_with("high"));
        assert!(has_high);
    }

    #[test]
    fn test_phase_alignment() {
        let mut mesh_a = TopicMesh::new("test".to_string(), MeshConfig::default());
        let mut mesh_b = TopicMesh::new("test".to_string(), MeshConfig::default());

        mesh_a.pulse_phase = 0.1;
        mesh_b.pulse_phase = 0.9;

        // Initial diff is 0.2 (0.9 - 1.1 or 0.1 - (-0.1))

        mesh_a.align_pulse(mesh_b.pulse_phase, 0.5);
        // Expected phase: 0.1 + (diff * 0.5)
        // Diff 0.9 -> 0.1 is -0.2 (0.1 - 0.9 + 1.0 or whatever)
        // Let's just check they got closer
        let diff_before = 0.2;
        let diff_after = {
            let d = (mesh_a.pulse_phase - mesh_b.pulse_phase).abs();
            if d > 0.5 {
                1.0 - d
            } else {
                d
            }
        };
        assert!(diff_after < diff_before);
    }

    #[test]
    fn test_spike_handling() {
        let mut mesh = TopicMesh::new("test".to_string(), MeshConfig::default());
        mesh.add_peer("danger-node".to_string(), 0.5);

        let initial_cond = mesh.known_peers.get("danger-node").unwrap().conductivity;
        let _initial_pressure = mesh.local_pressure;

        // Handle danger spike
        mesh.handle_spike("danger-node", 255);

        assert_eq!(
            mesh.local_pressure, 10.0,
            "Pressure should max out on danger spike"
        );
        let final_cond = mesh.known_peers.get("danger-node").unwrap().conductivity;
        assert!(
            final_cond >= initial_cond + 2.0,
            "Path should thicken significantly"
        );
    }
}
