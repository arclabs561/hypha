//! Bridge: read EnergyStatus from ESP over USB serial and drive a Hypha SporeNode.
//!
//! The node's metabolism is updated from the device; the node joins the mesh
//! and advertises that energy. One process = one ESP-backed spore.
//!
//! Usage:
//!   cargo run --bin esp_bridge -- [--port /dev/cu.usbmodem1101]
//!   cargo run --bin esp_bridge -- --stdin   # read JSON lines from stdin (test without device)
//!   (ESP sends newline-delimited JSON: {"source_id":"esp-1","energy_score":0.85})

use hypha::{EnergyStatus, MockMetabolism, SporeNode};
use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tracing::info;

const DEFAULT_PORT: &str = "/dev/cu.usbmodem1101";
const BAUD: u32 = 115200;

fn apply_energy_line(line: &str, metabolism: &std::sync::Mutex<MockMetabolism>) {
    let s = line.trim();
    if s.is_empty() {
        return;
    }
    if let Ok(status) = serde_json::from_str::<EnergyStatus>(s) {
        if let Ok(mut m) = metabolism.lock() {
            m.energy = status.energy_score.clamp(0.0, 1.0);
            info!(
                source_id = %status.source_id,
                energy_score = status.energy_score,
                "ESP energy update"
            );
        }
    }
}

fn serial_reader(port_path: String, metabolism: Arc<std::sync::Mutex<MockMetabolism>>) {
    loop {
        let Ok(port) = serialport::new(port_path.clone(), BAUD)
            .timeout(Duration::from_millis(500))
            .open()
        else {
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        info!(path = %port_path, "Serial port opened");
        let mut reader = BufReader::new(port);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => apply_energy_line(&line, &metabolism),
                Err(_) => break,
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn stdin_reader(metabolism: Arc<std::sync::Mutex<MockMetabolism>>) {
    let mut reader = BufReader::new(std::io::stdin());
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).map(|n| n == 0).unwrap_or(true) {
            break;
        }
        apply_energy_line(&line, &metabolism);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    let use_stdin = args.iter().any(|a| a == "--stdin");
    let port = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| DEFAULT_PORT.to_string());

    let tmp = tempdir()?;
    let metabolism = Arc::new(std::sync::Mutex::new(MockMetabolism::new(0.5, false)));
    let metabolism_clone = metabolism.clone();

    if use_stdin {
        std::thread::spawn(move || stdin_reader(metabolism_clone));
        info!("Reading EnergyStatus JSON lines from stdin.");
    } else {
        std::thread::spawn(move || serial_reader(port, metabolism_clone));
    }

    let mut node = SporeNode::new_with_metabolism(tmp.path(), metabolism)?;
    node.add_capability(hypha::Capability::Sensing("esp".to_string()));

    info!("Spore node started; metabolism driven by ESP. Ctrl+C to stop.");
    node.start().await?;
    Ok(())
}
