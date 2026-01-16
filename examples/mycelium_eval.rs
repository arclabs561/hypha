use hypha::{Capability, SporeNode, Task};
use serde_json::json;
use std::fs::File;
use std::io::Write;
use std::time::Instant;
use tempfile::tempdir;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut results = Vec::new();

    // Sweeping through the "Dead Node %" to find the Percolation Threshold
    for dead_percentage in [0, 20, 40, 60, 80] {
        println!(
            "Evaluating Mycelium with {}% low-power nodes...",
            dead_percentage
        );

        let start_time = Instant::now();
        let num_nodes = 10;
        let num_dead = (num_nodes as f32 * (dead_percentage as f32 / 100.0)) as usize;

        let mut nodes = Vec::new();
        let tmp = tempdir()?;

        for i in 0..num_nodes {
            let path = tmp.path().join(format!("node_{}", i));
            std::fs::create_dir(&path)?;
            let mut node = SporeNode::new(&path)?;

            if i < num_dead {
                // Simulate a "Dying Spore"
                let mut state = node.physical_state.lock().unwrap();
                state.voltage = 3.35;
                state.mah_remaining = 10.0;
            } else {
                node.add_capability(Capability::Compute(100));
            }
            nodes.push(node);
        }

        // Evaluate a Task across the mesh
        let task = Task {
            id: "viral-task".to_string(),
            required_capability: Capability::Compute(100),
            priority: 1,
            pheromone_intensity: 1.0,
            source_id: "test-source".to_string(),
            auth_token: None,
        };

        let mut successful_bids = 0;
        for node in &nodes {
            if node.evaluate_task(&task, successful_bids).is_some() {
                successful_bids += 1;
            }
        }

        let elapsed = start_time.elapsed();

        results.push(json!({
            "dead_percentage": dead_percentage,
            "successful_bids": successful_bids,
            "healthy_nodes": num_nodes - num_dead,
            "duration_ms": elapsed.as_millis(),
        }));
    }

    let report_path = "hypha_eval_report.json";
    let mut file = File::create(report_path)?;
    file.write_all(serde_json::to_string_pretty(&results)?.as_bytes())?;

    println!("Evaluation complete. Data written to {}", report_path);
    println!("--- RESULT SUMMARY ---");
    for res in results {
        println!(
            "{}% Dead -> {} Bids",
            res["dead_percentage"], res["successful_bids"]
        );
    }

    Ok(())
}
