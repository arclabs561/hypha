#![allow(clippy::needless_range_loop)]

//! Mycelial Synchrony & Pressure-Aware Routing Experiment
//!
//! Tests:
//! 1. Phase Alignment: Can nodes reach global synchrony (aligned pulse phases) through local alignment?
//! 2. Pressure-Aware Routing: Do messages flow toward lower-pressure nodes?
//! 3. Mycelial Conductivity: How do pressure gradients affect path thickening?

use hypha::mesh::{MeshConfig, TopicMesh};
use rand::{rng, Rng};
use serde::Serialize;
use std::fs::File;
use std::io::Write;

#[derive(Serialize)]
struct SynchronyResult {
    tick: u32,
    avg_phase: f32,
    phase_variance: f32,
    avg_conductivity: f32,
    avg_pressure: f32,
    delivery_rate: f32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Running Mycelial Synchrony Experiment...");

    let node_count = 50;
    let mut meshes: Vec<TopicMesh> = (0..node_count)
        .map(|_| TopicMesh::new("sync".to_string(), MeshConfig::default()))
        .collect();

    // Initial random connectivity
    let mut rng = rng();
    for i in 0..node_count {
        for _ in 0..10 {
            let j = rng.random_range(0..node_count);
            if i != j {
                meshes[i].add_peer(format!("node-{}", j), 0.8);
            }
        }
    }

    let mut history = Vec::new();
    let mut total_delivered = 0u32;
    let mut messages_published = 0u32;

    for tick in 0..200 {
        // 1. Synchrony: Nodes align pulse with neighbors
        for i in 0..node_count {
            meshes[i].tick_pulse(0.01); // Global time advancement

            // Collect neighbor phases
            let neighbors: Vec<(f32, f32)> = meshes[i]
                .known_peers
                .iter()
                .filter_map(|(id, _)| {
                    let idx = id
                        .strip_prefix("node-")
                        .and_then(|s| s.parse::<usize>().ok())?;
                    if idx < node_count {
                        Some((
                            meshes[idx].pulse_phase,
                            meshes[idx]
                                .known_peers
                                .get(&format!("node-{}", i))?
                                .energy_score,
                        ))
                    } else {
                        None
                    }
                })
                .collect();

            for (phase, energy) in neighbors {
                // Align more strongly with high-energy neighbors
                meshes[i].align_pulse(phase, 0.1 * energy);
            }
        }

        // 2. Pressure & Flow: Simulate message load
        // Node 0 is a heavy publisher (high pressure source)
        if tick % 5 == 0 {
            messages_published += 1;
            let delivered = simulate_sync_propagation(&mut meshes, &format!("m-{}", tick), 0);
            total_delivered += delivered;
        }

        // Update pressures based on deliveries
        for i in 0..node_count {
            let current_pressure = meshes[i].local_pressure;
            // Pressure increases with backlog, decreases with processing
            let new_pressure =
                (current_pressure * 0.9 + (meshes[i].message_cache.len() as f32 * 0.01)).min(10.0);
            meshes[i].set_pressure(new_pressure);

            // Share pressure with neighbors
            let my_id = format!("node-{}", i);
            for j in 0..node_count {
                if i != j {
                    meshes[j].update_peer_pressure(&my_id, new_pressure);
                }
            }
        }

        // 3. Heartbeat & Adaptation
        if tick % 10 == 0 {
            for i in 0..node_count {
                meshes[i].heartbeat();
            }
        }

        // Metrics
        let phases: Vec<f32> = meshes.iter().map(|m| m.pulse_phase).collect();
        let avg_phase = phases.iter().sum::<f32>() / node_count as f32;
        let phase_variance =
            phases.iter().map(|&p| (p - avg_phase).powi(2)).sum::<f32>() / node_count as f32;
        let avg_conductivity = meshes[0]
            .known_peers
            .values()
            .map(|p| p.conductivity)
            .sum::<f32>()
            / meshes[0].known_peers.len() as f32;
        let avg_pressure = meshes.iter().map(|m| m.local_pressure).sum::<f32>() / node_count as f32;

        history.push(SynchronyResult {
            tick,
            avg_phase,
            phase_variance,
            avg_conductivity,
            avg_pressure,
            delivery_rate: total_delivered as f32
                / (messages_published.max(1) as usize * (node_count - 1)) as f32,
        });

        if tick % 50 == 0 {
            println!(
                "Tick {}: Variance={:.4}, Avg Pressure={:.4}",
                tick, phase_variance, avg_pressure
            );
        }
    }

    // Write results to JSON for dashboard
    let json = serde_json::to_string_pretty(&history)?;
    std::fs::write("hypha_sync_eval.json", json)?;
    println!("Results saved to hypha_sync_eval.json");

