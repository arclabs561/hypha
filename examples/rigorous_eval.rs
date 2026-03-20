//! Rigorous Evaluation Suite for Hypha
//!
//! Based on Protocol Labs' Gossipsub v1.1 Evaluation methodology:
//! - Delivery rate and latency percentiles
//! - Energy consumption per delivery
//! - Fault injection (degradation, partition)
//! - Convergence metrics

use hypha::eval::{EvalRun, EvalScenario, FaultType, MetricsCollector};
use hypha::{Capability, SporeNode};
use rand::{rng, Rng};
use serde_json::json;
use std::fs::File;
use std::io::Write;
use std::time::Duration;
use tempfile::tempdir;

/// Simulates message propagation through the network using peer-to-peer relaying.
/// Returns (delivery_count, latencies_us)
fn simulate_propagation(
    nodes: &[SporeNode],
    message_id: &str,
    payload: &[u8],
    drop_probability: f32,
    publisher_count: usize,
) -> (u64, Vec<u64>) {
    let mut rng = rng();
    let mut delivered_nodes = std::collections::HashSet::new();
    let mut latencies = Vec::new();

    // Start from publisher_count publishers
    let mut current_wave: Vec<(usize, u64)> = (0..publisher_count)
        .filter(|&i| i < nodes.len())
        .map(|i| (i, 0u64))
        .collect();

    for (_i, _) in &current_wave {
        delivered_nodes.insert(*_i);
    }

    // Potential neighbors: randomize to avoid artificial isolation
    let mut neighbor_indices: Vec<usize> = (0..nodes.len()).collect();
    use rand::seq::SliceRandom;

    // Propagation waves (max 12 hops)
    for _hop in 0..12 {
        let mut next_wave = Vec::new();
        for (node_idx, current_latency) in current_wave {
            // Pick D=8 neighbors for higher reach in stress (D=6 is standard)
            neighbor_indices.shuffle(&mut rng);
            let sample_size = 8.min(nodes.len());

            for &neighbor_idx in &neighbor_indices[..sample_size] {
                if neighbor_idx == node_idx || delivered_nodes.contains(&neighbor_idx) {
                    continue;
                }

                let neighbor = &nodes[neighbor_idx];

                // Skip exhausted nodes
                if neighbor.is_exhausted() {
                    continue;
                }

                // Apply drop probability (network impairment)
                if rng.random::<f32>() < drop_probability {
                    continue;
                }

                // Success!
                if neighbor.simulate_receive(message_id, payload).is_ok() {
                    delivered_nodes.insert(neighbor_idx);
                    let hop_latency = 15_000 + rng.random_range(0..5_000);
                    let total_latency = current_latency + hop_latency;
                    latencies.push(total_latency);

                    neighbor.consume_energy(0.1);

                    // Relay based on Pulse-Gated strategy (simulated phase > 0.7)
                    let energy = neighbor.energy_score();
                    let should_relay = if energy > 0.9 {
                        true
                    } else {
                        energy > 0.6 && rng.random::<f32>() > 0.3 // ~70% pulse peak probability
                    };

                    if should_relay {
                        next_wave.push((neighbor_idx, total_latency));
                    }
                }
            }
        }
        current_wave = next_wave;
        if current_wave.is_empty() {
            break;
        }
    }

    (delivered_nodes.len() as u64, latencies)
}

