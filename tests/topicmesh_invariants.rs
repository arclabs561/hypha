use hypha::mesh::{MeshConfig, TopicMesh};

#[test]
fn test_update_peer_score_inserts_peer() {
    let mut mesh = TopicMesh::new("t".to_string(), MeshConfig::default());
    assert_eq!(mesh.known_peers.len(), 0);
    mesh.update_peer_score("peer-a", 0.7);
    assert!(mesh.known_peers.contains_key("peer-a"));
    assert!((mesh.known_peers["peer-a"].energy_score - 0.7).abs() < 1e-6);
}

#[test]
fn test_conductivity_thickens_then_decays() {
    let mut mesh = TopicMesh::new("t".to_string(), MeshConfig::default());
    mesh.add_peer("peer-a".to_string(), 0.8);
    mesh.set_pressure(10.0);
    mesh.update_peer_pressure("peer-a", 0.0);

    let c0 = mesh.known_peers["peer-a"].conductivity;
    mesh.record_message("peer-a", "m1");
    let c1 = mesh.known_peers["peer-a"].conductivity;
    assert!(c1 > c0, "conductivity should increase on flow");

    // One heartbeat decays conductivity toward baseline.
    let _ = mesh.heartbeat();
    let c2 = mesh.known_peers["peer-a"].conductivity;
    assert!(c2 < c1, "conductivity should decay each heartbeat");
    assert!(c2 >= 0.5, "conductivity has a floor");
}

#[test]
fn test_prune_excess_prefers_removing_low_score() {
    let mut mesh = TopicMesh::new("t".to_string(), MeshConfig::default());

    // Add peers: one clearly low-score by pressure and low energy.
    mesh.add_peer("good".to_string(), 0.9);
    mesh.add_peer("bad".to_string(), 0.1);
    mesh.update_peer_pressure("bad", 10.0);

    // Force both into mesh plus extras so we exceed D_high quickly.
    mesh.mesh_peers.insert("good".to_string());
    mesh.mesh_peers.insert("bad".to_string());
    for i in 0..20 {
        let id = format!("peer-{}", i);
        mesh.add_peer(id.clone(), 0.6);
        mesh.mesh_peers.insert(id);
    }

    assert!(mesh.mesh_peers.len() > mesh.config.d_high);
    let _ = mesh.heartbeat();

    // After pruning down, "bad" should be a strong candidate for removal.
    // We don't require it always (ties exist), but in this setup it should be.
    assert!(
        !mesh.mesh_peers.contains("bad"),
        "expected very low-score peer to be pruned"
    );
}

#[test]
fn test_duplicate_count_increments_on_replay() {
    let mut mesh = TopicMesh::new("t".to_string(), MeshConfig::default());
    mesh.add_peer("peer-a".to_string(), 0.8);

    assert_eq!(mesh.duplicate_count, 0);
    mesh.record_message("peer-a", "m1");
    assert_eq!(mesh.duplicate_count, 0);
    mesh.record_message("peer-a", "m1");
    assert_eq!(mesh.duplicate_count, 1);
    mesh.record_message("peer-a", "m1");
    assert_eq!(mesh.duplicate_count, 2);
}

#[test]
fn test_backoff_blocks_graft_immediately() {
    let mut mesh = TopicMesh::new("t".to_string(), MeshConfig::default());
    mesh.add_peer("peer-a".to_string(), 0.9);
    mesh.mesh_peers.insert("peer-a".to_string());

    // PRUNE introduces backoff and removes from mesh.
    mesh.handle_prune("peer-a", std::time::Duration::from_secs(60));
    assert!(!mesh.mesh_peers.contains("peer-a"));
    assert!(mesh.backoff.contains_key("peer-a"));

    // A new graft request during backoff must be rejected.
    assert!(!mesh.handle_graft("peer-a"));
    assert!(!mesh.mesh_peers.contains("peer-a"));
}

#[test]
fn test_heartbeat_never_panics_on_nan_energy_scores() {
    // Real systems will occasionally ingest garbage floats. The mesh layer should
    // degrade gracefully (treat NaNs as "bad") rather than panic.
    let mut mesh = TopicMesh::new("t".to_string(), MeshConfig::default());

    mesh.add_peer("ok-0".to_string(), 0.6);
    mesh.add_peer("ok-1".to_string(), 0.7);
    mesh.add_peer("nan".to_string(), f32::NAN);

    // Force a mesh state that triggers pruning + selection.
    for id in ["ok-0", "ok-1", "nan"] {
        mesh.mesh_peers.insert(id.to_string());
    }
    // Add enough peers to exceed d_high.
    for i in 0..25 {
        let id = format!("peer-{i}");
        mesh.add_peer(id.clone(), 0.55);
        mesh.mesh_peers.insert(id);
    }

    let _ = mesh.heartbeat();
    assert!(mesh.mesh_peers.len() <= mesh.config.d_high);
}
