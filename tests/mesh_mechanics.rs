use hypha::mesh::{MeshConfig, TopicMesh};
use hypha::{Capability, Task};

#[test]
fn test_task_diffusion_mechanics() {
    let task = Task {
        id: "task-1".to_string(),
        required_capability: Capability::Compute(100),
        priority: 1,
        reach_intensity: 1.0,
        source_id: "source".to_string(),
        auth_token: None,
    };

    // Case 1: Healthy neighbor, low pressure
    // Diffusion should be high
    let diffusion = task.diffuse(1.0, 0.9, 0.0);
    assert!(
        diffusion > 0.8,
        "Diffusion should be high for healthy neighbor"
    );

    // Case 2: Healthy neighbor, but HIGH pressure
    // Diffusion should be throttled
    let diffusion_pressured = task.diffuse(1.0, 0.9, 10.0);
    assert!(
        diffusion_pressured < 0.1,
        "Diffusion should be throttled by pressure"
    );

    // Case 3: Dead neighbor
    // Diffusion should be minimal
    let diffusion_dead = task.diffuse(1.0, 0.0, 0.0);
    assert!(
        diffusion_dead < 0.3,
        "Diffusion should be low for dead neighbor"
    );
}

#[test]
fn test_mesh_config_adaptive() {
    // Normal energy (1.0)
    let config_normal = MeshConfig::adaptive(1.0);
    assert_eq!(config_normal.d, 6);
    assert_eq!(config_normal.d_high, 12);

    // Low energy (0.4)
    let config_low = MeshConfig::adaptive(0.4);
    assert_eq!(config_low.d, 4);
    assert_eq!(config_low.d_high, 8);

    // Critical energy (0.1)
    let config_crit = MeshConfig::adaptive(0.1);
    assert_eq!(config_crit.d, 2);
    assert_eq!(config_crit.d_high, 4);
}

#[test]
fn test_topic_mesh_pressure_updates() {
    let mut mesh = TopicMesh::new("test".to_string(), MeshConfig::default());

    mesh.set_pressure(5.0);
    assert_eq!(mesh.local_pressure, 5.0);

    // Add peer
    mesh.add_peer("peer-1".to_string(), 1.0);

    // Update peer pressure
    mesh.update_peer_pressure("peer-1", 8.0);

    let peer = mesh.known_peers.get("peer-1").unwrap();
    assert_eq!(peer.pressure, 8.0);

    // Score should be impacted by high pressure
    // Pressure score = 1.0 - (8.0 / 10.0) = 0.2
    // Score = E*0.3 + A*0.2 + C*0.3 + P*0.2
    //       = 1.0*0.3 + 0*0.2 + 0.2*0.3 + 0.2*0.2
    //       = 0.3 + 0 + 0.06 + 0.04 = 0.4
    assert!(peer.score() < 0.5, "High pressure should lower peer score");
}
