#!/usr/bin/env bash
# Operator snapshot for the Hypha home mesh from charizard or another Mac.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BROKER_HOST="${1:-${HYPHA_MQTT_HOST:-192.168.1.9}}"
BROKER_PORT="${2:-${HYPHA_MQTT_PORT:-1883}}"
HEALTH_COUNT="${HYPHA_HEALTH_COUNT:-8}"
BLE_COUNT="${HYPHA_BLE_COUNT:-48}"
HEALTH_TIMEOUT="${HYPHA_HEALTH_TIMEOUT:-5}"
BLE_TIMEOUT="${HYPHA_BLE_TIMEOUT:-8}"
MQTT_SSH_HOST="${HYPHA_MQTT_SSH_HOST:-}"
MQTT_SSH_BROKER_HOST="${HYPHA_MQTT_SSH_BROKER_HOST:-localhost}"
MQTT_USER_VALUE="${HYPHA_MQTT_USER:-${MQTT_USER:-}}"
MQTT_PASS_VALUE="${HYPHA_MQTT_PASS:-${MQTT_PASS:-}}"
OTA_URL="${HYPHA_OTA_URL:-http://192.168.1.36:8930/fw/hypha/firmware.bin}"
EXPECTED_FW_VERSION=""
DOCTOR_STATUS=0
HEALTH_SUMMARY="$(mktemp -t hypha-health-summary.XXXXXX)"
BLE_SUMMARY="$(mktemp -t hypha-ble-summary.XXXXXX)"

cleanup() {
  rm -f "$HEALTH_SUMMARY" "$BLE_SUMMARY"
}
trap cleanup EXIT

have_cmd() {
  command -v "$1" >/dev/null 2>&1
}

section() {
  printf '\n%s\n' "$1"
}

run_checked() {
  local rc
  set +e
  "$@"
  rc=$?
  set -e
  if [[ $rc -ne 0 && $DOCTOR_STATUS -eq 0 ]]; then
    DOCTOR_STATUS=$rc
  fi
}

run_checked_capture() {
  local out_file=$1
  shift
  local rc
  set +e
  "$@" | tee "$out_file"
  rc=${PIPESTATUS[0]}
  set -e
  if [[ $rc -ne 0 && $DOCTOR_STATUS -eq 0 ]]; then
    DOCTOR_STATUS=$rc
  fi
}

has_live_health() {
  local board=$1
  grep -Eq "^${board}[[:space:]].*live-uptime-advanced" "$HEALTH_SUMMARY"
}

has_health_row() {
  local board=$1
  grep -Eq "^${board}[[:space:]]" "$HEALTH_SUMMARY"
}

has_no_live_health_row() {
  local board=$1
  grep -Eq "^${board}[[:space:]].*no-live-health-sample" "$HEALTH_SUMMARY"
}

has_missing_health_row() {
  local board=$1
  grep -Eq "^${board}[[:space:]].*missing-expected-health" "$HEALTH_SUMMARY"
}

has_direct_out() {
  local board=$1
  grep -Eq "^${board}[[:space:]]+[^[:space:]]+[[:space:]]+-?[0-9]+[[:space:]]+[0-9]+[[:space:]]+(direct|weak-direct-rssi)" "$BLE_SUMMARY"
}

has_direct_in() {
  local board=$1
  grep -Eq "^[^[:space:]]+[[:space:]]+${board}[[:space:]]+-?[0-9]+[[:space:]]+[0-9]+[[:space:]]+(direct|weak-direct-rssi)" "$BLE_SUMMARY" \
    || grep -Eq "^${board}[[:space:]].*heard-by=" "$BLE_SUMMARY"
}

health_state_for() {
  local board=$1
  if ! has_health_row "$board" || has_missing_health_row "$board"; then
    printf 'missing'
  elif has_live_health "$board"; then
    printf 'live'
  elif has_no_live_health_row "$board"; then
    printf 'retained-only'
  else
    printf 'single-sample'
  fi
}

