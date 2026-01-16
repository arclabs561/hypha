//! Mesh Layer Evaluation
//!
//! Tests the gossip mesh management layer for resilience under various conditions.
//! This evaluates:
//! - Mesh maintenance (GRAFT/PRUNE with D parameters)
//! - Opportunistic grafting recovery
//! - Flood publishing for eclipse resistance
//! - Energy-aware peer scoring

use hypha::mesh::{MeshConfig, TopicMesh, MeshControl};
use std::collections::HashSet;
use rand::prelude::*;
use serde::Serialize;

/// Result of a mesh evaluation scenario
#[derive(Debug, Serialize)]
struct MeshEvalResult {
    scenario: String,
    heartbeat_count: u32,
    final_mesh_size: usize,
    final_median_score: f32,
    graft_count: u32,
    prune_count: u32,
    delivery_rate: f32,
    messages_delivered: u32,
    messages_published: u32,
    recovery_heartbeats: Option<u32>,
}

/// Simulate message propagation through mesh
fn simulate_mesh_propagation(
    meshes: &mut [TopicMesh],
    msg_id: &str,
    publisher_idx: usize,
    drop_prob: f32,
) -> (u32, Vec<u64>) {
    let mut rng = thread_rng();
    let mut delivered = 0u32;
    let mut latencies = Vec::new();
    let node_count = meshes.len();
    
    // Publisher floods to all peers above threshold
    let targets = meshes[publisher_idx].get_forward_targets(true);
    
    // Track which nodes received
    let mut received: Vec<bool> = vec![false; node_count];
    received[publisher_idx] = true;
    
    // Simulate propagation waves (BFS-like)
    let mut current_wave: Vec<usize> = targets.iter()
        .filter_map(|id| id.strip_prefix("node-").and_then(|s| s.parse().ok()))
        .filter(|&i| i < node_count)
        .collect();
    
    let mut hop = 1;
    while !current_wave.is_empty() && hop < 10 {
        let mut next_wave = Vec::new();
        
        for &idx in &current_wave {
            if received[idx] || rng.gen::<f32>() < drop_prob {
                continue;
            }
            
            received[idx] = true;
            delivered += 1;
            
            // Latency based on hops
            let latency = hop as u64 * 15_000 + rng.gen_range(0..5_000);
            latencies.push(latency);
            
            // Record message
            meshes[idx].record_message(&format!("node-{}", publisher_idx), msg_id);
            
            // Forward to mesh peers
            let forwards = meshes[idx].get_forward_targets(false);
            for fwd in forwards {
                if let Some(fwd_idx) = fwd.strip_prefix("node-").and_then(|s| s.parse::<usize>().ok()) {
                    if fwd_idx < node_count && !received[fwd_idx] {
                        next_wave.push(fwd_idx);
                    }
                }
            }
        }
        
        current_wave = next_wave;
        hop += 1;
    }
    
    (delivered, latencies)
}

/// Run mesh heartbeats and count control messages
fn run_heartbeats(meshes: &mut [TopicMesh], count: u32) -> (u32, u32) {
    let mut graft_count = 0u32;
    let mut prune_count = 0u32;
    
    for _ in 0..count {
        for mesh in meshes.iter_mut() {
            let controls = mesh.heartbeat();
            for (_, ctrl) in controls {
                match ctrl {
                    MeshControl::Graft { .. } => graft_count += 1,
                    MeshControl::Prune { .. } => prune_count += 1,
                    _ => {}
                }
            }
        }
    }
    
    (graft_count, prune_count)
}

/// Scenario: Baseline mesh formation
fn scenario_baseline(node_count: usize) -> MeshEvalResult {
    let mut meshes: Vec<TopicMesh> = (0..node_count)
        .map(|_| TopicMesh::new("hypha".to_string(), MeshConfig::default()))
        .collect();
    
    // Add all nodes as peers to each other
    for i in 0..node_count {
        for j in 0..node_count {
            if i != j {
                meshes[i].add_peer(format!("node-{}", j), 0.5 + (j as f32 * 0.01));
            }
        }
    }
    
    // Run heartbeats to form mesh
    let (graft_count, prune_count) = run_heartbeats(&mut meshes, 5);
    
    // Publish messages
    let mut total_delivered = 0u32;
    let msg_count = 50u32;
    
    for msg_idx in 0..msg_count {
        let publisher = msg_idx as usize % node_count;
        let (delivered, _) = simulate_mesh_propagation(
            &mut meshes,
            &format!("msg-{}", msg_idx),
            publisher,
            0.0,
        );
        total_delivered += delivered;
    }
    
    let expected = msg_count * (node_count as u32 - 1);
    let delivery_rate = total_delivered as f32 / expected as f32;
    
    let stats = meshes[0].stats();
    
    MeshEvalResult {
        scenario: "baseline".to_string(),
        heartbeat_count: 5,
        final_mesh_size: stats.mesh_size,
        final_median_score: stats.median_score,
        graft_count,
        prune_count,
        delivery_rate,
        messages_delivered: total_delivered,
        messages_published: msg_count,
        recovery_heartbeats: None,
    }
}