/// Run a single evaluation scenario
fn run_scenario(scenario: &EvalScenario) -> Result<EvalRun, Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let mut collector = MetricsCollector::new();
    let mut nodes = Vec::new();
    let mut rng = rng();

    // Create nodes
    let low_energy_count =
        (scenario.node_count as f32 * scenario.low_energy_percentage / 100.0) as usize;

    for i in 0..scenario.node_count {
        let path = tmp.path().join(format!("node_{}", i));
        std::fs::create_dir(&path)?;
        let mut node = SporeNode::new(&path)?;

        // Configure low-energy nodes
        if i < low_energy_count {
            let mut meta = node.metabolism.lock().unwrap();
            if let Some(batt) = meta.as_any().downcast_mut::<hypha::BatteryMetabolism>() {
                batt.voltage = 3.3 + rng.random::<f32>() * 0.1; // 3.3-3.4V
                batt.mah_remaining = rng.random_range(5.0..50.0); // 5-50 mAh
            }
        } else {
            node.add_capability(Capability::Compute(100));
        }

        nodes.push(node);
    }

    // Track initial energy
    let initial_energy: f32 = nodes.iter().map(|n| n.mah_remaining()).sum();

    // Process fault schedule
    let mut current_drop_prob = 0.0f32;
    let mut partitioned = false;

    for fault_event in &scenario.fault_schedule {
        match &fault_event.fault {
            FaultType::Degradation { drop_probability } => {
                current_drop_prob = *drop_probability;
                collector.record_fault(fault_event.fault.clone());
            }
            FaultType::Partition { .. } => {
                partitioned = true;
                collector.record_fault(fault_event.fault.clone());
            }
            FaultType::PartitionHeal => {
                partitioned = false;
                collector.record_fault(fault_event.fault.clone());
            }
            FaultType::SyncSpike { intensity } => {
                // Simulate a node triggering a spike
                if let Some(n) = nodes.first() {
                    let _ = n.trigger_sync_spike(*intensity);
                }
                collector.record_fault(fault_event.fault.clone());
                // Spike effect: temporarily reduce drop probability for the next few messages
                current_drop_prob = (current_drop_prob - 0.4).max(0.0);
            }
            _ => {}
        }
    }

    // Simulate message publishing
    let message_count = (scenario.duration.as_secs_f32() * scenario.message_rate_per_sec) as usize;
    let payload = vec![0u8; scenario.message_size_bytes];

    for msg_idx in 0..message_count {
        let msg_id = format!("{}-{}", scenario.name, msg_idx);
        collector.record_publish(nodes.len());

        // Simulate propagation
        let effective_drop = if partitioned {
            0.5_f32.max(current_drop_prob) // Partition causes ~50% drop
        } else {
            current_drop_prob
        };

        let (_delivered, latencies) = simulate_propagation(
            &nodes,
            &msg_id,
            &payload,
            effective_drop,
            scenario.publisher_count,
        );

        for lat_us in latencies {
            collector.record_delivery(Duration::from_micros(lat_us));
        }

        // Publishers consume extra energy
        let publisher_idx = msg_idx % scenario.publisher_count;
        if publisher_idx < nodes.len() {
            nodes[publisher_idx].consume_energy(0.5); // 0.5 mAh per publish
        }
    }

    // Record final energy state
    let energy_scores: Vec<f32> = nodes.iter().map(|n| n.energy_score()).collect();
    collector.record_energy_snapshot(energy_scores);

    // Check consistency (message counts across nodes)
    let message_counts: Vec<usize> = nodes.iter().map(|n| n.message_count()).collect();
    let max_count = message_counts.iter().max().copied().unwrap_or(0);
    let divergence: usize = message_counts.iter().map(|&c| max_count - c).sum();
    collector.record_consistency(divergence);

    // Calculate total energy consumed
    let final_energy: f32 = nodes.iter().map(|n| n.mah_remaining()).sum();
    let mah_consumed = initial_energy - final_energy;

    Ok(collector.finalize(scenario, mah_consumed))
}

