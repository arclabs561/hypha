#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GOOD="$(mktemp -t hypha-power-good.XXXXXX.json)"
BAD="$(mktemp -t hypha-power-bad.XXXXXX.json)"
BAD_OUT="$(mktemp -t hypha-power-bad-out.XXXXXX)"
trap 'rm -f "$GOOD" "$BAD" "$BAD_OUT"' EXIT

cat >"$GOOD" <<'JSON'
{
  "board": "hypha-fc84",
  "firmware_sha": "8406186",
  "firmware_version": "0.16.1",
  "mode": "dark_baseline",
  "power_source": "usb",
  "measurement_device": "bench-meter",
  "wifi_ap": "sanitized",
  "rssi_min": -66,
  "rssi_max": -58,
  "wifi_power_save": "none",
  "mqtt_path": "sanitized-broker",
  "publish_interval_s": 30,
  "led_mode": "off",
  "led_max": 0,
  "ble_scan": "on",
  "sample_duration_s": 600,
  "sample_rate_hz": 1,
  "warmup_s": 60,
  "mean_current_ma": 12.4,
  "p95_current_ma": 18.2,
  "delivered_observations": 20,
  "publish_failures": 0,
  "energy_mj_per_observation": 22320.0,
  "raw_trace": "external:/tmp/private-trace.csv",
  "notes": "sanitized"
}
JSON

python3 "$ROOT/scripts/validate_power_measurement.py" "$GOOD" >/dev/null

cat >"$BAD" <<'JSON'
{
  "board": "",
  "firmware_sha": "8406186",
  "firmware_version": "0.16.1",
  "mode": "bad",
  "power_source": "usb",
  "measurement_device": "bench-meter",
  "wifi_ap": "sanitized",
  "rssi_min": -40,
  "rssi_max": -70,
  "wifi_power_save": "none",
  "mqtt_path": "sanitized-broker",
  "publish_interval_s": 30,
  "led_mode": "off",
  "led_max": 0,
  "ble_scan": "on",
  "sample_duration_s": 600,
  "sample_rate_hz": 1,
  "warmup_s": 60,
  "mean_current_ma": 20.0,
  "p95_current_ma": 10.0,
  "delivered_observations": 0,
  "publish_failures": 0,
  "energy_mj_per_observation": 1.0,
  "raw_trace": "external:/tmp/private-trace.csv",
  "notes": "sanitized"
}
JSON

if python3 "$ROOT/scripts/validate_power_measurement.py" "$BAD" >"$BAD_OUT" 2>&1; then
  echo "expected invalid power measurement to fail" >&2
  exit 1
fi

grep -q 'board: required non-empty string' "$BAD_OUT"
grep -q 'rssi_min: must be <= rssi_max' "$BAD_OUT"
grep -q 'mean_current_ma: must be <= p95_current_ma' "$BAD_OUT"
grep -q 'energy_mj_per_observation: must be 0' "$BAD_OUT"

printf 'power measurement validator: ok\n'