bool_word() {
  if "$@"; then
    printf 'yes'
  else
    printf 'no'
  fi
}

visibility_action_for() {
  case "$1" in
    ok)
      printf 'none'
      ;;
    radio-visible-mqtt-stale)
      printf 'power-cycle-or-usb-log'
      ;;
    sample-window-too-short)
      printf 'run-visibility-check'
      ;;
    health-live-ble-out-missing)
      printf 'wait-or-power-cycle'
      ;;
    radio-isolated)
      printf 'check-power-range-or-usb'
      ;;
    *)
      printf 'inspect'
      ;;
  esac
}

correlate_expected_visibility() {
  [[ -n ${HYPHA_EXPECTED_BOARDS:-} ]] || return 0
  [[ -s $HEALTH_SUMMARY || -s $BLE_SUMMARY ]] || return 0

  section "correlated visibility"
  printf 'note: combines health freshness with direct BLE adjacency for expected boards\n'
  printf '%-18s %-13s %-10s %-9s %-27s %s\n' board health direct_out direct_in action hint

  local expected board health direct_out direct_in hint action
  expected="${HYPHA_EXPECTED_BOARDS//,/ }"
  for board in $expected; do
    [[ -n $board ]] || continue
    health="$(health_state_for "$board")"
    direct_out="$(bool_word has_direct_out "$board")"
    direct_in="$(bool_word has_direct_in "$board")"

    if [[ $health == "single-sample" ]]; then
      hint="sample-window-too-short"
    elif [[ $health != "live" && $direct_in == "yes" ]]; then
      hint="radio-visible-mqtt-stale"
    elif [[ $health == "live" && $direct_out == "no" ]]; then
      hint="health-live-ble-out-missing"
    elif [[ $direct_out == "no" && $direct_in == "no" ]]; then
      hint="radio-isolated"
    else
      hint="ok"
    fi

    action="$(visibility_action_for "$hint")"
    printf '%-18s %-13s %-10s %-9s %-27s %s\n' "$board" "$health" "$direct_out" "$direct_in" "$action" "$hint"
  done
}

local_mqtt_health() {
  local auth_args=()
  [[ -n $MQTT_USER_VALUE ]] && auth_args+=("-u" "$MQTT_USER_VALUE")
  [[ -n $MQTT_PASS_VALUE ]] && auth_args+=("-P" "$MQTT_PASS_VALUE")

  if have_cmd timeout; then
    { timeout "$HEALTH_TIMEOUT" mosquitto_sub -h "$BROKER_HOST" -p "$BROKER_PORT" "${auth_args[@]}" -v -t 'hypha/+/health' -C "$HEALTH_COUNT" || true; } \
      | HYPHA_EXPECTED_FW="$EXPECTED_FW_VERSION" bash "$ROOT/scripts/hypha_health_snapshot.sh"
  else
    mosquitto_sub -h "$BROKER_HOST" -p "$BROKER_PORT" "${auth_args[@]}" -v -t 'hypha/+/health' -C "$HEALTH_COUNT" \
      | HYPHA_EXPECTED_FW="$EXPECTED_FW_VERSION" bash "$ROOT/scripts/hypha_health_snapshot.sh"
  fi
}

ssh_mqtt_health() {
  local remote_cmd
  printf -v remote_cmd 'timeout %q mosquitto_sub -h %q -p %q' \
    "$HEALTH_TIMEOUT" "$MQTT_SSH_BROKER_HOST" "$BROKER_PORT"
  if [[ -n $MQTT_USER_VALUE ]]; then
    printf -v remote_cmd '%s -u %q' "$remote_cmd" "$MQTT_USER_VALUE"
  fi
  if [[ -n $MQTT_PASS_VALUE ]]; then
    printf -v remote_cmd '%s -P %q' "$remote_cmd" "$MQTT_PASS_VALUE"
  fi
  printf -v remote_cmd '%s -v -t %q -C %q || true' \
    "$remote_cmd" 'hypha/+/health' "$HEALTH_COUNT"
  ssh "$MQTT_SSH_HOST" "$remote_cmd" \
    | HYPHA_EXPECTED_FW="$EXPECTED_FW_VERSION" bash "$ROOT/scripts/hypha_health_snapshot.sh"
}

