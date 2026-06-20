#!/usr/bin/env bash
# Summarize retained hypha/<board>/health MQTT payloads.
#
# Input can be raw JSON lines:
#   {"board":"esp-c6-fc84",...}
#
# Or mosquitto_sub -v lines:
#   hypha/esp-c6-fc84/health {"board":"esp-c6-fc84",...}
#
# Usage:
#   mosquitto_sub -v -t 'hypha/+/health' -C 4 | bash scripts/hypha_health_snapshot.sh
#   bash scripts/hypha_health_snapshot.sh /tmp/hypha-health.jsonl

set -euo pipefail

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

json_from_line() {
  local line=$1
  if [[ $line == \{* ]]; then
    printf '%s\n' "$line"
    return
  fi
  printf '%s\n' "${line#* }"
}

need_cmd jq

printf '%-18s %-7s %-9s %-6s %-5s %-6s %-12s %-6s %s\n' \
  board fw led_state mode rssi peers ota loop notes

status=0
while IFS= read -r line; do
  [[ -n $line ]] || continue
  json=$(json_from_line "$line")
  if ! row=$(jq -r '
    def n($k): (.[$k] // 0);
    def s($k): (.[$k] // "");
    def note:
      [
        (if s("led_state") == "dark" and s("mode") == "auto" and s("led") == "000000"
         then "healthy-dark" else empty end),
        (if s("led_state") == "fault" then "mqtt-bus-down-led" else empty end),
        (if n("peer_pulses") == 0 then "no-mqtt-peer-pulses" else empty end),
        (if n("wifi_rssi") < -75 then "weak-wifi" else empty end),
        (if n("loop_max_ms") > 250 then "loop-starved" else empty end),
        (if n("ota_failures") > 0 then "ota-failures" else empty end),
        (if (s("ota_state") | test("bad|mismatch|error")) then "ota-attention" else empty end)
      ] | if length == 0 then "ok" else join(",") end;
    [
      (s("board")),
      (s("fw")),
      (s("led_state")),
      (s("mode")),
      (n("wifi_rssi") | tostring),
      (n("peer_pulses") | tostring),
      (s("ota_state")),
      (n("loop_max_ms") | tostring),
      note
    ] | @tsv
  ' <<<"$json" 2>/dev/null); then
    printf 'warn: skipped malformed health line: %s\n' "$line" >&2
    status=1
    continue
  fi
  IFS=$'\t' read -r board fw led_state mode rssi peers ota loop notes <<<"$row"
  printf '%-18s %-7s %-9s %-6s %-5s %-6s %-12s %-6s %s\n' \
    "$board" "$fw" "$led_state" "$mode" "$rssi" "$peers" "$ota" "$loop" "$notes"
done < <(if [[ $# -gt 0 ]]; then cat "$@"; else cat; fi)

exit "$status"