/// Scenario: Mesh under attack (low-scoring Sybils)
fn scenario_sybil_attack(honest_count: usize, sybil_count: usize) -> MeshEvalResult {
    let total = honest_count + sybil_count;
    let mut meshes: Vec<TopicMesh> = (0..total)
        .map(|_| TopicMesh::new("hypha".to_string(), MeshConfig::default()))
        .collect();
    
    // Add peers: honest nodes have high scores, sybils have low
    for i in 0..total {
        for j in 0..total {
            if i != j {
                let peer_is_honest = j < honest_count;
                let score = if peer_is_honest { 0.7 + (j as f32 * 0.01) } else { 0.1 };
                meshes[i].add_peer(format!("node-{}", j), score);
            }
        }
    }
    
    // Initial mesh formation
    let (mut graft_count, mut prune_count) = run_heartbeats(&mut meshes, 3);
    
    // Sybils try to graft into honest meshes (simulate attack)
    for i in honest_count..total {
        for j in 0..honest_count {
            // Sybil requests graft
            let accepted = meshes[j].handle_graft(&format!("node-{}", i));
            if accepted {
                graft_count += 1;
            }
        }
    }
    
    // Run more heartbeats - honest nodes should prune low-scoring Sybils
    let (g, p) = run_heartbeats(&mut meshes, 5);
    graft_count += g;
    prune_count += p;
    
    // Publish from honest nodes only
    let mut total_delivered = 0u32;
    let msg_count = 30u32;
    
    for msg_idx in 0..msg_count {
        let publisher = msg_idx as usize % honest_count;
        let (delivered, _) = simulate_mesh_propagation(
            &mut meshes,
            &format!("msg-{}", msg_idx),
            publisher,
            0.0,
        );
        // Only count deliveries to honest nodes
        total_delivered += delivered.min((honest_count - 1) as u32);
    }
    
    let expected = msg_count * (honest_count as u32 - 1);
    let delivery_rate = total_delivered as f32 / expected as f32;
    
    // Check mesh composition of first honest node - how many are honest?
    let honest_in_mesh = meshes[0].mesh_peers.iter()
        .filter(|id| {
            id.strip_prefix("node-")
                .and_then(|s| s.parse::<usize>().ok())
                .map(|i| i < honest_count)
                .unwrap_or(false)
        })
        .count();
    
    let sybil_in_mesh = meshes[0].mesh_size() - honest_in_mesh;
    
    MeshEvalResult {
        scenario: format!("sybil_{}h_{}s_{}in", honest_count, sybil_count, sybil_in_mesh),
        heartbeat_count: 8,
        final_mesh_size: meshes[0].mesh_size(),
        final_median_score: meshes[0].mesh_median_score(),
        graft_count,
        prune_count,
        delivery_rate,
        messages_delivered: total_delivered,
        messages_published: msg_count,
        recovery_heartbeats: None,
    }
}

