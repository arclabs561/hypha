#!/usr/bin/env bash
# Operator snapshot for the Hypha home mesh from charizard or another Mac.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BROKER_HOST="${1:-${HYPHA_MQTT_HOST:-192.168.1.9}}"
BROKER_PORT="${2:-${HYPHA_MQTT_PORT:-1883}}"
HEALTH_COUNT="${HYPHA_HEALTH_COUNT:-8}"
MQTT_SSH_HOST="${HYPHA_MQTT_SSH_HOST:-}"
MQTT_SSH_BROKER_HOST="${HYPHA_MQTT_SSH_BROKER_HOST:-localhost}"
MQTT_USER_VALUE="${HYPHA_MQTT_USER:-${MQTT_USER:-}}"
MQTT_PASS_VALUE="${HYPHA_MQTT_PASS:-${MQTT_PASS:-}}"
OTA_URL="${HYPHA_OTA_URL:-http://192.168.1.36:8930/fw/hypha/firmware.bin}"
EXPECTED_FW_VERSION=""

have_cmd() {
  command -v "$1" >/dev/null 2>&1
}

section() {
  printf '\n%s\n' "$1"
}

local_mqtt_health() {
  local auth_args=()
  [[ -n $MQTT_USER_VALUE ]] && auth_args+=("-u" "$MQTT_USER_VALUE")
  [[ -n $MQTT_PASS_VALUE ]] && auth_args+=("-P" "$MQTT_PASS_VALUE")

  if have_cmd timeout; then
    { timeout 5 mosquitto_sub -h "$BROKER_HOST" -p "$BROKER_PORT" "${auth_args[@]}" -v -t 'hypha/+/health' -C "$HEALTH_COUNT" || true; } \
      | HYPHA_EXPECTED_FW="$EXPECTED_FW_VERSION" bash "$ROOT/scripts/hypha_health_snapshot.sh"
  else
    mosquitto_sub -h "$BROKER_HOST" -p "$BROKER_PORT" "${auth_args[@]}" -v -t 'hypha/+/health' -C "$HEALTH_COUNT" \
      | HYPHA_EXPECTED_FW="$EXPECTED_FW_VERSION" bash "$ROOT/scripts/hypha_health_snapshot.sh"
  fi
}

ssh_mqtt_health() {
  local remote_cmd
  printf -v remote_cmd 'timeout 5 mosquitto_sub -h %q -p %q' \
    "$MQTT_SSH_BROKER_HOST" "$BROKER_PORT"
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
  local_mqtt_health
elif [[ -n $MQTT_SSH_HOST ]] && have_cmd ssh; then
  printf 'via ssh: %s broker=%s\n' "$MQTT_SSH_HOST" "$MQTT_SSH_BROKER_HOST"
  ssh_mqtt_health
else
  printf 'skip: mosquitto_sub not installed; set HYPHA_MQTT_SSH_HOST to query through the broker host\n'
fi

section "fleet power"
printf 'run: just fleet-power-doctor\n'
printf 'checks: boot history, abrupt previous boots, link-loss windows, UPS client presence\n'
