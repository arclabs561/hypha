//! Slime Mold Task Allocation Experiment
//!
//! Models tasks as "nutrients" that emit pheromones through the mycelial mesh.
//! Nodes bid on tasks based on local pheromone intensity, which is shaped by
//! path conductivity, energy scores, and pressure gradients.

use hypha::mesh::{MeshConfig, TopicMesh};
use hypha::{Bid, Capability, Task};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Running Slime Mold Auction Experiment...");

    let node_count = 30;
    let mut meshes: Vec<TopicMesh> = (0..node_count)
        .map(|_| TopicMesh::new("tasks".to_string(), MeshConfig::default()))
        .collect();

    let capabilities = [
        Capability::Compute(100),
        Capability::Storage(1000),
        Capability::Sensing("thermal".to_string()),
    ];

    for i in 0..node_count {
        let _cap = capabilities[i % capabilities.len()].clone();
        // Some nodes are "strong" (mains power), some "weak" (low battery)
        let energy = if i < 5 { 1.0 } else { 0.4 + (i as f32 * 0.01) };

        // Setup initial mesh connectivity
        for j in 0..node_count {
            if i != j {
                meshes[i].add_peer(format!("node-{}", j), energy);
            }
        }
    }

    // Run heartbeats to stabilize mesh
    for _ in 0..10 {
        for mesh in meshes.iter_mut() {
            mesh.heartbeat();
        }
    }

    // Scenario: A "Compute" task is injected at Node 29 (far from stable Node 0-4)
    let task = Task::new(
        "heavy-compute".to_string(),
        Capability::Compute(100),
        1,
        "node-29".to_string(),
    );

    println!("Injecting task at node-29. Pheromone diffusing through mesh...");

    // Simulate pheromone accumulation
    let mut node_pheromones: Vec<f32> = vec![0.0; node_count];
    node_pheromones[29] = 1.0;

    for wave in 1..=5 {
        let mut new_pheromones = vec![0.0; node_count];
        for i in 0..node_count {
            if node_pheromones[i] > 0.01 {
                let my_id = format!("node-{}", i);
                let mesh_peers = meshes[i].mesh_peers.clone();
                for peer_id in mesh_peers {
                    if let Some(peer_idx) = peer_id
                        .strip_prefix("node-")
                        .and_then(|s| s.parse::<usize>().ok())
                    {
                        let conductivity = meshes[i]
                            .known_peers
                            .get(&peer_id)
                            .map(|p| p.conductivity)
                            .unwrap_or(1.0);
                        let energy = meshes[peer_idx]
                            .known_peers
                            .get(&my_id)
                            .map(|p| p.energy_score)
                            .unwrap_or(0.5);
                        let pressure = meshes[peer_idx].local_pressure;

                        let diffused =
                            task.diffuse(conductivity, energy, pressure) * node_pheromones[i];
                        new_pheromones[peer_idx] = (new_pheromones[peer_idx] + diffused).min(1.0);
                    }
                }
            }
        }
        node_pheromones = new_pheromones;
        let reached = node_pheromones.iter().filter(|&&p| p > 0.05).count();
        println!("Wave {}: Task pheromone reached {} nodes.", wave, reached);
    }

    // Final Bidding
    println!("\nBidding Results (Top 5):");
    let mut bids = Vec::new();
    for (idx, &intensity) in node_pheromones.iter().enumerate() {
        if intensity > 0.05 {
            // Node checks if it has capability
            if idx % capabilities.len() == 0 {
                // This node has Compute
                let score = meshes[idx]
                    .known_peers
                    .values()
                    .next()
                    .map(|p| p.energy_score)
                    .unwrap_or(0.5);
                let bid = Bid {
                    task_id: task.id.clone(),
                    bidder_id: format!("node-{}", idx),
                    energy_score: score * intensity, // Bio-weighted bid
                    cost_mah: 50.0,
                };
                bids.push(bid);
            }
        }
    }

    bids.sort_by(|a, b| b.energy_score.partial_cmp(&a.energy_score).unwrap());
    for b in bids.iter().take(5) {
        println!(
            "  Bidder: {}, Weighted Score: {:.4}",
            b.bidder_id, b.energy_score
        );
    }

    if let Some(winner) = bids.first() {
        println!(
            "\nWinner: {} - task successfully allocated via mycelial gradient.",
            winner.bidder_id
        );
    } else {
        println!("\nFAILED: No suitable nodes reached by pheromone.");
    }

    Ok(())
}
