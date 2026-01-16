# Hypha Development Guide

`hypha` is a bio-inspired agentic P2P coordination layer.

## Architecture Invariants
- **Spore Model**: Every node is a "Spore" with persistence (`fjall`), identity (`ed25519`), and physical state modeling.
- **Mycelial Memory**: LSM-tree based persistence ensures durability on SD cards/Flash with minimal wear.
- **Adaptive Pulse**: Heartbeat intervals are a function of `EnergyScore` (Voltage + mAh).
- **Delta-State Reconciliation**: Nodes compare state-vectors and only exchange deltas.
- **Sovereign Agency**: UCAN-signed delegations for serverless task allocation.

## Bio-Inspired Decisions
- **Energy Pheromones**: Nodes gossip their energy levels to create a gradient. Low-power nodes gravitate toward "MainsHubs" for offloading.
- **Quorum Sensing**: Auction bids are limited by neighborhood density. Spores stay silent if enough healthy peers are already bidding.

## Build & Test
- **Check**: `cargo check`
- **Unit Tests**: `cargo test`
- **Simulations**: `cargo test --test viral_sim`
- **Evaluations**: `cargo run --example mycelium_eval`

## Key Commands
- `cargo run --example basic_node`: Spawns a single spore with local persistence.
- `cargo run --example mycelium_eval`: Runs a data-driven percolation sweep.

## Design nuance
- Identity is stored in `hypha_state/spore_soul`.
- `PhysicalState` is shared via `Arc<Mutex>` for real-time simulation updates.
