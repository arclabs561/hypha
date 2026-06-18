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

The examples are small demonstrations of current heuristics, not proofs of a
distributed scheduling or routing model:

```bash
cargo run --example mycelium_eval
cargo run --example slime_mold_auction
```

The allocation examples are intentionally synthetic. They exercise local task
bidding and diffusion-style scoring, but the mechanism is still a heuristic and
should not be read as a formal Physarum, auction, or adversarial mesh protocol.

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
- Peer scores, conductivity, task diffusion, and allocation are prototype
  heuristics. They do not yet carry the decay, validation penalties, causality
  contracts, or measurements needed for stronger routing, security, or power
  claims.
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
