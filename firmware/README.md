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
  Retained health reports `boot`, configured `power_source`, `led_state`,
  `led`, `uptime_s`, `wifi_rssi`, `mqtt_reconnects`, `peer_pulses`, `ota_state`,
  `ota_checks`, `ota_failures`, and `placement_state`.

These are separate observations:

- WiFi connected: the board joined an AP.
- MQTT connected: the board can reach the broker and publish retained health.
- ESP-NOW peers: the board directly hears another ESP-NOW board on the current
  channel.
- Mesh delivery: a message reached a destination, possibly through relays. That
  must be measured by route, relay, or payload evidence, not by direct peer
  count alone.
- Placement fingerprint: on boot, the XIAO/IDF firmware scans visible WiFi
  AP BSSIDs/RSSI, compares against the previous boot's NVS-stored fingerprint,
  then reports `placement_state` (`no_baseline`, `stable`, `moved`,
  `inconclusive`, or an error). This is a self-observation, not a room label;
  room names belong in infra or the consuming application.

For XIAO/IDF boards, inspect retained health instead of reading the LED alone:

```bash
mosquitto_sub -v -t 'hypha/+/health' -C 4 | just hypha-health
```

If the local machine does not have `mosquitto_sub`, `just mesh-doctor` can read
through a host that does:

```bash
HYPHA_MQTT_SSH_HOST=<broker-ssh-host> HYPHA_MQTT_SSH_BROKER_HOST=<broker-lan-ip> just mesh-doctor
```

For brokers with credentials, set `HYPHA_MQTT_USER` and `HYPHA_MQTT_PASS`.

`healthy-dark` means the controllable LED is intentionally off in auto mode.
`seen` is the number of health samples observed for that board in the current
doctor run. `live-uptime-advanced` means multiple samples had the same boot ID
and the later sample reported higher `uptime_s`; that is a live-activity hint.
`uptime-not-advancing` means multiple samples were observed but the reported
uptime did not increase.
`freshness-unknown` means the retained payload came from legacy firmware that
does not report `uptime_s`; treat it as last-known state, not proof the board is
currently alive.
`power-source-unknown` means the firmware reports the field but the build did
not set a specific `POWER_SOURCE`.
`fw-not-ota-version` means `just mesh-doctor` found a signed OTA manifest and
the board's reported firmware version does not match it.
`legacy-no-ota-state` means the board did not report secure OTA decision
telemetry, so a blank OTA column should not be read as a successful check.
`no-mqtt-peer-pulses` means the board has not heard MQTT firefly pulses from
other boards; it is not the same thing as WiFi failure or direct ESP-NOW
isolation.
`rssi-read-errors` means the firmware could not read WiFi RSSI during at least
one health window.
`mqtt-reconnected` means the MQTT client reconnected after its first connection.
`cmd-ignored` means the board received at least one command that was invalid or
not addressed to that board.
`no-health-payloads` means the broker query returned no retained health payloads
to summarize.

`power_source` is build-time configured (`POWER_SOURCE=usb|mains|battery|...`),
not automatic battery detection. Automatic battery inference needs board-level
voltage/current sensing hardware or a declared power-path input.

## HTTP OTA signing

`hypha_esp_c6_idf` accepts HTTP OTA only when the image has a signed manifest
next to it. If the image URL is `.../firmware.bin`, the manifest URL is
`.../firmware.bin.manifest.json`.

After producing an IDF `firmware.bin`, sign it with:

```bash
just esp-c6-http-ota-sign /path/to/firmware.bin firmware/mesh_ota/keys/priv.pem
```

The signed manifest version must be strictly greater than the running firmware
version. The current IDF firmware version is `0.16.1`; boards reporting
`0.16.0` will accept a correctly signed `0.16.1` image.
