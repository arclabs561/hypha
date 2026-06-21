#!/usr/bin/env python3
"""Validate sanitized ESP32-C6 power measurement summaries."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any


REQUIRED_STRING_FIELDS = {
    "board",
    "firmware_sha",
    "firmware_version",
    "mode",
    "power_source",
    "measurement_device",
    "wifi_ap",
    "wifi_power_save",
    "mqtt_path",
    "led_mode",
    "ble_scan",
    "raw_trace",
    "notes",
}

REQUIRED_NONNEGATIVE_FIELDS = {
    "led_max",
    "warmup_s",
    "mean_current_ma",
    "p95_current_ma",
    "delivered_observations",
    "publish_failures",
    "energy_mj_per_observation",
}

REQUIRED_POSITIVE_FIELDS = {
    "publish_interval_s",
    "sample_duration_s",
    "sample_rate_hz",
}


def is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def validate_summary(path: Path) -> list[str]:
    errors: list[str] = []
    try:
        data = json.loads(path.read_text())
    except json.JSONDecodeError as exc:
        return [f"invalid json: {exc}"]

    if not isinstance(data, dict):
        return ["top-level value must be an object"]

    for field in sorted(REQUIRED_STRING_FIELDS):
        value = data.get(field)
        if not isinstance(value, str) or not value.strip():
            errors.append(f"{field}: required non-empty string")

    for field in sorted(REQUIRED_NONNEGATIVE_FIELDS):
        value = data.get(field)
        if not is_number(value) or value < 0:
            errors.append(f"{field}: required non-negative number")

    for field in sorted(REQUIRED_POSITIVE_FIELDS):
        value = data.get(field)
        if not is_number(value) or value <= 0:
            errors.append(f"{field}: required positive number")

    for field in ("rssi_min", "rssi_max"):
        value = data.get(field)
        if not is_number(value):
            errors.append(f"{field}: required number")

    if is_number(data.get("led_max")) and data["led_max"] > 255:
        errors.append("led_max: must be <= 255")

    if is_number(data.get("warmup_s")) and is_number(data.get("sample_duration_s")):
        if data["warmup_s"] >= data["sample_duration_s"]:
            errors.append("warmup_s: must be < sample_duration_s")

    if is_number(data.get("rssi_min")) and is_number(data.get("rssi_max")):
        if data["rssi_min"] > data["rssi_max"]:
            errors.append("rssi_min: must be <= rssi_max")

    if is_number(data.get("mean_current_ma")) and is_number(data.get("p95_current_ma")):
        if data["mean_current_ma"] > data["p95_current_ma"]:
            errors.append("mean_current_ma: must be <= p95_current_ma")

    delivered = data.get("delivered_observations")
    energy = data.get("energy_mj_per_observation")
    if is_number(delivered) and is_number(energy):
        if delivered == 0 and energy != 0:
            errors.append("energy_mj_per_observation: must be 0 when delivered_observations is 0")
        if delivered > 0 and energy <= 0:
            errors.append("energy_mj_per_observation: must be > 0 when delivered_observations is > 0")

    return errors


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print("usage: validate_power_measurement.py <summary.json> [...]", file=sys.stderr)
        return 2

    status = 0
    for raw_path in argv[1:]:
        path = Path(raw_path)
        errors = validate_summary(path)
        if errors:
            status = 1
            for error in errors:
                print(f"{path}: {error}", file=sys.stderr)
        else:
            print(f"{path}: ok")
    return status


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
