# Hypha ESP firmware

Prints `EnergyStatus` JSON over USB CDC so the host can run `esp_bridge` and join the mesh.

## Quick start (once ESP toolchain is installed)

1. Install [espup](https://github.com/esp-rs/espup): `cargo install espup`
2. Install target: `espup install` (pick ESP32-S3 or C3 for native USB)
3. From hypha repo root: `cd firmware/hypha_esp`
4. Set target in `.cargo/config.toml`: `target = "xtensa-esp32s3-espidf"` or `riscv32imc-esp-espidf`
5. `source ~/export-esp.sh` (or the path espup gave)
6. `cargo build --release` then `cargo espflash flash --monitor` (or use ESP-IDF `idf.py flash monitor`)

## Without flashing (test bridge only)

From hypha repo root:

```bash
# Test bridge with stdin (no device)
echo '{"source_id":"esp-1","energy_score":0.85}' | cargo run --bin esp_bridge -- --stdin
```

With the USB device connected, run the bridge and see if the device already prints lines the bridge can parse; if not, flash this firmware.