local_mqtt_ble_peers() {
  local auth_args=()
  [[ -n $MQTT_USER_VALUE ]] && auth_args+=("-u" "$MQTT_USER_VALUE")
  [[ -n $MQTT_PASS_VALUE ]] && auth_args+=("-P" "$MQTT_PASS_VALUE")

  if have_cmd timeout; then
    { timeout "$BLE_TIMEOUT" mosquitto_sub -h "$BROKER_HOST" -p "$BROKER_PORT" "${auth_args[@]}" -v -t 'hypha/+/ble' -C "$BLE_COUNT" || true; } \
      | bash "$ROOT/scripts/hypha_ble_peers_snapshot.sh"
  else
    mosquitto_sub -h "$BROKER_HOST" -p "$BROKER_PORT" "${auth_args[@]}" -v -t 'hypha/+/ble' -C "$BLE_COUNT" \
      | bash "$ROOT/scripts/hypha_ble_peers_snapshot.sh"
  fi
}

ssh_mqtt_ble_peers() {
  local remote_cmd
  printf -v remote_cmd 'timeout %q mosquitto_sub -h %q -p %q' \
    "$BLE_TIMEOUT" "$MQTT_SSH_BROKER_HOST" "$BROKER_PORT"
  if [[ -n $MQTT_USER_VALUE ]]; then
    printf -v remote_cmd '%s -u %q' "$remote_cmd" "$MQTT_USER_VALUE"
  fi
  if [[ -n $MQTT_PASS_VALUE ]]; then
    printf -v remote_cmd '%s -P %q' "$remote_cmd" "$MQTT_PASS_VALUE"
  fi
  printf -v remote_cmd '%s -v -t %q -C %q || true' \
    "$remote_cmd" 'hypha/+/ble' "$BLE_COUNT"
  ssh "$MQTT_SSH_HOST" "$remote_cmd" \
    | bash "$ROOT/scripts/hypha_ble_peers_snapshot.sh"
}

section "tailscale"
if have_cmd tailscale; then
  TS="$(tailscale status 2>&1 || true)"
  if [[ $TS == failed\ to\ connect* ]]; then
    printf 'unknown: tailscale status unavailable: %s\n' "$TS"
  else
    printf '%s\n' "$TS" | awk -f "$ROOT/scripts/mesh_doctor_tailscale.awk"
  fi
else
  printf 'skip: tailscale not installed\n'
fi

section "mqtt broker"
if have_cmd nc; then
  if nc -z -G 2 "$BROKER_HOST" "$BROKER_PORT" >/dev/null 2>&1; then
    printf 'ok: %s:%s reachable\n' "$BROKER_HOST" "$BROKER_PORT"
  else
    printf 'fail: %s:%s unreachable\n' "$BROKER_HOST" "$BROKER_PORT"
    if have_cmd arp; then
      arp_line="$(arp -an 2>/dev/null | awk -v host="$BROKER_HOST" '$0 ~ "\\(" host "\\)" {print; found=1} END {if (!found) exit 1}' || true)"
      if [[ -n $arp_line ]]; then
        printf 'arp: %s\n' "$arp_line"
      fi
    fi
  fi
else
  printf 'skip: nc not installed\n'
fi

section "ota server"
if ! have_cmd curl; then
  printf 'skip: curl not installed\n'