/// Run evaluation sweep and generate report
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Hypha Rigorous Evaluation Suite");
    println!("================================\n");

    let mut all_runs: Vec<EvalRun> = Vec::new();

    // 1. Baseline (no faults)
    println!("Running: Baseline scenarios...");
    let baseline = EvalScenario {
        name: "baseline".to_string(),
        node_count: 30,
        publisher_count: 3,
        message_rate_per_sec: 5.0,
        duration: Duration::from_secs(2),
        ..Default::default()
    };
    let run = run_scenario(&baseline)?;
    println!(
        "  Delivery rate: {:.2}%",
        run.delivery.delivery_rate() * 100.0
    );
    println!("  p99 latency: {:?}", run.delivery.p99());
    all_runs.push(run);

    // 2. Percolation threshold sweep
    println!("\nRunning: Percolation threshold sweep...");
    for pct in [0, 20, 40, 60, 80, 90] {
        let scenario = EvalScenario {
            name: format!("percolation_{}pct", pct),
            node_count: 30,
            publisher_count: 3,
            message_rate_per_sec: 5.0,
            low_energy_percentage: pct as f32,
            duration: Duration::from_secs(2),
            ..Default::default()
        };
        let run = run_scenario(&scenario)?;
        println!(
            "  {}% dead: delivery={:.1}%, exhausted={}",
            pct,
            run.delivery.delivery_rate() * 100.0,
            run.energy.nodes_exhausted
        );
        all_runs.push(run);
    }

    // 3. Degradation attacks (like Gossipsub report)
    println!("\nRunning: Degradation attack scenarios...");
    for drop_pct in [10, 30, 50, 70, 90] {
        let scenario = EvalScenario {
            name: format!("degradation_{}pct", drop_pct),
            node_count: 30,
            publisher_count: 3,
            message_rate_per_sec: 5.0,
            duration: Duration::from_secs(2),
            fault_schedule: vec![hypha::eval::FaultEvent {
                time: Duration::ZERO,
                fault: FaultType::Degradation {
                    drop_probability: drop_pct as f32 / 100.0,
                },
            }],
            ..Default::default()
        };
        let run = run_scenario(&scenario)?;
        println!(
            "  {}% drop: delivery={:.1}%, p99={:?}",
            drop_pct,
            run.delivery.delivery_rate() * 100.0,
            run.delivery.p99()
        );
        all_runs.push(run);
    }

    // 4. Network partition
    println!("\nRunning: Network partition scenario...");
    let partition_scenario = EvalScenario {
        name: "network_partition".to_string(),
        node_count: 30,
        publisher_count: 3,
        message_rate_per_sec: 5.0,
        duration: Duration::from_secs(2),
        fault_schedule: vec![hypha::eval::FaultEvent {
            time: Duration::ZERO,
            fault: FaultType::Partition {
                group_a: (0..15).map(|i| format!("node_{}", i)).collect(),
                group_b: (15..30).map(|i| format!("node_{}", i)).collect(),
            },
        }],
        ..Default::default()
    };
    let run = run_scenario(&partition_scenario)?;
    println!(
        "  Partition: delivery={:.1}%, p99={:?}",
        run.delivery.delivery_rate() * 100.0,
        run.delivery.p99()
    );
    all_runs.push(run);

    // 5. Combined stress test (partition + degradation + low energy)
    println!("\nRunning: Combined stress test...");
    let mut stress_scenario = EvalScenario {
        name: "combined_stress".to_string(),
        node_count: 30,
        publisher_count: 3,
        message_rate_per_sec: 5.0,
        low_energy_percentage: 30.0,
        duration: Duration::from_secs(2),
        fault_schedule: vec![
            hypha::eval::FaultEvent {
                time: Duration::ZERO,
                fault: FaultType::Degradation {
                    drop_probability: 0.2,
                },
            },
            hypha::eval::FaultEvent {
                time: Duration::ZERO,
                fault: FaultType::Partition {
                    group_a: (0..15).map(|i| format!("node_{}", i)).collect(),
                    group_b: (15..30).map(|i| format!("node_{}", i)).collect(),
                },
            },
        ],
        ..Default::default()
    };
    stress_scenario.cooldown = Duration::from_secs(5);
    let run = run_scenario(&stress_scenario)?;
    println!(
        "  Stress: delivery={:.1}%, exhausted={}, p99={:?}",
        run.delivery.delivery_rate() * 100.0,
        run.energy.nodes_exhausted,
        run.delivery.p99()
    );
    all_runs.push(run);

    // 6. Recovery via Spike (stall + trigger spike)
    println!("\nRunning: Recovery via Spike scenario...");
    let recovery_scenario = EvalScenario {
        name: "recovery_via_spike".to_string(),
        node_count: 30,
        publisher_count: 3,
        message_rate_per_sec: 5.0,
        duration: Duration::from_secs(4),
        fault_schedule: vec![
            hypha::eval::FaultEvent {
                time: Duration::ZERO,
                fault: FaultType::Degradation {
                    drop_probability: 0.8, // Massive stall
                },
            },
            hypha::eval::FaultEvent {
                time: Duration::from_secs(2),
                fault: FaultType::SyncSpike { intensity: 255 },
            },
        ],
        ..Default::default()
    };
    let run = run_scenario(&recovery_scenario)?;
    println!(
        "  Recovery: delivery={:.1}%, p99={:?}",
        run.delivery.delivery_rate() * 100.0,
        run.delivery.p99()
    );
    all_runs.push(run);

    // Generate summary report
    println!("\n================================");
    println!("EVALUATION SUMMARY");
    println!("================================\n");

    let report: Vec<serde_json::Value> = all_runs
        .iter()
        .map(|run| {
            json!({
                "scenario": run.scenario,
                "node_count": run.node_count,
                "delivery": {
                    "rate": format!("{:.2}%", run.delivery.delivery_rate() * 100.0),
                    "messages_published": run.delivery.messages_published,
                    "messages_delivered": run.delivery.messages_delivered,
                    "p50_ms": run.delivery.p50().map(|d| d.as_millis()),
                    "p90_ms": run.delivery.p90().map(|d| d.as_millis()),
                    "p99_ms": run.delivery.p99().map(|d| d.as_millis()),
                },
                "energy": {
                    "total_mah_consumed": format!("{:.2}", run.energy.total_mah_consumed),
                    "mah_per_delivery": format!("{:.4}", run.energy.mah_per_delivery),
                    "nodes_exhausted": run.energy.nodes_exhausted,
                    "gini_coefficient": format!("{:.3}", run.energy.energy_gini()),
                },
                "consistency": {
                    "converged": run.consistency.converged(),
                    "max_divergence": run.consistency.max_divergence,
                },
                "fault_events": run.fault_events.len(),
            })
        })
        .collect();

    // Print summary table
    println!(
        "{:<25} {:>10} {:>10} {:>10} {:>10}",
        "Scenario", "Delivery%", "p99(ms)", "Exhausted", "mAh/msg"
    );
    println!("{}", "-".repeat(70));

    for run in &all_runs {
        let p99_str = run
            .delivery
            .p99()
            .map(|d| format!("{:.1}", d.as_millis()))
            .unwrap_or("-".to_string());
        println!(
            "{:<25} {:>10.1} {:>10} {:>10} {:>10.4}",
            run.scenario,
            run.delivery.delivery_rate() * 100.0,
            p99_str,
            run.energy.nodes_exhausted,
            run.energy.mah_per_delivery
        );
    }

    // Write detailed report
    let report_path = "hypha_rigorous_eval.json";
    let mut file = File::create(report_path)?;
    file.write_all(serde_json::to_string_pretty(&report)?.as_bytes())?;
    println!("\nDetailed report written to: {}", report_path);

    // Critical analysis
    println!("\n================================");
    println!("CRITICAL ANALYSIS");
    println!("================================\n");

    // Find scenarios that failed to meet targets
    let failed_scenarios: Vec<_> = all_runs
        .iter()
        .filter(|r| r.delivery.delivery_rate() < 0.95)
        .collect();

    if failed_scenarios.is_empty() {
        println!("All scenarios achieved >95% delivery rate.");
    } else {
        println!(
            "WARNING: {} scenarios below 95% delivery threshold:",
            failed_scenarios.len()
        );
        for run in failed_scenarios {
            println!(
                "  - {}: {:.1}%",
                run.scenario,
                run.delivery.delivery_rate() * 100.0
            );
        }
    }

    // Energy efficiency analysis
    let high_energy_scenarios: Vec<_> = all_runs
        .iter()
        .filter(|r| r.energy.mah_per_delivery > 0.5)
        .collect();

    if !high_energy_scenarios.is_empty() {
        println!(
            "\nWARNING: {} scenarios with high energy cost (>0.5 mAh/msg):",
            high_energy_scenarios.len()
        );
        for run in high_energy_scenarios {
            println!(
                "  - {}: {:.3} mAh/msg",
                run.scenario, run.energy.mah_per_delivery
            );
        }
    }

    Ok(())
}
