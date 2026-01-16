# hypha

`hypha` (Fungal Branching) is a Rust-based agentic P2P coordination layer designed for high-write resilience, power efficiency, and physical-world interaction.

## Core Architecture: The Spore Model

Nodes in `hypha` are "Spores"—autonomous units of persistence, networking, and agency.

### 1. Mycelial Memory (`fjall`)
Uses an LSM-tree for local state persistence. This is critical for Raspberry Pi/DIY hardware to minimize SD card wear during high-frequency gossip updates.

### 2. Agentic Capabilities
Nodes register what they can do (Compute, Storage, Sensing). 
- **Power-Aware Bidding**: Nodes evaluate tasks and only bid if they have the required energy (mAh) and voltage stability.
- **Sovereign Agency**: Every delegation is cryptographically signed using Ed25519 keys, creating a serverless "Provenance of Trust."

### 3. Virtual Sensors
A trait-based sensor system allows nodes to treat gossip messages from neighbors as local "Virtual Sensors." This enables privacy-preserving sensor fusion (e.g., mmWave + Audio) across the mesh.

### 4. Adaptive Pulse
Heartbeat intervals stretch dynamically from **1s to 60s** based on real-time `PhysicalState` modeling.

## Testing: High-Fidelity Simulation

We use **`turmoil`** for Deterministic Simulation Testing (DST).
- **Physical Force Modeling**: We simulate voltage drops and energy consumption to observe how the network pulse adapts.
- **Reconciliation Testing**: Verify that "dormant" nodes can sync their memories using Delta-State reconciliation after long sleep cycles.

## Getting Started

```rust
use hypha::{SporeNode, PowerMode, Capability};
use tempfile::tempdir;

#[tokio::main]
async fn main() {
    let tmp = tempdir().unwrap();
    let mut node = SporeNode::new(tmp.path()).unwrap();
    
    // Register a local sensor/capability
    node.add_capability(Capability::Sensing("mmWave".to_string()));
    
    // The node will automatically adjust its pulse based on voltage
    node.start().await.unwrap();
}
```

## Running Simulations

```bash
cargo test test_mycelium_energy_drain_simulation
```
