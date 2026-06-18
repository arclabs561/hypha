# Hypha Architecture

Hypha coordinates local nodes that have different power budgets and
capabilities. A node can sense, store, compute, or bridge an embedded device.
The current implementation is a host crate plus smaller support crates.

## Crates

`hypha` is the host crate. It owns `SporeNode`, persisted identity through
`fjall`, libp2p networking, task bidding, shared-state sync, and the optional
WASM runtime wrapper.

`hypha-core` contains the small shared type surface: `Capability`, `Task`,
`Bid`, `EnergyStatus`, `PowerMode`, `Metabolism`, and sensor traits. This is the
piece intended to remain usable by firmware and bridge code.

`hypha-ota` contains signed OTA protocol helpers and image-format utilities.

`hypha-firefly` contains no-std pulse-coupled oscillator, peer-table, and LED
state logic used by firmware experiments. Firefly language is kept for the
oscillator model, but it is not a security or power-control claim by itself.

## Runtime Shape

```mermaid
graph TD
    sensor[embedded sensor or local source] --> bridge[bridge or host node]
    bridge --> core[hypha-core types]
    host[hypha host crate] --> core
    host --> store[fjall local store]
    host --> mesh[libp2p gossipsub]
    host --> compute[optional wasmtime runtime]
```

Embedded devices do not run libp2p, tokio, fjall, or wasmtime. They report
energy and sensor state through a transport. A host process runs the full node
and joins the mesh.

## Data Flow

1. A node or bridge produces `EnergyStatus`, sensor readings, or a `Task`.
2. The host node updates local metabolism and capability state.
3. The mesh shares status and control messages over libp2p gossipsub.
4. Nodes can bid for work when they have the requested capability and enough
   energy. The current bidding logic is a local heuristic, not a settled
   distributed auction protocol.
5. Shared-state sync uses `yrs` updates over the mesh.

## Current Status

Implemented:

- Persisted node identity.
- Prototype power-aware heartbeat interval and local task-bidding heuristics.
- libp2p status/control/task topics.
- Shared-state sync plumbing.
- ESP bridge path for newline-delimited `EnergyStatus` JSON.
- Host-side tests for selected firmware logic.

Prototype or incomplete:

- UCAN validation is a placeholder.
- `hypha-core` is not fully no-std-clean yet.
- WASM execution is a wrapper around wasmtime, not a full scheduling system.
- Peer scoring and conductivity are local heuristics. They are not currently a
  GossipSub-style adversarial score or a Physarum-style flow model.
- Task messages and `yrs` shared-state updates do not yet share an explicit
  application-level causality contract.
- Evaluation coverage is still weighted toward graph and delivery behavior; any
  power-saving claim needs current or energy measurements.
- Firmware images, signing, and deployment are not part of a published release
  flow.

## Topology

Roles are derived from capability and power state, not hardcoded topology. A
mains-powered node with storage can act as a sink. Smaller nodes can act as
sources. A deployment with one storage node is a star. A deployment without one
can use buffering and gossip. See [docs/TOPOLOGY.md](docs/TOPOLOGY.md).
