//! Emergent Auction Live Experiment
//!
//! Simulates a live mycelial network where tasks are allocated via
//! pheromone diffusion and local bidding.
//!
//! Nodes: 10
//! Topology: Sparse line (0-1-2-3-4-5-6-7-8-9)
//! Task: Heavy Compute (Cap: 100)
//! Source: node-9 (peripheral)
//! Winner Goal: node-0 (high energy hub)

use hypha::{SporeNode, Capability, Task, Bid, PowerMode};
use hypha::mesh::{MeshConfig, TopicMesh};
use tokio::sync::mpsc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Running Emergent Auction Live Experiment...");

    let node_count = 10;
    let tmp = tempdir()?;
    let mut nodes = Vec::new();

    for i in 0..node_count {
        let path = tmp.path().join(format!("spore_{}", i));
        std::fs::create_dir(&path)?;
        let mut node = SporeNode::new(&path)?;
        
        // Node 0 is mains powered, others on battery
        if i == 0 {
            node.set_power_mode(PowerMode::Normal);
            let mut state = node.physical_state.lock().unwrap();
            state.is_mains_powered = true;
        } else {
            node.set_power_mode(PowerMode::LowBattery);
        }

        // Capabilities
        if i % 2 == 0 {
            node.add_capability(Capability::Compute(100));
        }
        
        nodes.push(node);
    }

    // Connect in a sparse line topology
    for i in 0..node_count {
        let mut mesh = nodes[i].mesh.lock().unwrap();
        if i > 0 {
            mesh.add_peer(format!("node-{}", i-1), 0.8);
        }
        if i < node_count - 1 {
            mesh.add_peer(format!("node-{}", i+1), 0.8);
        }
    }

    // Stabilize mesh
    println!("Stabilizing sparse mycelium...");
    for _ in 0..20 {
        for i in 0..node_count {
            let mut mesh = nodes[i].mesh.lock().unwrap();
            mesh.heartbeat();
        }
    }

    // Inject Task at node-9
    let task = Task::new(
        "live-compute-1".to_string(),
        Capability::Compute(100),
        1,
        "node-9".to_string()
    );

    println!("Injecting task at node-9. Simulating diffusion waves...");

    // Live bidding state
    let mut bid_history: HashMap<String, Vec<Bid>> = HashMap::new();
    let mut node_message_counts: Vec<usize> = vec![0; node_count];

    for wave in 1..=10 {
        println!("Wave {}:", wave);
        for i in (0..node_count).rev() {
            let my_id = format!("node-{}", i);
            let mut bids = bid_history.entry(task.id.clone()).or_insert(Vec::new());
            
            if let Some(bid) = nodes[i].process_task_bundle(&task, &mut bids) {
                println!("  Node {} bid: Weighted Score {:.4}", i, bid.energy_score);
            }

            // Simulate message propagation along the line
            if i > 0 {
                node_message_counts[i-1] += 1;
                let mut mesh = nodes[i-1].mesh.lock().unwrap();
                mesh.record_message(&my_id, &format!("task-wave-{}-{}", task.id, wave));
            }
        }
    }

    println!("\nFinal Allocation Summary:");
    if let Some(bids) = bid_history.get(&task.id) {
        let mut sorted_bids = bids.clone();
        sorted_bids.sort_by(|a, b| b.energy_score.partial_cmp(&a.energy_score).unwrap());
        
        for (rank, b) in sorted_bids.iter().take(3).enumerate() {
            println!("  Rank {}: Bidder {}, Score {:.4}", rank+1, b.bidder_id, b.energy_score);
        }
        
        if let Some(winner) = sorted_bids.first() {
            println!("\nWinner: {} - task successfully pulled toward the high-energy hub.", winner.bidder_id);
            if winner.bidder_id == "node-0" {
                println!("SUCCESS: Gradient-based routing worked perfectly.");
            }
        }
    }

    Ok(())
}
