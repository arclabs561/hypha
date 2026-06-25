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

payloads="$(mktemp -t hypha-health-payloads.XXXXXX)"
observed="$(mktemp -t hypha-health-observed.XXXXXX)"
live_observed="$(mktemp -t hypha-health-live.XXXXXX)"
cleanup() {
  rm -f "$payloads" "$observed" "$live_observed"
}
trap cleanup EXIT

status=0
while IFS= read -r line; do
  [[ -n $line ]] || continue
  topic=""
  if [[ $line != \{* ]]; then
    topic="${line%% *}"
  fi
  json=$(json_from_line "$line")
  if ! compact="$(
    jq -c --arg topic "$topic" '
      def topic_board:
        (try ($topic | capture("^hypha/(?<board>[^/]+)/health$").board) catch "");
      if ((.board // "") == "") and topic_board != ""
      then . + {board: topic_board}
      else .
      end
    ' <<<"$json" 2>/dev/null
  )"; then
    printf 'warn: skipped malformed health line: %s\n' "$line" >&2
    status=1
    continue
  fi
  printf '%s\n' "$compact" >>"$payloads"
  jq -r '.board // empty' <<<"$compact" >>"$observed"
done < <(if [[ $# -gt 0 ]]; then cat "$@"; else cat; fi)

printf '%-18s %-7s %-8s %-8s %-4s %-10s %-13s %-42s %-9s %-6s %-5s %-6s %-12s %-18s %-28s %-6s %s\n' \
  board fw boot uptime seen power placement place_evidence led_state mode rssi peers ota ota_counts tx_counts loop notes

if [[ -s $payloads ]]; then
  jq -s -r '
    def n($k): (.[$k] // 0);
    def s($k): (.[$k] // "");
    def place_evidence:
      if has("placement_aps") or has("placement_baseline_aps") or has("placement_common") or has("placement_shifted") or has("placement_jaccard_milli")
      then "aps=\(n("placement_aps"))/base=\(n("placement_baseline_aps"))/common=\(n("placement_common"))/shift=\(n("placement_shifted"))/j=\(n("placement_jaccard_milli"))"
      else ""
      end;
    def ota_counts:
      if has("ota_checks") or has("ota_failures")
      then "checks=\(n("ota_checks"))/fail=\(n("ota_failures"))"
      else ""
      end;
    def tx_counts:
      if has("ble_tx_ok") or has("ble_tx_fail") or has("health_tx_ok") or has("health_tx_fail") or has("pulse_tx_ok") or has("pulse_tx_fail")
      then "ble=\(n("ble_tx_ok"))/\(n("ble_tx_fail"));h=\(n("health_tx_ok"))/\(n("health_tx_fail"));p=\(n("pulse_tx_ok"))/\(n("pulse_tx_fail"))"
      else ""
      end;
    def note:
      [
        (if s("led_state") == "dark" and s("mode") == "auto" and s("led") == "000000"
         then "healthy-dark" else empty end),
        (if (env.HYPHA_EXPECTED_FW // "") != "" and s("fw") != "" and s("fw") != env.HYPHA_EXPECTED_FW
         then "fw-not-ota-version" else empty end),
        (if (env.HYPHA_EXPECTED_FW // "") != ""
            and s("fw") != ""
            and s("fw") != env.HYPHA_EXPECTED_FW
            and s("ota_state") == "not_newer"
         then "ota-not-newer-while-outdated" else empty end),
        (if s("led_state") == "fault" then "mqtt-bus-down-led" else empty end),
        (if has("boot") | not then "legacy-no-boot-id" else empty end),
        (if has("uptime_s") | not then "freshness-unknown" else empty end),
        (if n("_seen") > 1
            and has("uptime_s")
            and (._first_uptime | type == "number")
            and s("boot") == s("_first_boot")
            and n("uptime_s") > n("_first_uptime")
         then "live-uptime-advanced"
         elif n("_seen") > 1 and has("uptime_s")
         then "uptime-not-advancing"
         else empty end),
        (if has("power_source") | not then "legacy-no-power-source" else empty end),
        (if s("power_source") == "unknown" then "power-source-unknown" else empty end),
        (if has("peer_pulses") and n("peer_pulses") == 0
         then "no-mqtt-peer-pulses"
         elif has("peer_pulses") | not
         then "legacy-no-peer-pulses-field"
         else empty end),
        (if n("wifi_rssi") < -75 then "weak-wifi" else empty end),
        (if n("rssi_err") > 0 then "rssi-read-errors" else empty end),
        (if n("mqtt_reconnects") > 0 then "mqtt-reconnected" else empty end),
        (if n("cmd_ignored") > 0 then "cmd-ignored" else empty end),
        (if n("loop_max_ms") > 250 then "loop-starved" else empty end),
        (if n("ota_failures") > 0 then "ota-failures" else empty end),
        (if n("ble_tx_fail") > 0 then "ble-publish-failures" else empty end),
        (if n("health_tx_fail") > 0 then "health-publish-failures" else empty end),
        (if n("pulse_tx_fail") > 0 then "pulse-publish-failures" else empty end),
        (if has("ota_state") | not then "legacy-no-ota-state" else empty end),
        (if (s("ota_state") | test("bad|mismatch|error")) then "ota-attention" else empty end),
        (if s("placement_state") == "moved" then "placement-moved"
         elif s("placement_state") == "inconclusive" then "placement-inconclusive"
         elif (s("placement_state") | test("error$")) then "placement-attention"
         else empty end),
        (if has("placement_state") | not then "legacy-no-placement" else empty end)
      ] | if length == 0 then "ok" else join(",") end;
    def live:
      n("_seen") > 1
      and has("uptime_s")
      and (._first_uptime | type == "number")
      and s("boot") == s("_first_boot")
      and n("uptime_s") > n("_first_uptime");
    def row:
      [
        (s("board")),
        (s("fw")),
        (s("boot")),
        (if has("uptime_s") then (n("uptime_s") | tostring) else "" end),
        (n("_seen") | tostring),
        (s("power_source")),
        (s("placement_state")),
        place_evidence,
        (s("led_state")),
        (s("mode")),
        (if has("wifi_rssi") then (n("wifi_rssi") | tostring) else "" end),
        (if has("peer_pulses") then (n("peer_pulses") | tostring) else "" end),
        (s("ota_state")),
        ota_counts,
        tx_counts,
        (if has("loop_max_ms") then (n("loop_max_ms") | tostring) else "" end),
        note,
        (if live then "1" else "0" end)
      ] | join("\u001f");
    sort_by(.board // "")
    | group_by(.board // "")
    | .[]
    | . as $group
    | ($group[-1] + {
        _seen: ($group | length),
        _first_uptime: ($group[0].uptime_s // null),
        _first_boot: ($group[0].boot // "")
      })
    | row
  ' "$payloads" | while IFS=$'\037' read -r board fw boot uptime seen power placement place_evidence led_state mode rssi peers ota ota_counts tx_counts loop notes live; do
    printf '%-18s %-7s %-8s %-8s %-4s %-10s %-13s %-42s %-9s %-6s %-5s %-6s %-12s %-18s %-28s %-6s %s\n' \
      "$board" "$fw" "$boot" "$uptime" "$seen" "$power" "$placement" "$place_evidence" "$led_state" "$mode" "$rssi" "$peers" "$ota" "$ota_counts" "$tx_counts" "$loop" "$notes"
    if [[ $live == "1" ]]; then
      printf '%s\n' "$board" >>"$live_observed"
    fi
  done
else
  printf '%-18s %-7s %-8s %-8s %-4s %-10s %-13s %-42s %-9s %-6s %-5s %-6s %-12s %-18s %-28s %-6s %s\n' \
    none "" "" "" "" "" "" "" "" "" "" "" "" "" "" "" "no-health-payloads"
fi

if [[ -n ${HYPHA_EXPECTED_BOARDS:-} ]]; then
  expected="${HYPHA_EXPECTED_BOARDS//,/ }"
  for board in $expected; do
    [[ -n $board ]] || continue
    if ! grep -Fxq "$board" "$observed"; then
      printf '%-18s %-7s %-8s %-8s %-4s %-10s %-13s %-42s %-9s %-6s %-5s %-6s %-12s %-18s %-28s %-6s %s\n' \
        "$board" "" "" "" "0" "" "" "" "" "" "" "" "" "" "" "" "missing-expected-health"
      [[ -n ${HYPHA_REQUIRE_LIVE:-} ]] && status=2
    elif [[ -n ${HYPHA_REQUIRE_LIVE:-} ]] && ! grep -Fxq "$board" "$live_observed"; then
      printf '%-18s %-7s %-8s %-8s %-4s %-10s %-13s %-42s %-9s %-6s %-5s %-6s %-12s %-18s %-28s %-6s %s\n' \
        "$board" "" "" "" "0" "" "" "" "" "" "" "" "" "" "" "" "no-live-health-sample"
      status=2
    fi
  done
fi

exit "$status"
