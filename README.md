# hypha

`hypha` is a Rust-based P2P coordination prototype focused on:

- persistent node identity + local state
- mesh maintenance experiments (graft/prune, scoring, backoff)
- power-aware behavior hooks (heartbeat pacing, bidding heuristics)

## Core Architecture: The Spore Model

Nodes in `hypha` are "Spores"—autonomous units of persistence, networking, and agency.

### 1. Mycelial Memory (`fjall`)
Uses an LSM-tree for local state persistence. This is critical for Raspberry Pi/DIY hardware to minimize SD card wear during high-frequency gossip updates.

### 2. Agentic Capabilities
Nodes register what they can do (Compute, Storage, Sensing). 
- **Power-Aware Bidding**: Nodes evaluate tasks and only bid if they have the required energy (mAh) and voltage stability.
- **Sovereign Agency (prototype)**: UCAN/capability types exist, but task authorization is currently a stub and **not security**.

### 3. Virtual Sensors
A trait-based sensor system allows nodes to treat gossip messages from neighbors as local "Virtual Sensors." This enables privacy-preserving sensor fusion (e.g., mmWave + Audio) across the mesh.

### 4. Adaptive Pulse
Heartbeat intervals stretch dynamically from **1s to 60s** based on real-time `PhysicalState` modeling.

## Testing: High-Fidelity Simulation

We use **`turmoil`** for deterministic test harnesses around timing/power logic.

- `tests/mycelium_world.rs`: basic heartbeat pacing under simulated voltage drain
- `tests/viral_sim.rs`: sanity checks for power-mode driven heartbeat changes

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
cargo test --test mycelium_world
cargo test --test viral_sim
```
