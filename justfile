default:
    @just --list

# Run bridge reading EnergyStatus from stdin (test without device)
esp-bridge-stdin:
    (printf '%s\n' '{"source_id":"esp-1","energy_score":0.85}'; sleep 5) | cargo run --bin esp_bridge -- --stdin

# Run bridge on USB serial (default: /dev/cu.usbmodem1101). Flash firmware first if device doesn't send JSON.
esp-bridge port="/dev/cu.usbmodem1101":
    port='{{port}}'; cargo run --bin esp_bridge -- --port "${port#port=}"

# List serial ports that may be ESP boards.
esp-c6-list-ports:
    bash scripts/esp_list_ports.sh

# Probe a connected ESP32-C6 before flashing. Confirm flash is 4MB+ for the OTA table.
esp-c6-board-info port="/dev/cu.usbmodem1101" before="usb-reset":
    port='{{port}}'; before='{{before}}'; cargo espflash board-info --chip esp32c6 --before "$before" --port "${port#port=}"

# Build ESP32-C6 IDF firmware. Required env: WIFI_SSID, WIFI_PASS, MQTT_HOST.
# Optional env: MQTT_PORT, MQTT_USER, MQTT_PASS, BOARD_ID, OTA_URL, POWER_SOURCE,
# OTA_PUBKEY_HEX or OTA_PUBKEY_PATH. OTA updates are skipped unless a pubkey is embedded.
esp-c6-build:
    cd firmware/hypha_esp_c6_idf && cargo build --release

# Flash one ESP32-C6 over USB with the dual-slot OTA partition table.
esp-c6-flash port="/dev/cu.usbmodem1101":
    port='{{port}}'; port="${port#port=}"; cargo espflash board-info --chip esp32c6 --before usb-reset --port "$port"; cd firmware/hypha_esp_c6_idf && cargo espflash flash --release --chip esp32c6 --before usb-reset --port "$port" --partition-table partitions_ota.csv --erase-parts otadata --monitor

# Flash every /dev/cu.usbmodem* ESP32-C6. Use without BOARD_ID so firmware derives unique IDs from MAC.
esp-c6-flash-all:
    bash -c 'set -euo pipefail; found=0; for port in /dev/cu.usbmodem*; do [[ -e "$port" ]] || continue; found=1; echo "probing $port"; cargo espflash board-info --chip esp32c6 --before usb-reset --port "$port"; echo "flashing $port"; (cd firmware/hypha_esp_c6_idf && cargo espflash flash --release --chip esp32c6 --before usb-reset --port "$port" --partition-table partitions_ota.csv --erase-parts otadata); done; [[ "$found" -eq 1 ]] || { echo "no /dev/cu.usbmodem* ports found" >&2; exit 1; }'

# Validate C6 serial output from connected boards.
esp-c6-validate-serial:
    bash scripts/validate_esp_serial.sh

# Build and sign the ESP-NOW mesh OTA application image.
mesh-ota-build version="0.1.0":
    mkdir -p firmware/hypha_esp_c6/build/mesh_ota
    cd firmware/hypha_esp_c6 && RUSTC_WRAPPER= cargo espflash save-image --release --chip esp32c6 --features led,mesh_ota build/mesh_ota/firmware.bin
    RUSTC_WRAPPER= cargo run --manifest-path firmware/mesh_ota/Cargo.toml -- --bin firmware/hypha_esp_c6/build/mesh_ota/firmware.bin --version "{{version}}" --key firmware/mesh_ota/keys/priv.pem --out-dir firmware/hypha_esp_c6/build/mesh_ota

# Verify the staged ESP-NOW mesh OTA manifest against the staged public key.
mesh-ota-verify:
    RUSTC_WRAPPER= cargo run --manifest-path firmware/mesh_ota/Cargo.toml -- --verify --manifest firmware/hypha_esp_c6/build/mesh_ota/manifest.json --pubkey firmware/hypha_esp_c6/build/mesh_ota/pubkey.hex

# Sign an ESP-IDF HTTP OTA image. Writes firmware.bin.manifest.json next to the image.
esp-c6-http-ota-sign bin key version="" out_dir="":
    bash scripts/sign_http_ota.sh "{{bin}}" "{{key}}" "{{version}}" "{{out_dir}}"

# Rebuild the ESP-NOW firmware with the staged OTA manifest/image embedded.
mesh-ota-firmware:
    cd firmware/hypha_esp_c6 && RUSTC_WRAPPER= MESH_OTA_PUBKEY_PATH=build/mesh_ota/pubkey.hex MESH_OTA_MANIFEST_PATH=build/mesh_ota/manifest.json MESH_OTA_IMAGE_PATH=build/mesh_ota/firmware.bin cargo build --release --features led,mesh_ota

# Validate the host bridge against connected boards.
esp-bridge-validate:
    bash scripts/validate_esp_bridge.sh

# Summarize retained MQTT health payloads from stdin or files.
hypha-health *args:
    bash scripts/hypha_health_snapshot.sh {{args}}

# Inspect Tailscale, broker reachability, USB boards, and retained MQTT health.
mesh-doctor broker="192.168.1.9" port="1883":
    bash scripts/mesh_doctor.sh "{{broker}}" "{{port}}"

# Inspect host boot history and link-loss evidence after a power event.
fleet-power-doctor:
    bash scripts/fleet_power_doctor.sh

# Ping Healthchecks.io with host, boot ID, and uptime. Set HEALTHCHECKS_URL.
healthchecks-ping mode="":
    bash scripts/healthchecks_ping.sh "{{mode}}"

# Validate sanitized ESP32-C6 power measurement summaries.
power-measurement-validate *paths:
    python3 scripts/validate_power_measurement.py {{paths}}

# Print a sanitized ESP32-C6 power measurement summary template.
power-measurement-template:
    @python3 scripts/validate_power_measurement.py --template

# Stream all C6 serial logs to /tmp/esp-debug.log.
esp-c6-debug:
    bash scripts/esp_debug_monitor.sh

# Stop local ESP monitors/bridges that may hold serial ports.
esp-c6-kill-ports:
    pkill -f 'espflash monitor.*esp32c6' 2>/dev/null || true
    pkill -f 'esp_bridge --ports' 2>/dev/null || true
    pkill -f 'esp_bridge --port' 2>/dev/null || true

check:
    cargo fmt --all -- --check
    cargo clippy --all-targets -- -D warnings
    cargo test
    cargo test --manifest-path firmware/host-tests/Cargo.toml
    bash -n scripts/mesh_doctor.sh scripts/sign_http_ota.sh scripts/healthchecks_ping.sh
    bash scripts/test_hypha_health_snapshot.sh
    bash scripts/test_mesh_doctor_ota_health.sh
    bash scripts/test_mesh_doctor_ssh_mqtt.sh
    bash scripts/test_mesh_doctor_tailscale.sh
    bash scripts/test_fleet_power_doctor.sh
    bash scripts/test_healthchecks_ping.sh
    bash scripts/test_power_measurement_validator.sh
    bash scripts/test_sign_http_ota.sh

test:
    cargo test

fmt:
    cargo fmt --all
