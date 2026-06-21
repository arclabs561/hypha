#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -t hypha-health.XXXXXX)"
trap 'rm -f "$TMP"' EXIT

cat >"$TMP" <<'JSON'
hypha/hypha-fc84/health {"board":"hypha-fc84","fw":"0.16.0","boot":"abc123ef","uptime_s":1234,"power_source":"usb","wifi_rssi":-62,"peer_pulses":3,"led":"000000","led_state":"dark","mode":"auto","ota_state":"not_newer","loop_max_ms":52,"placement_state":"moved","placement_aps":7,"placement_baseline_aps":6,"placement_common":2,"placement_shifted":2,"placement_jaccard_milli":250}
hypha/hypha-fc84/health {"board":"hypha-fc84","fw":"0.16.0","boot":"abc123ef","uptime_s":1300,"power_source":"usb","wifi_rssi":-61,"peer_pulses":5,"led":"000000","led_state":"dark","mode":"auto","ota_state":"not_newer","loop_max_ms":53,"placement_state":"moved","placement_aps":7,"placement_baseline_aps":6,"placement_common":2,"placement_shifted":2,"placement_jaccard_milli":250}
hypha/hypha-unknown/health {"board":"hypha-unknown","fw":"0.16.1","boot":"unkboot","uptime_s":120,"power_source":"unknown","wifi_rssi":-55,"peer_pulses":2,"led":"000000","led_state":"dark","mode":"auto","ota_state":"not_newer","loop_max_ms":42,"placement_state":"stable"}
hypha/hypha-old/health {"board":"hypha-old","fw":"0.16.0","wifi_rssi":-70,"led":"000000","led_state":"dark","mode":"auto","loop_max_ms":62}
JSON

OUT="$(HYPHA_EXPECTED_FW=0.16.1 bash "$ROOT/scripts/hypha_health_snapshot.sh" "$TMP")"
EMPTY_OUT="$(bash "$ROOT/scripts/hypha_health_snapshot.sh" /dev/null)"

grep -q 'boot' <<<"$OUT"
grep -q 'uptime' <<<"$OUT"
grep -q 'power' <<<"$OUT"
grep -q 'hypha-fc84' <<<"$OUT"
grep -q 'abc123ef' <<<"$OUT"
grep -q '1300' <<<"$OUT"
if [[ $(grep -c '^hypha-fc84' <<<"$OUT") -ne 1 ]]; then
  echo "expected duplicate board health rows to collapse to the latest payload" >&2
  exit 1
fi
grep -q 'usb' <<<"$OUT"
grep -q 'placement' <<<"$OUT"
grep -Eq 'hypha-fc84.*moved' <<<"$OUT"
grep -Eq 'hypha-fc84.*placement-moved' <<<"$OUT"
grep -Eq 'hypha-fc84.*fw-not-ota-version' <<<"$OUT"
grep -q 'healthy-dark' <<<"$OUT"
grep -Eq 'hypha-old.*healthy-dark' <<<"$OUT"
grep -Eq 'hypha-old.*legacy-no-boot-id' <<<"$OUT"
grep -Eq 'hypha-old.*freshness-unknown' <<<"$OUT"
grep -Eq 'hypha-old.*legacy-no-power-source' <<<"$OUT"
grep -Eq 'hypha-old.*legacy-no-peer-pulses-field' <<<"$OUT"
grep -Eq 'hypha-old.*legacy-no-ota-state' <<<"$OUT"
grep -Eq 'hypha-old.*legacy-no-placement' <<<"$OUT"
grep -Eq 'hypha-unknown.*power-source-unknown' <<<"$OUT"
grep -Eq '^none .*no-health-payloads' <<<"$EMPTY_OUT"

printf 'hypha-health snapshot parser: ok\n'