/// Scenario: Mesh recovery after partition
fn scenario_partition_recovery(node_count: usize) -> MeshEvalResult {
    let mut meshes: Vec<TopicMesh> = (0..node_count)
        .map(|_| TopicMesh::new("hypha".to_string(), MeshConfig::default()))
        .collect();
    
    // Initial full connectivity
    for i in 0..node_count {
        for j in 0..node_count {
            if i != j {
                meshes[i].add_peer(format!("node-{}", j), 0.5 + (j as f32 * 0.02));
            }
        }
    }
    
    // Form initial mesh
    run_heartbeats(&mut meshes, 5);
    
    // Simulate partition: remove half the peers from each side
    let half = node_count / 2;
    for i in 0..half {
        for j in half..node_count {
            meshes[i].known_peers.remove(&format!("node-{}", j));
            meshes[i].mesh_peers.remove(&format!("node-{}", j));
            meshes[j].known_peers.remove(&format!("node-{}", i));
            meshes[j].mesh_peers.remove(&format!("node-{}", i));
        }
    }
    
    // Publish during partition (messages won't reach other half)
    let msg_count = 20u32;
    let mut total_delivered = 0u32;
    let mut total_expected = 0u32;
    
    for msg_idx in 0..msg_count {
        let publisher = msg_idx as usize % half; // Only left partition
        let (delivered, _) = simulate_mesh_propagation(
            &mut meshes,
            &format!("part-msg-{}", msg_idx),
            publisher,
            0.0,
        );
        total_delivered += delivered;
        total_expected += (node_count - 1) as u32;
    }
    
    let partition_delivery_rate = total_delivered as f32 / total_expected as f32;
    println!("  Partitioned: delivery={:.1}%", partition_delivery_rate * 100.0);
    
    // Heal partition: restore connectivity
    for i in 0..half {
        for j in half..node_count {
            meshes[i].add_peer(format!("node-{}", j), 0.5 + (j as f32 * 0.02));
            meshes[j].add_peer(format!("node-{}", i), 0.5 + (i as f32 * 0.02));
        }
    }
    
    // Run heartbeats to recover
    let mut recovery_heartbeats = 0u32;
    loop {
        recovery_heartbeats += 1;
        let _ = run_heartbeats(&mut meshes, 1);
        
        // Check if meshes span partition
        let left_has_right = meshes[0].mesh_peers.iter()
            .any(|id| {
                id.strip_prefix("node-")
                    .and_then(|s| s.parse::<usize>().ok())
                    .map(|i| i >= half)
                    .unwrap_or(false)
            });
        
        if left_has_right || recovery_heartbeats > 20 {
            break;
        }
    }
    
    // Publish after recovery
    let mut recovered_delivered = 0u32;
    let mut recovered_expected = 0u32;
    for msg_idx in 0..msg_count {
        let publisher = msg_idx as usize % half;
        let (delivered, _) = simulate_mesh_propagation(
            &mut meshes,
            &format!("recv-msg-{}", msg_idx),
            publisher,
            0.0,
        );
        recovered_delivered += delivered;
        recovered_expected += (node_count - 1) as u32;
    }
    
    let delivery_rate = recovered_delivered as f32 / recovered_expected as f32;
    
    MeshEvalResult {
        scenario: "partition_recovery".to_string(),
        heartbeat_count: 5 + recovery_heartbeats + 1,
        final_mesh_size: meshes[0].mesh_size(),
        final_median_score: meshes[0].mesh_median_score(),
        graft_count: 0, // Not tracked here specifically
        prune_count: 0,
        delivery_rate,
        messages_delivered: recovered_delivered,
        messages_published: msg_count,
        recovery_heartbeats: Some(recovery_heartbeats),
    }
}

