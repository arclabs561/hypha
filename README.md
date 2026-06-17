# hypha

[![CI](https://github.com/arclabs561/hypha/actions/workflows/ci.yml/badge.svg)](https://github.com/arclabs561/hypha/actions/workflows/ci.yml)

Power-aware coordination for local sensor and compute nodes.

`hypha` models nodes as spores with a stable identity, a local store, a power
state, and a set of capabilities. The host crate runs a libp2p-based node. The
`hypha-core` crate holds the smaller type surface used by embedded and bridge
code.

## Build

```bash
git clone https://github.com/arclabs561/hypha
cd hypha
cargo test
```

The workspace is not published to crates.io yet. Use it from a checkout.

## Usage

Create a node, register what it can do, and let its power state affect whether
it bids for work:

```rust
use hypha::{Capability, PowerMode, SporeNode};
use tempfile::tempdir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let mut node = SporeNode::new(tmp.path())?;

    node.add_capability(Capability::Compute(100));
    node.set_power_mode(PowerMode::Normal);

    println!("energy={:.2}", node.energy_score());
    Ok(())
}
```

Run a small allocation example:

```bash
cargo run --example slime_mold_auction
```

Example output:

```text
Running Slime Mold Auction Experiment...
Wave 1: Task pheromone reached 4 nodes.
Wave 2: Task pheromone reached 12 nodes.
Wave 3: Task pheromone reached 24 nodes.
Wave 4: Task pheromone reached 28 nodes.
Wave 5: Task pheromone reached 30 nodes.

Bidding Results (Top 5):
  Bidder: node-3, Weighted Score: 1.0000
  Bidder: node-27, Weighted Score: 0.6700
  Bidder: node-24, Weighted Score: 0.5349
  Bidder: node-18, Weighted Score: 0.4547
  Bidder: node-0, Weighted Score: 0.3064

Winner: node-3 - task successfully allocated via mycelial gradient.
```

## Layout

- `src/`: host node, libp2p wiring, task bidding, sync, and examples.
- `crates/hypha-core/`: capability, metabolism, task, bid, and sensor types.
- `crates/hypha-ota/`: signed OTA protocol helpers.
- `crates/hypha-firefly/`: no-std firefly synchronization and LED logic.
- `firmware/`: ESP experiments and host-side firmware logic tests.
- `tests/`: simulation, schema compatibility, adversarial input, and libp2p tests.

## Embedded Split

Embedded devices do not run the full host crate. They use `hypha-core` types and
send readings to a host bridge. See [EMBEDDED.md](EMBEDDED.md) for the split and
the current ESP bridge path.

## Limitations

- The root `hypha` crate is host-only. It depends on libp2p, tokio, fjall, and
  wasmtime.
- UCAN handling is a placeholder and must not be treated as authorization.
- `hypha-core` is being kept small, but it is not fully no-std-clean yet.
- The firmware directories are experiments. Built images and signing keys are
  intentionally ignored because images may contain deployment credentials.

## Checks

```bash
just check
```

CI runs `cargo check --all-targets`, `cargo test`, `cargo fmt --all -- --check`,
and `cargo clippy --all-targets -- -D warnings`.

## License

Licensed under either Apache-2.0 or MIT.
