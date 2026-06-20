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

## Connectivity Signals

Hypha firmware currently has two C6 lines with different transport meanings:

- `hypha_esp_c6` (older ESP-NOW/firefly boards): `peers` means recently heard
  one-hop ESP-NOW source MACs. It is direct RF visibility only. A warm
  red-orange firefly pulse is the isolated hue (`peer_count == 0`), with
  brightness still modulated by energy and firefly phase.
- `hypha_esp_c6_idf` (XIAO/MQTT boards): WiFi STA connects to the AP, MQTT
  connects to the broker, and board-to-board firefly pulses go through MQTT. An
  amber slow breath means the MQTT bus has been unreachable for more than 120 s.
  Retained health reports `led_state`, `led`, `wifi_rssi`, `mqtt_reconnects`,
  `peer_pulses`, `ota_state`, `ota_checks`, and `ota_failures`.

These are separate observations:

- WiFi connected: the board joined an AP.
- MQTT connected: the board can reach the broker and publish retained health.
- ESP-NOW peers: the board directly hears another ESP-NOW board on the current
  channel.
- Mesh delivery: a message reached a destination, possibly through relays. That
  must be measured by route, relay, or payload evidence, not by direct peer
  count alone.
- Placement fingerprint: RSSI and neighbor sets can suggest a room move, but
  the firmware should report observations; room labels belong in infra or the
  consuming application.

For XIAO/IDF boards, inspect retained health instead of reading the LED alone:

```bash
mosquitto_sub -v -t 'hypha/+/health' -C 4 | just hypha-health
```

`healthy-dark` means the controllable LED is intentionally off in auto mode.
`no-mqtt-peer-pulses` means the board has not heard MQTT firefly pulses from
other boards; it is not the same thing as WiFi failure or direct ESP-NOW
isolation.