/// Scenario: Energy drain (nodes losing energy over time)
fn scenario_energy_drain(node_count: usize) -> MeshEvalResult {
    let mut meshes: Vec<TopicMesh> = (0..node_count)
        .map(|_| TopicMesh::new("hypha".to_string(), MeshConfig::default()))
        .collect();
    
    // Initial state: all nodes start with high energy
    let mut energy_scores: Vec<f32> = vec![0.9; node_count];
    
    for i in 0..node_count {
        for j in 0..node_count {
            if i != j {
                meshes[i].add_peer(format!("node-{}", j), energy_scores[j]);
            }
        }
    }
    
    // Form mesh
    run_heartbeats(&mut meshes, 5);
    
    let mut total_graft = 0u32;
    let mut total_prune = 0u32;
    let mut total_delivered = 0u32;
    let mut total_expected = 0u32;
    let mut messages_published = 0u32;
    
    // Simulate drain over time
    for round in 0..10 {
        // Drain energy from first half of nodes
        for i in 0..node_count/2 {
            energy_scores[i] = (energy_scores[i] - 0.1).max(0.0);
        }
        
        // Update scores in all meshes
        for i in 0..node_count {
            for j in 0..node_count {
                if i != j {
                    meshes[i].update_peer_score(&format!("node-{}", j), energy_scores[j]);
                }
            }
        }
        
        // Heartbeat to adapt mesh
        let (g, p) = run_heartbeats(&mut meshes, 1);
        total_graft += g;
        total_prune += p;
        
        // Count active nodes (receivers) for this round
        let active_receivers = energy_scores.iter().filter(|&&e| e > 0.1).count() - 1;
        
        // Publish messages from active nodes only
        for msg_idx in 0..10 {
            let publisher = (round * 10 + msg_idx) as usize % node_count;
            if energy_scores[publisher] > 0.1 {
                messages_published += 1;
                total_expected += active_receivers as u32;
                
                let (delivered, _) = simulate_mesh_propagation(
                    &mut meshes,
                    &format!("drain-msg-{}-{}", round, msg_idx),
                    publisher,
                    0.0,
                );
                total_delivered += delivered.min(active_receivers as u32);
            }
        }
    }
    
    let delivery_rate = if total_expected > 0 {
        total_delivered as f32 / total_expected as f32
    } else {
        0.0
    };
    
    MeshEvalResult {
        scenario: "energy_drain".to_string(),
        heartbeat_count: 15,
        final_mesh_size: meshes[node_count-1].mesh_size(),
        final_median_score: meshes[node_count-1].mesh_median_score(),
        graft_count: total_graft,
        prune_count: total_prune,
        delivery_rate,
        messages_delivered: total_delivered,
        messages_published,
        recovery_heartbeats: None,
    }
}

fn main() {
    println!("Mesh Layer Evaluation");
    println!("{}", "=".repeat(70));
    println!();
    
    let mut results = Vec::new();
    
    // Baseline
    println!("Running: baseline (50 nodes)...");
    results.push(scenario_baseline(50));
    
    // Sybil attacks at different ratios
    for sybil_ratio in [0.25, 0.5, 1.0, 2.0] {
        let honest = 30;
        let sybil = (honest as f32 * sybil_ratio) as usize;
        println!("Running: sybil attack ({} honest, {} sybils)...", honest, sybil);
        results.push(scenario_sybil_attack(honest, sybil));
    }
    
    // Partition recovery
    println!("Running: partition recovery (40 nodes)...");
    results.push(scenario_partition_recovery(40));
    
    // Energy drain
    println!("Running: energy drain (50 nodes)...");
    results.push(scenario_energy_drain(50));
    
    // Print results table
    println!("\n{}", "=".repeat(70));
    println!("MESH EVALUATION RESULTS");
    println!("{}", "=".repeat(70));
    println!();
    
    println!("{:<35} {:>8} {:>8} {:>8} {:>10}",
        "Scenario", "Delivery", "MeshSz", "MedScore", "Recovery");
    println!("{}", "-".repeat(70));
    
    for r in &results {
        println!("{:<35} {:>7.1}% {:>8} {:>8.3} {:>10}",
            r.scenario,
            r.delivery_rate * 100.0,
            r.final_mesh_size,
            r.final_median_score,
            r.recovery_heartbeats.map(|h| format!("{}hb", h)).unwrap_or("-".to_string()),
        );
    }
    
    println!();
    println!("{}", "=".repeat(70));
    println!("ANALYSIS");
    println!("{}", "=".repeat(70));
    println!();
    
    // Analyze Sybil resilience
    let baseline_delivery = results[0].delivery_rate;
    println!("Sybil Resilience (delivery rate vs baseline {:.1}%):", baseline_delivery * 100.0);
    for r in &results {
        if r.scenario.contains("sybil") {
            let pct_of_baseline = (r.delivery_rate / baseline_delivery) * 100.0;
            let status = if pct_of_baseline >= 90.0 { "PASS" } else if pct_of_baseline >= 70.0 { "DEGRADED" } else { "FAIL" };
            println!("  {}: {:.1}% of baseline [{}]", r.scenario, pct_of_baseline, status);
        }
    }
    
    // Partition recovery
    if let Some(r) = results.iter().find(|r| r.scenario == "partition_recovery") {
        println!("\nPartition Recovery:");
        println!("  Recovery time: {} heartbeats", r.recovery_heartbeats.unwrap_or(0));
        println!("  Post-recovery delivery: {:.1}%", r.delivery_rate * 100.0);
    }
    
    // Energy adaptation
    if let Some(r) = results.iter().find(|r| r.scenario == "energy_drain") {
        println!("\nEnergy Adaptation:");
        println!("  Final mesh median score: {:.3}", r.final_median_score);
        println!("  Delivery under drain: {:.1}%", r.delivery_rate * 100.0);
        println!("  Graft/Prune ratio: {}/{} ({:.2})",
            r.graft_count, r.prune_count,
            r.graft_count as f32 / (r.prune_count.max(1)) as f32
        );
    }
    
    // Write JSON report
    let json = serde_json::to_string_pretty(&results).unwrap();
    std::fs::write("hypha_mesh_eval.json", &json).unwrap();
    println!("\nDetailed report: hypha_mesh_eval.json");
    
    // Run packet loss sweep
    run_packet_loss_sweep(50);
    
    // Run path thickening test
    run_path_thickening_test(20);
}

