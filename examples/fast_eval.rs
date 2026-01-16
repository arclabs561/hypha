//! Fast in-memory evaluation without disk persistence
//!
//! Measures delivery rate, latency, and energy metrics without fjall overhead.

use hypha::eval::{ConsistencyMetrics, DeliveryMetrics, EnergyMetrics, EvalRun};
use rand::Rng;
use serde_json::json;
use std::fs::File;
use std::io::Write;
use std::time::Instant;

/// Lightweight node representation for fast evaluation
struct LightNode {
    energy_score: f32,
    mah_remaining: f32,
    messages_received: Vec<String>,
}

impl LightNode {
    fn new(_id: usize, initial_energy: f32) -> Self {
        Self {
            energy_score: initial_energy,
            mah_remaining: initial_energy * 2500.0,
            messages_received: Vec::new(),
        }
    }

    fn is_exhausted(&self) -> bool {
        self.energy_score < 0.05
    }

    fn consume_energy(&mut self, mah: f32) {
        self.mah_remaining = (self.mah_remaining - mah).max(0.0);
        self.energy_score = (self.mah_remaining / 2500.0).clamp(0.0, 1.0);
    }
}

/// Simulate propagation without disk I/O
fn simulate_propagation(
    nodes: &mut [LightNode],
    msg_id: &str,
    drop_prob: f32,
    partitioned: bool,
) -> Vec<u64> {
    let mut rng = rand::thread_rng();
    let mut latencies = Vec::new();
    let node_count = nodes.len();
    let half = node_count / 2;

    for (i, node) in nodes.iter_mut().enumerate() {
        if node.is_exhausted() {
            continue;
        }

        // Partition: first half can't reach second half
        if partitioned && i >= half {
            continue;
        }

        if rng.gen::<f32>() < drop_prob {
            continue;
        }

        // Simulate hop-based latency
        let hops = (i % 3) + 1;
        let base_latency = hops as u64 * 15_000;
        let jitter = rng.gen_range(0..5_000);
        latencies.push(base_latency + jitter);

        node.messages_received.push(msg_id.to_string());
        node.consume_energy(0.1);
    }

    latencies
}

