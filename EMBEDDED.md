# Embedded / resource-constrained targets

Hypha is intended to be usable in embedded and resource-constrained environments (e.g. ESP32, nRF, STM32) as well as on hosts (Raspberry Pi, servers). This document describes the split and how to get there.

## Intent

- **Embedded device**: Runs a minimal “spore” stack: identity, metabolism (power state), capabilities, sensors, and a **transport shim**. It does **not** run the full P2P stack (libp2p, CRDT sync, WASM compute).
- **Host**: Runs the full `hypha` node (mesh, gossipsub, persistence, optional WASM). It can **bridge** one or more embedded devices into the mesh (e.g. over USB serial or BLE).

## Crate split

| Crate | Role | Typical use |
|-------|------|-------------|
| **hypha-core** | Types, metabolism, capabilities, virtual sensors. No networking, no persistence, no WASM. | Shared by both embedded firmware and host. Can be built with `no_std` + `alloc` for MCU. |
| **hypha** | Full node: SporeNode, fjall, libp2p, yrs sync, wasmtime. Depends on hypha-core. | Host only (Pi, Mac, server). |

Embedded firmware depends only on `hypha-core`. It implements `Metabolism` from real hardware (e.g. ADC voltage, fuel gauge), reports `EnergyStatus` and sensor readings, and sends them over a transport (serial, BLE, LoRa) to a host that runs full `hypha` and proxies the device into the mesh.

## What lives in hypha-core (embedding-friendly)

- **Types**: `Capability`, `Task`, `Bid`, `EnergyStatus`, `PowerMode`
- **Metabolism**: trait `Metabolism`, `BatteryMetabolism`, `MockMetabolism`
- **Sensors**: trait `VirtualSensor`, `BasicSensor`

These use only `core`/`alloc`, `serde` (with `no_default_features`), and optional `std` for compatibility. No `libp2p`, `tokio`, `fjall`, `yrs`, or `wasmtime`.

## What stays in hypha (host-only)

- **SporeNode**: identity recovery from storage (fjall), libp2p swarm, mesh, sync
- **TopicMesh** / mesh logic: `Instant`, `HashMap`, `rand` — not suitable for bare-metal without a shim
- **Sync**: yrs CRDT over gossipsub
- **Compute**: wasmtime sandbox

## Roadmap for embedded

1. **Done**: Extract `hypha-core` with types + metabolism + sensors; host `hypha` depends on it.
2. **Next**: Add `no_std` + `alloc` support to `hypha-core` (feature flags, gate `std::any::Any` and similar).
3. **Then**: Define a small **transport** abstraction (e.g. “send this `EnergyStatus` / sensor payload”) so embedded firmware can push state to a host bridge without depending on libp2p.
4. **Optional**: On MCU, provide a minimal “mesh view” or peer list updated by the host (e.g. over serial) so the device can do local decisions (e.g. “don’t bid when host says 3+ peers already bidding”).

## Is it useful?

**Yes, if** you want:

- **Real power state in the mesh** — ESP (or other MCU) reports actual voltage/mAh so hypha’s power-aware bidding and “energy gradient” are grounded in hardware, not only simulation.
- **Real sensors as first-class spores** — device advertises `Capability::Sensing(...)`, sends readings; the mesh treats it as a peer for routing and tasks without running libp2p on the device.
- **Edge presence** — battery/solar devices (field sensors, portable nodes) participate in the coordination layer via a single host bridge instead of each running a full stack.

**Less useful if** you only need simulated nodes or host-only deployments; then the current hypha + turmoil setup is enough.

## Planning: phases and deliverables

Use this to decide what to build and in what order.

| Phase | Goal | Deliverables | Depends on |
|-------|------|---------------|------------|
| **0** | Prove the split works on host | (Done) `hypha-core` crate; host `hypha` uses it. | — |
| **1** | ESP appears in the mesh as a data source | (1a) ESP firmware: read voltage (ADC), build `EnergyStatus` + one capability, serialize to JSON or simple binary. (1b) Host bridge: small daemon that reads from `/dev/cu.usbmodem*`, parses payloads, and either injects into an existing SporeNode (e.g. virtual sensor) or runs a minimal hypha node that represents the ESP as a peer. (1c) See the ESP’s energy score and capability in the mesh (e.g. in an existing dashboard or log). | ESP dev env (esp-rs or IDF), USB serial |
| **2** | Reliable wire format and identity | (2a) Define a minimal **transport envelope** (e.g. length-prefixed JSON or CBOR: `EnergyStatus`, optional sensor readings, device id). (2b) Optionally: device identity (e.g. derived from ESP eFuse or stored key) so the host can map serial lines to stable peer ids. (2c) Host bridge subscribes to mesh and can push **downstream** to the device (e.g. “current peer count” or “don’t bid”) so the ESP can adapt. | Phase 1 |
| **3** | hypha-core on device without std | (3a) Add `no_std` + `alloc` to `hypha-core` (feature flags, optional `std`). (3b) Ensure serde and types work with `alloc::string::String` / `alloc::vec::Vec`. (3c) ESP firmware depends on `hypha-core` with `default-features = false` and uses it for all hypha types. | Phase 1 or 2 |
| **4** | Multiple devices, production-ish bridge | (4a) Bridge supports multiple serial ports or BLE devices. (4b) Config file or CLI to map device id → capability set and topic. (4c) Optional: reconnect handling, backoff, and basic auth or attestation. | Phase 2 |

**Suggested order for you:** Do **Phase 1** first. It’s the smallest path to “ESP in the mesh”: one firmware binary that sends `EnergyStatus` (+ optional sensor) over USB, and one host program that reads that port and feeds hypha. Phase 2 tightens the contract; Phase 3 makes the crate truly embeddable (no_std); Phase 4 scales out.

## Real use: ESP bridge (Phase 1)

- **Binary**: `cargo run --bin esp_bridge` reads EnergyStatus JSON from USB serial (or `--stdin` for testing) and drives a SporeNode’s metabolism; the node joins the mesh and advertises the device’s energy.
- **Just**: `just esp-bridge` (real port), `just esp-bridge-stdin` (test without device).
- **Device**: Plug ESP (e.g. `/dev/cu.usbmodem1101`). If the board doesn’t already send newline-delimited `{"source_id":"…","energy_score":0.85}` JSON, flash the firmware in `firmware/hypha_esp` (see `firmware/README.md`).
- **Tested**: Bridge opens the real USB port and runs; stdin test shows energy updates and Spore active. After flashing the firmware, you should see `ESP energy update` logs and the mesh using the device’s score.

## Using the ESP (or other MCU) today

- Build firmware that uses **hypha-core** only: read ADC/voltage, implement `Metabolism`, produce `EnergyStatus` and capability/sensor payloads.
- Send those payloads over USB CDC (or UART/BLE) to a host process that runs full `hypha` and injects the device as a virtual peer or sensor source.
- No need to run libp2p or CRDT on the device; the host is the mesh participant.
