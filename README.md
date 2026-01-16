# vire

`vire` (Viral + Wire) is a Rust-based P2P coordination layer designed for resilience and low-power efficiency.

## Features

- **Viral Coordination**: Uses `libp2p`'s `gossipsub` for efficient message propagation.
- **Low-Battery Awareness**: Adaptive duty cycling and gossip frequency based on device power state.
- **Deterministic Testing**: Integrated with `turmoil` for simulated network testing.
- **Embedded Ready**: Designed to run on resource-constrained devices like Raspberry Pi.

## Coordination Strategies

- **Normal**: Full participation in gossip and relay.
- **LowBattery**: Reduced heartbeat frequency, limited relaying.
- **Critical**: Passive listening only, minimal pulse to maintain presence.

## Sandboxing & Simulation

We use `turmoil` to simulate network partitions, latency, and packet loss in a deterministic environment.

```rust
#[cfg(test)]
mod tests {
    use turmoil;
    // turmoil tests here
}
```

## Getting Started

```rust
use vire::{SporeNode, PowerMode};

#[tokio::main]
async fn main() {
    let mut node = SporeNode::new();
    node.set_power_mode(PowerMode::LowBattery);
    node.start().await.unwrap();
}
```