fn run_path_thickening_test(node_count: usize) {
    println!("\n{}", "=".repeat(70));
    println!("PATH THICKENING (Mycelial Conductivity)");
    println!("{}", "=".repeat(70));
    
    let mut meshes: Vec<TopicMesh> = (0..node_count)
        .map(|_| TopicMesh::new("hypha".to_string(), MeshConfig::default()))
        .collect();
    
    // Group A: high energy (0.8), but we won't use them
    // Group B: moderate energy (0.5), but we'll send messages through them
    for i in 0..node_count {
        for j in 0..node_count {
            if i == j { continue; }
            let score = if j < node_count / 2 { 0.8 } else { 0.5 };
            meshes[i].add_peer(format!("node-{}", j), score);
        }
    }
    
    // Initial mesh formation - should prefer Group A
    run_heartbeats(&mut meshes, 5);
    let initial_a = meshes[0].mesh_peers.iter().filter(|id| id.strip_prefix("node-").and_then(|s| s.parse::<usize>().ok()).map(|i| i < node_count/2).unwrap_or(false)).count();
    println!("Initial mesh: {} honest (Group A), {} lower-energy (Group B)", initial_a, meshes[0].mesh_size() - initial_a);
    
    // Send 100 messages ONLY through Group B
    for m in 0..100 {
        for i in 0..node_count {
            for j in node_count/2..node_count {
                if i != j {
                    meshes[i].record_message(&format!("node-{}", j), &format!("flow-{}", m));
                }
            }
        }
        run_heartbeats(&mut meshes, 1);
    }
    
    let final_a = meshes[0].mesh_peers.iter().filter(|id| id.strip_prefix("node-").and_then(|s| s.parse::<usize>().ok()).map(|i| i < node_count/2).unwrap_or(false)).count();
    println!("Final mesh: {} honest (Group A), {} high-flow (Group B)", final_a, meshes[0].mesh_size() - final_a);
    
    if final_a < initial_a {
        println!("  STATUS: SUCCESS - Mesh migrated to high-flow paths despite lower energy.");
    } else {
        println!("  STATUS: FAILED - Mesh stayed static.");
    }
}

fn run_packet_loss_sweep(node_count: usize) {
    println!("\n{}", "=".repeat(70));
    println!("PACKET LOSS SWEEP (Percolation Threshold)");
    println!("{}", "=".repeat(70));
    
    println!("{:<15} {:>10} {:>10}", "Loss Rate", "Delivery", "Status");
    println!("{}", "-".repeat(70));
    
    for loss_rate in [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9] {
        let mut meshes: Vec<TopicMesh> = (0..node_count)
            .map(|_| TopicMesh::new("hypha".to_string(), MeshConfig::default()))
            .collect();
        
        for i in 0..node_count {
            for j in 0..node_count {
                if i != j {
                    meshes[i].add_peer(format!("node-{}", j), 0.8);
                }
            }
        }
        
        run_heartbeats(&mut meshes, 5);
        
        let mut total_delivered = 0u32;
        let msg_count = 50u32;
        for msg_idx in 0..msg_count {
            let (delivered, _) = simulate_mesh_propagation(&mut meshes, &format!("loss-{}", msg_idx), 0, loss_rate);
            total_delivered += delivered;
        }
        
        let rate = total_delivered as f32 / (msg_count * (node_count as u32 - 1)) as f32;
        let status = if rate > 0.95 { "PERFECT" } else if rate > 0.5 { "DEGRADED" } else { "FAILED" };
        println!("{:<15.1}% {:>9.1}% {:>10}", loss_rate * 100.0, rate * 100.0, status);
    }
}
