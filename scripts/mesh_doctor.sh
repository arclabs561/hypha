#!/usr/bin/env bash
# Operator snapshot for the Hypha home mesh from charizard or another Mac.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BROKER_HOST="${1:-${HYPHA_MQTT_HOST:-192.168.1.9}}"
BROKER_PORT="${2:-${HYPHA_MQTT_PORT:-1883}}"
HEALTH_COUNT="${HYPHA_HEALTH_COUNT:-8}"

have_cmd() {
  command -v "$1" >/dev/null 2>&1
}

section() {
  printf '\n%s\n' "$1"
}

section "tailscale"
if have_cmd tailscale; then
  TS="$(tailscale status 2>&1 || true)"
  if [[ $TS == failed\ to\ connect* ]]; then
    printf 'unknown: tailscale status unavailable: %s\n' "$TS"
  else
    printf '%s\n' "$TS" | awk '
      /offline/ {
        state=$0
        sub(/^[[:space:]]*[0-9.]+[[:space:]]+/, "", state)
        print "offline: " state
        count++
      }
      END {
        if (count == 0) print "ok: no offline peers reported"
      }'
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
if ! have_cmd mosquitto_sub; then
  printf 'skip: mosquitto_sub not installed; feed retained payloads to: just hypha-health\n'
elif ! nc -z -G 2 "$BROKER_HOST" "$BROKER_PORT" >/dev/null 2>&1; then
  printf 'skip: broker unreachable\n'
else
  if have_cmd timeout; then
    timeout 5 mosquitto_sub -h "$BROKER_HOST" -p "$BROKER_PORT" -v -t 'hypha/+/health' -C "$HEALTH_COUNT" \
      | bash "$ROOT/scripts/hypha_health_snapshot.sh"
  else
    mosquitto_sub -h "$BROKER_HOST" -p "$BROKER_PORT" -v -t 'hypha/+/health' -C "$HEALTH_COUNT" \
      | bash "$ROOT/scripts/hypha_health_snapshot.sh"
  fi
fi

section "fleet power"
printf 'run: just fleet-power-doctor\n'
printf 'checks: boot history, abrupt previous boots, link-loss windows, UPS client presence\n'