    // Update Dashboard HTML
    generate_sync_dashboard(&history)?;

    Ok(())
}

fn simulate_sync_propagation(meshes: &mut [TopicMesh], msg_id: &str, publisher_idx: usize) -> u32 {
    let mut delivered = 0u32;
    let node_count = meshes.len();
    let mut received = vec![false; node_count];
    received[publisher_idx] = true;

    // Use pressure-aware forwarding: favor lower pressure nodes
    let mut current_wave = vec![publisher_idx];
    for _hop in 0..5 {
        let mut next_wave = Vec::new();
        for &idx in &current_wave {
            // Get all neighbors
            let neighbors: Vec<usize> = meshes[idx]
                .known_peers
                .keys()
                .filter_map(|id| {
                    id.strip_prefix("node-")
                        .and_then(|s| s.parse::<usize>().ok())
                })
                .filter(|&i| i < node_count && !received[i])
                .collect();

            // Choose targets with bias toward low pressure
            let mut targets = neighbors;
            targets.sort_by(|&a, &b| {
                meshes[a]
                    .local_pressure
                    .partial_cmp(&meshes[b].local_pressure)
                    .unwrap()
            });

            // Forward to top 3 best neighbors
            for &t in targets.iter().take(3) {
                received[t] = true;
                delivered += 1;
                meshes[t].record_message(&format!("node-{}", idx), msg_id);
                next_wave.push(t);
            }
        }
        current_wave = next_wave;
        if current_wave.is_empty() {
            break;
        }
    }
    delivered
}

fn generate_sync_dashboard(history: &[SynchronyResult]) -> Result<(), Box<dyn std::error::Error>> {
    let ticks: Vec<u32> = history.iter().map(|r| r.tick).collect();
    let variances: Vec<f32> = history.iter().map(|r| r.phase_variance).collect();
    let pressures: Vec<f32> = history.iter().map(|r| r.avg_pressure).collect();
    let conductivities: Vec<f32> = history.iter().map(|r| r.avg_conductivity).collect();

    let html = format!(
        r#"
<!DOCTYPE html>
<html>
<head>
    <title>Hypha Synchrony Dashboard</title>
    <script src="https://cdn.jsdelivr.net/npm/chart.js"></script>
    <style>
        body {{ font-family: -apple-system, sans-serif; background: #0f172a; color: #e2e8f0; padding: 20px; }}
        .container {{ max-width: 1000px; margin: 0 auto; }}
        .card {{ background: #1e293b; border-radius: 12px; padding: 24px; margin-bottom: 24px; border: 1px solid #334155; }}
        h1 {{ text-align: center; }}
        canvas {{ height: 300px !important; }}
    </style>
</head>
<body>
    <div class="container">
        <h1>Hypha Mycelial Synchrony</h1>
        
        <div class="card">
            <h2>Phase Variance (Emergent Synchrony)</h2>
            <p>Decreasing variance indicates global heartbeat alignment.</p>
            <canvas id="varianceChart"></canvas>
        </div>

        <div class="card">
            <h2>Pressure & Conductivity</h2>
            <p>Pressure gradients driving path thickening.</p>
            <canvas id="pressureChart"></canvas>
        </div>
    </div>

    <script>
        const options = {{ 
            responsive: true, maintainAspectRatio: false,
            scales: {{ y: {{ ticks: {{ color: '#94a3b8' }} }}, x: {{ ticks: {{ color: '#94a3b8' }} }} }},
            plugins: {{ legend: {{ labels: {{ color: '#94a3b8' }} }} }}
        }};

        new Chart(document.getElementById('varianceChart'), {{
            type: 'line',
            data: {{
                labels: {ticks:?},
                datasets: [{{
                    label: 'Phase Variance',
                    data: {variances:?},
                    borderColor: '#f43f5e',
                    fill: false
                }}]
            }},
            options: options
        }});

        new Chart(document.getElementById('pressureChart'), {{
            type: 'line',
            data: {{
                labels: {ticks:?},
                datasets: [
                    {{ label: 'Avg Pressure', data: {pressures:?}, borderColor: '#38bdf8' }},
                    {{ label: 'Avg Conductivity', data: {conductivities:?}, borderColor: '#fbbf24' }}
                ]
            }},
            options: options
        }});
    </script>
</body>
</html>
    "#,
        ticks = ticks,
        variances = variances,
        pressures = pressures,
        conductivities = conductivities,
    );

    let mut file = File::create("hypha_sync_dashboard.html")?;
    file.write_all(html.as_bytes())?;
    println!("Synchrony dashboard generated: hypha_sync_dashboard.html");
    Ok(())
}