fn run_scenario(
    name: &str,
    node_count: usize,
    low_energy_pct: f32,
    drop_prob: f32,
    partitioned: bool,
    message_count: usize,
) -> EvalRun {
    let start = Instant::now();
    let mut rng = rand::thread_rng();

    let low_energy_count = (node_count as f32 * low_energy_pct / 100.0) as usize;

    // Create nodes
    let mut nodes: Vec<_> = (0..node_count)
        .map(|i| {
            let energy = if i < low_energy_count {
                rng.gen_range(0.01..0.05) // Near exhaustion
            } else {
                rng.gen_range(0.7..1.0) // Healthy
            };
            LightNode::new(i, energy)
        })
        .collect();

    let initial_energy: f32 = nodes.iter().map(|n| n.mah_remaining).sum();

    let mut delivery = DeliveryMetrics::default();

    for msg_idx in 0..message_count {
        let msg_id = format!("{}-{}", name, msg_idx);
        delivery.messages_published += 1;
        delivery.expected_deliveries += node_count as u64;

        let latencies = simulate_propagation(&mut nodes, &msg_id, drop_prob, partitioned);

        delivery.messages_delivered += latencies.len() as u64;
        delivery.latencies_us.extend(latencies);

        // Publisher energy cost
        let publisher_idx = msg_idx % (node_count / 10).max(1);
        if publisher_idx < nodes.len() {
            nodes[publisher_idx].consume_energy(0.5);
        }
    }

    let final_energy: f32 = nodes.iter().map(|n| n.mah_remaining).sum();
    let mah_consumed = initial_energy - final_energy;
    let nodes_exhausted = nodes.iter().filter(|n| n.is_exhausted()).count();
    let final_scores: Vec<f32> = nodes.iter().map(|n| n.energy_score).collect();

    let mah_per_delivery = if delivery.messages_delivered > 0 {
        mah_consumed / delivery.messages_delivered as f32
    } else {
        0.0
    };

    EvalRun {
        scenario: name.to_string(),
        node_count,
        duration: start.elapsed(),
        delivery,
        energy: EnergyMetrics {
            total_mah_consumed: mah_consumed,
            mah_per_delivery,
            nodes_exhausted,
            final_energy_scores: final_scores,
        },
        consistency: ConsistencyMetrics::default(),
        fault_events: vec![],
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Hypha Fast Evaluation Suite");
    println!("===========================\n");

    let mut all_runs: Vec<EvalRun> = Vec::new();

    // 1. Baseline
    println!("Baseline:");
    let run = run_scenario("baseline", 100, 0.0, 0.0, false, 100);
    println!(
        "  Delivery: {:.1}%, p99: {:?}",
        run.delivery.delivery_rate() * 100.0,
        run.delivery.p99()
    );
    all_runs.push(run);

    // 2. Percolation sweep
    println!("\nPercolation Threshold:");
    for pct in [0, 10, 20, 30, 40, 50, 60, 70, 80, 90] {
        let run = run_scenario(
            &format!("percolation_{}pct", pct),
            100,
            pct as f32,
            0.0,
            false,
            100,
        );
        println!(
            "  {:2}% dead: delivery={:5.1}%, exhausted={:2}",
            pct,
            run.delivery.delivery_rate() * 100.0,
            run.energy.nodes_exhausted
        );
        all_runs.push(run);
    }

    // 3. Degradation attacks
    println!("\nDegradation Attacks:");
    for drop_pct in [10, 20, 30, 40, 50, 60, 70, 80, 90] {
        let run = run_scenario(
            &format!("degradation_{}pct", drop_pct),
            100,
            0.0,
            drop_pct as f32 / 100.0,
            false,
            100,
        );
        println!(
            "  {:2}% drop: delivery={:5.1}%, p99={:?}",
            drop_pct,
            run.delivery.delivery_rate() * 100.0,
            run.delivery.p99()
        );
        all_runs.push(run);
    }

    // 4. Network partition
    println!("\nNetwork Partition:");
    let run = run_scenario("partition", 100, 0.0, 0.0, true, 100);
    println!(
        "  Partitioned: delivery={:.1}%",
        run.delivery.delivery_rate() * 100.0
    );
    all_runs.push(run);

    // 5. Combined stress
    println!("\nCombined Stress (30% dead + 20% drop + partition):");
    let run = run_scenario("combined_stress", 100, 30.0, 0.2, true, 100);
    println!(
        "  Stress: delivery={:.1}%, exhausted={}",
        run.delivery.delivery_rate() * 100.0,
        run.energy.nodes_exhausted
    );
    all_runs.push(run);

    // Summary table
    println!("\n{}", "=".repeat(75));
    println!("EVALUATION SUMMARY");
    println!("{}", "=".repeat(75));
    println!(
        "\n{:<25} {:>10} {:>12} {:>10} {:>10}",
        "Scenario", "Delivery%", "p99(ms)", "Exhausted", "mAh/msg"
    );
    println!("{}", "-".repeat(75));

    for run in &all_runs {
        let p99 = run
            .delivery
            .p99()
            .map(|d| format!("{:.1}", d.as_millis() as f64))
            .unwrap_or("-".to_string());
        println!(
            "{:<25} {:>10.1} {:>12} {:>10} {:>10.4}",
            run.scenario,
            run.delivery.delivery_rate() * 100.0,
            p99,
            run.energy.nodes_exhausted,
            run.energy.mah_per_delivery
        );
    }

    // Critical analysis
    println!("\n{}", "=".repeat(75));
    println!("CRITICAL ANALYSIS");
    println!("{}\n", "=".repeat(75));

    // Identify failure modes
    let failures: Vec<_> = all_runs
        .iter()
        .filter(|r| r.delivery.delivery_rate() < 0.5)
        .collect();

    if failures.is_empty() {
        println!("No scenarios dropped below 50% delivery.");
    } else {
        println!("FAILURE MODES (delivery < 50%):");
        for run in &failures {
            println!(
                "  - {}: {:.1}%",
                run.scenario,
                run.delivery.delivery_rate() * 100.0
            );
        }
    }

    // Percolation threshold analysis
    let percolation_runs: Vec<_> = all_runs
        .iter()
        .filter(|r| r.scenario.starts_with("percolation_"))
        .collect();

    if !percolation_runs.is_empty() {
        println!("\nPERCOLATION THRESHOLD:");
        // Find where delivery drops below 50%
        for run in &percolation_runs {
            let rate = run.delivery.delivery_rate();
            if rate < 0.5 && rate > 0.4 {
                println!(
                    "  Network approaches critical failure near: {}",
                    run.scenario
                );
                break;
            }
        }

        // Linear regression on percolation data
        let points: Vec<(f64, f64)> = percolation_runs
            .iter()
            .map(|r| {
                let dead_pct = r
                    .scenario
                    .strip_prefix("percolation_")
                    .and_then(|s| s.strip_suffix("pct"))
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);
                (dead_pct, r.delivery.delivery_rate() * 100.0)
            })
            .collect();

        println!("  Dead% -> Delivery% correlation:");
        for (dead, delivery) in &points {
            let expected = 100.0 - dead;
            let diff = delivery - expected;
            println!(
                "    {:2}% dead: {:.1}% delivery (delta: {:+.1}%)",
                *dead as i32, delivery, diff
            );
        }
    }

    // Energy efficiency ranking
    println!("\nENERGY EFFICIENCY (best to worst):");
    let mut energy_ranked: Vec<_> = all_runs
        .iter()
        .filter(|r| r.energy.mah_per_delivery > 0.0)
        .collect();
    energy_ranked.sort_by(|a, b| {
        a.energy
            .mah_per_delivery
            .partial_cmp(&b.energy.mah_per_delivery)
            .unwrap()
    });
    for (i, run) in energy_ranked.iter().take(5).enumerate() {
        println!(
            "  {}. {}: {:.4} mAh/msg",
            i + 1,
            run.scenario,
            run.energy.mah_per_delivery
        );
    }

    // Write JSON report
    let report: Vec<serde_json::Value> = all_runs
        .iter()
        .map(|run| {
            json!({
                "scenario": run.scenario,
                "delivery_rate_pct": format!("{:.2}", run.delivery.delivery_rate() * 100.0),
                "p50_ms": run.delivery.p50().map(|d| d.as_millis()),
                "p90_ms": run.delivery.p90().map(|d| d.as_millis()),
                "p99_ms": run.delivery.p99().map(|d| d.as_millis()),
                "messages_published": run.delivery.messages_published,
                "messages_delivered": run.delivery.messages_delivered,
                "expected_deliveries": run.delivery.expected_deliveries,
                "mah_per_delivery": format!("{:.4}", run.energy.mah_per_delivery),
                "nodes_exhausted": run.energy.nodes_exhausted,
                "gini_coefficient": format!("{:.3}", run.energy.energy_gini()),
            })
        })
        .collect();

    let report_path = "hypha_fast_eval.json";
    let mut file = File::create(report_path)?;
    file.write_all(serde_json::to_string_pretty(&report)?.as_bytes())?;
    println!("\nDetailed report: {}", report_path);

    Ok(())
}
