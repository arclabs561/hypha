default:
    @just --list

# Run bridge reading EnergyStatus from stdin (test without device)
esp-bridge-stdin:
    (printf '%s\n' '{"source_id":"esp-1","energy_score":0.85}'; sleep 5) | cargo run --bin esp_bridge -- --stdin

# Run bridge on USB serial (default: /dev/cu.usbmodem1101). Flash firmware first if device doesn't send JSON.
esp-bridge port="/dev/cu.usbmodem1101":
    cargo run --bin esp_bridge -- --port {{port}}

check:
    cargo fmt --all -- --check
    cargo clippy --all-targets -- -D warnings
    cargo test

test:
    cargo test

fmt:
    cargo fmt --all
