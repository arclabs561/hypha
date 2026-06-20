---
status: proposal
scope: ESP32-C6 power measurement
grounded_in: ADR-0001, docs/design/hypha-refinement-roadmap.md, firmware/hypha_esp_c6_idf/**
review_trigger: revisit after the first bench capture lands, or before adding LP-core or sleep-mode firmware code.
---

# Design: ESP32-C6 Power Measurement

## Problem

Hypha cannot claim ESP32-C6 sleep, RX-gating, or sleeper-node power savings
from code shape alone. The current firmware can report health and synchronize
LED flashes, but the roadmap requires measured current or energy before any
sleep-mode design becomes a product claim.

## Grounding

Official ESP-IDF stable docs for ESP32-C6 are the baseline for capability
claims:

- Sleep modes: Light-sleep preserves CPU/RAM/peripheral state while reducing
  clocks and supply voltage; Deep-sleep powers off CPUs, most RAM, and APB
  digital peripherals, leaving the RTC controller, ULP coprocessor, and RTC
  FAST memory powered.
- Wi-Fi/Bluetooth: connections are not maintained in Light-sleep or Deep-sleep.
  Connection-preserving designs should use modem-sleep and automatic
  Light-sleep rather than treating Deep-sleep as connected.
- LP core: the C6 LP core can stay powered while the HP CPU is in low-power
  modes and can handle GPIO, sensor readings, LP I2C, and LP UART work before
  waking the HP CPU.
- Wi-Fi power save: `esp_wifi_set_ps()` sets the station power-save mode;
  `WIFI_PS_MAX_MODEM` uses the station `listen_interval` to determine beacon
  receive cadence.

References:

- https://docs.espressif.com/projects/esp-idf/en/stable/esp32c6/api-reference/system/sleep_modes.html
- https://docs.espressif.com/projects/esp-idf/en/stable/esp32c6/api-reference/system/ulp-lp-core.html
- https://docs.espressif.com/projects/esp-idf/en/stable/esp32c6/api-reference/network/esp_wifi.html

## Measurement Contract

Every power claim must name:

- board id and hardware variant;
- firmware git SHA and `CARGO_PKG_VERSION`;
- power source and measurement device;
- Wi-Fi AP, RSSI range, and whether 802.11 power save is enabled;
- MQTT broker path and publish interval;
- LED mode and `led_max`;
- BLE scan settings;
- sample duration, sample rate, and warm-up period;
- raw current trace path or summarized measurement file;
- delivered-observation count and failed-publish count.

The summary metric is energy per delivered observation. Current by mode is
supporting evidence, not the final claim.

## Initial Bench Matrix

Run the same flashed board through these states:

1. Baseline active: current firmware, MQTT connected, BLE scan on, LED auto,
   `led_max` default.
2. Dark baseline: same as baseline active, but `{"led":"off"}` and `led_max=0`
   except locate override.
3. TX-only pulse behavior: current firefly publish/couple behavior with no radio
   sleep change.
4. Wi-Fi modem-save candidate: `esp_wifi_set_ps(WIFI_PS_MAX_MODEM)` with a
   documented `listen_interval`; no Deep-sleep claim.
5. RX-gate candidate, if implemented: radio receive disabled or power-save mode
   changed only during a measured refractory window.
6. Deep-sleep sleeper spike, later: HP core wakes on timer or LP-core condition,
   publishes once, then returns to Deep-sleep.

Do not compare a Deep-sleep sleeper node against a continuously connected MQTT
node as if they provide the same service. They are different roles.

## Output File

Store each run as `docs/measurements/power/<date>-<board>-<mode>.json` or keep
it outside git if it contains private network identifiers. A sanitized summary
can be committed with this shape and checked with
`just power-measurement-validate`:

```json
{
  "board": "hypha-fc84",
  "firmware_sha": "<git sha>",
  "firmware_version": "0.16.0",
  "mode": "dark_baseline",
  "power_source": "usb",
  "measurement_device": "meter model or sanitized label",
  "wifi_ap": "sanitized ap label or hash",
  "rssi_min": -66,
  "rssi_max": -58,
  "wifi_power_save": "none",
  "mqtt_path": "broker label or sanitized path",
  "publish_interval_s": 30,
  "led_mode": "off",
  "led_max": 0,
  "ble_scan": "on",
  "sample_duration_s": 600,
  "sample_rate_hz": 1,
  "warmup_s": 60,
  "mean_current_ma": 0.0,
  "p95_current_ma": 0.0,
  "delivered_observations": 0,
  "publish_failures": 0,
  "energy_mj_per_observation": 0.0,
  "raw_trace": "external:/path-or-note",
  "notes": "sanitized"
}
```

## Gates

- No README or architecture claim may say a power feature saves energy until a
  measurement file shows the before/after delta on the same board.
- No RX-gating change may land without a latency note for urgent control or
  alert traffic.
- No LP-core sleeper implementation may land without an official-doc check for
  the exact wake source, peripheral, and memory domain it uses.
- If a measured mode reduces current but loses MQTT delivery, it is a different
  role, not an optimization of the same role.

## Non-goals

- Not implementing sleep or LP-core firmware in this document.
- Not selecting a battery, enclosure, or sensor.
- Not treating the outside transcript as a measurement.
- Not claiming Wi-Fi 6 TWT behavior until the actual AP and firmware path prove
  it.

## Next Action

Capture the baseline active and dark-baseline runs for `hypha-fc84`. Those two
runs separate LED cost from radio and scan cost before any sleep-mode work.
