#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -t hypha-health.XXXXXX)"
trap 'rm -f "$TMP"' EXIT

cat >"$TMP" <<'JSON'
hypha/hypha-fc84/health {"board":"hypha-fc84","fw":"0.16.0","boot":"abc123ef","power_source":"usb","wifi_rssi":-62,"peer_pulses":3,"led":"000000","led_state":"dark","mode":"auto","ota_state":"not_newer","loop_max_ms":52}
JSON

OUT="$(bash "$ROOT/scripts/hypha_health_snapshot.sh" "$TMP")"

grep -q 'boot' <<<"$OUT"
grep -q 'power' <<<"$OUT"
grep -q 'hypha-fc84' <<<"$OUT"
grep -q 'abc123ef' <<<"$OUT"
grep -q 'usb' <<<"$OUT"
grep -q 'healthy-dark' <<<"$OUT"

printf 'hypha-health snapshot parser: ok\n'
