# Power measurement summaries

Commit sanitized ESP32-C6 power summaries here when a run supports a Hypha
power claim. Keep raw traces outside git if they contain private network names
or device identifiers.

Validate summaries before committing:

```sh
just power-measurement-validate docs/measurements/power/*.json
```

Required fields:

```json
{
  "board": "hypha-fc84",
  "firmware_sha": "8406186",
  "firmware_version": "0.16.1",
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

`energy_mj_per_observation` is the summary metric. Current by mode is supporting
evidence, not the claim by itself.
`publish_interval_s`, `sample_duration_s`, and `sample_rate_hz` must be positive.
`warmup_s` must be shorter than `sample_duration_s`. `led_max` is the firmware
brightness cap and must be in `0..255`.
