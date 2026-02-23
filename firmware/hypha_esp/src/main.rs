//! Minimal Hypha ESP firmware: print EnergyStatus JSON over USB CDC every 2s.
//! Build with esp-idf (see firmware/README.md). Host runs: cargo run --bin esp_bridge

use std::thread;
use std::time::Duration;

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let source_id = "esp-1";
    let mut energy_score: f32 = 0.85;

    loop {
        let line = format!(
            r#"{{"source_id":"{}","energy_score":{:.2}}}"#,
            source_id, energy_score
        );
        println!("{}", line);
        thread::sleep(Duration::from_secs(2));
    }
}
