# hypha

`hypha` is a Rust-based P2P coordination prototype focused on:

- Persistent node identity + local state (LSM-tree)
- Power-aware mesh maintenance (graft/prune, scoring, backoff)
- Adaptive heartbeat pacing and bidding heuristics

## Architecture

Nodes in `hypha` are "Spores"—autonomous units of persistence, networking, and agency.

### 1. Persistence (`fjall`)
Uses an LSM-tree for local state persistence. Critical for Raspberry Pi/DIY hardware to minimize SD card wear during high-frequency gossip updates.

### 2. Capabilities & Power
Nodes register capabilities (Compute, Storage, Sensing).
- **Power-Aware Bidding**: Nodes evaluate tasks and only bid if they have the required energy (mAh) and voltage stability.
- **Agency**: UCAN/capability types exist (prototype).

### 3. Virtual Sensors
A trait-based sensor system allows nodes to treat gossip messages from neighbors as local "Virtual Sensors." Enables sensor fusion (e.g., mmWave + Audio) across the mesh.

### 4. Adaptive Pulse
Heartbeat intervals stretch dynamically from **1s to 60s** based on real-time `PhysicalState` modeling (voltage, drain).

## Testing

Uses **`turmoil`** for deterministic simulation.

- `tests/mycelium_world.rs`: Basic heartbeat pacing under simulated voltage drain.
- `tests/viral_sim.rs`: Sanity checks for power-mode driven changes.

## Usage

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
