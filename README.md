# vire

`vire` (Viral + Wire) is a Rust-based agentic P2P coordination layer designed for high-write resilience and power efficiency.

## Architecture: The Spore Model

Nodes in `vire` are modeled as "Spores"—self-contained units of persistence, networking, and agency.

- **Mycelial Memory (`fjall`)**: Uses a Log-Structured Merge-tree (LSM) for local state persistence. This is critical for Raspberry Pi setups where frequent small writes can degrade SD cards. `fjall` provides high write throughput for gossip metadata.
- **Viral Networking (`libp2p`)**: Implements `gossipsub` for epidemic message propagation.
- **Sovereign Agency (`ucan`)**: Uses UCAN (User Controlled Authorization Networks) for serverless task delegation. A node can "prove" its right to request work from a neighbor without a central authority.
- **Adaptive Pulse**: Heartbeat intervals stretch dynamically based on `PowerMode` (Normal, LowBattery, Critical).

## Testing Architecture: Deterministic Simulation

We use **`turmoil`** for Deterministic Simulation Testing (DST). This allows us to:
1.  **Freeze Time**: Simulate months of node interaction in seconds.
2.  **Model Power Drain**: Artificially inject "Low Battery" events at specific simulated timestamps to observe network phase transitions.
3.  **Simulate Packet Loss**: Test if the "virus" (coordination message) survives a 50% packet drop rate during a storm.

## Getting Started

```rust
use vire::{SporeNode, PowerMode};
use tempfile::tempdir;

#[tokio::main]
async fn main() {
    let tmp = tempdir().unwrap();
    let mut node = SporeNode::new(tmp.path()).unwrap();
    
    // Switch to low-power mode
    node.set_power_mode(PowerMode::LowBattery);
    
    // Start the swarm
    node.start().await.unwrap();
}
```

## Running Simulations

```bash
cargo test test_simulation_power_drain_viral_death
```