else
  image_code="$(curl -fsS -m 3 -o /dev/null -w '%{http_code}' "$OTA_URL" 2>/dev/null || true)"
  if [[ $image_code == 200 ]]; then
    printf 'ok: image reachable %s\n' "$OTA_URL"
  else
    printf 'fail: image not reachable %s (http=%s)\n' "$OTA_URL" "${image_code:-none}"
  fi

  manifest_url="${OTA_URL}.manifest.json"
  manifest_json="$(curl -fsS -m 3 "$manifest_url" 2>/dev/null || true)"
  if [[ -z $manifest_json ]]; then
    printf 'fail: signed manifest missing %s\n' "$manifest_url"
  elif have_cmd jq; then
    manifest_version="$(jq -r '.v // empty' <<<"$manifest_json" 2>/dev/null || true)"
    manifest_chunks="$(jq -r '.n // empty' <<<"$manifest_json" 2>/dev/null || true)"
    manifest_hash="$(jq -r '.h // empty' <<<"$manifest_json" 2>/dev/null || true)"
    if [[ -n $manifest_version && -n $manifest_chunks && -n $manifest_hash ]]; then
      EXPECTED_FW_VERSION="$manifest_version"
      printf 'ok: manifest v=%s chunks=%s hash=%s\n' \
        "$manifest_version" "$manifest_chunks" "${manifest_hash:0:12}"
    else
      printf 'fail: signed manifest malformed %s\n' "$manifest_url"
    fi
  else
    printf 'ok: manifest reachable %s\n' "$manifest_url"
  fi
fi

section "usb boards"
ports=()
for p in /dev/cu.usbmodem*; do
  [[ -e "$p" ]] && ports+=("$p")
done
if [[ ${#ports[@]} -eq 0 ]]; then
  printf 'none: no /dev/cu.usbmodem* data-USB boards visible\n'
else
  printf 'visible: %s\n' "${ports[*]}"
fi

section "mqtt health"
printf 'note: retained health is last-known state; live activity needs repeated samples with advancing uptime\n'
if ! have_cmd nc; then
  printf 'skip: nc not installed; cannot verify broker reachability\n'
elif ! nc -z -G 2 "$BROKER_HOST" "$BROKER_PORT" >/dev/null 2>&1; then
  printf 'skip: broker unreachable\n'
elif have_cmd mosquitto_sub; then
  run_checked_capture "$HEALTH_SUMMARY" local_mqtt_health
elif [[ -n $MQTT_SSH_HOST ]] && have_cmd ssh; then
  printf 'via ssh: %s broker=%s\n' "$MQTT_SSH_HOST" "$MQTT_SSH_BROKER_HOST"
  run_checked_capture "$HEALTH_SUMMARY" ssh_mqtt_health
else
  printf 'skip: mosquitto_sub not installed; set HYPHA_MQTT_SSH_HOST to query through the broker host\n'
fi

section "direct ble peers"
printf 'note: direct peer rows come from XIAO BLE adverts, not MQTT pulse counters\n'
if ! have_cmd nc; then
  printf 'skip: nc not installed; cannot verify broker reachability\n'
elif ! nc -z -G 2 "$BROKER_HOST" "$BROKER_PORT" >/dev/null 2>&1; then
  printf 'skip: broker unreachable\n'
elif have_cmd mosquitto_sub; then
  run_checked_capture "$BLE_SUMMARY" local_mqtt_ble_peers
elif [[ -n $MQTT_SSH_HOST ]] && have_cmd ssh; then
  printf 'via ssh: %s broker=%s\n' "$MQTT_SSH_HOST" "$MQTT_SSH_BROKER_HOST"
  run_checked_capture "$BLE_SUMMARY" ssh_mqtt_ble_peers
else
  printf 'skip: mosquitto_sub not installed; set HYPHA_MQTT_SSH_HOST to query through the broker host\n'
fi

correlate_expected_visibility

section "fleet power"
printf 'run: just fleet-power-doctor\n'
printf 'checks: boot history, abrupt previous boots, link-loss windows, UPS client presence\n'

exit "$DOCTOR_STATUS"
