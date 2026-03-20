#!/usr/bin/env bash
# Validate that esp_bridge can read from all connected ESP32-C6 ports.
# Runs the bridge (multi-port, no dashboard) for a fixed time and asserts
# we see at least one "ESP energy update" per port (by counting updates).
#
# Usage:
#   bash scripts/validate_esp_bridge.sh                    # all /dev/cu.usbmodem*
#   bash scripts/validate_esp_bridge.sh /dev/cu.usbmodem111201 /dev/cu.usbmodem111301
#
# Requires: cargo, esp_bridge built. Devices should be running hypha firmware (serial JSON).
# Exit: 0 if bridge sees at least one energy update per port (or more from peers).

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_SEC="${VALIDATE_BRIDGE_RUN_SEC:-12}"

die() {
  echo "error: $*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

need_cmd cargo

if [[ $# -gt 0 ]]; then
  PORTS=("$@")
else
  PORTS=()
  for p in /dev/cu.usbmodem*; do
    [[ -e "$p" ]] && PORTS+=("$p")
  done
fi

if [[ ${#PORTS[@]} -eq 0 ]]; then
  die "no ports given and no /dev/cu.usbmodem* found"
fi

PORTS_STR=$(IFS=,; echo "${PORTS[*]}")
LOG=$(mktemp -t hypha_bridge_validate.XXXXXX.log)
trap 'rm -f "$LOG"' EXIT

# If run right after serial validation, ports may still be released by the OS.
sleep "${VALIDATE_BRIDGE_DELAY_SEC:-5}"
echo "validate_esp_bridge: ${#PORTS[@]} port(s), run bridge ${RUN_SEC}s"

# Run in process group so we can kill bridge and all children (no leftover processes).
( set -m; cd "$ROOT" && RUST_LOG=info cargo run --bin esp_bridge --quiet -- --ports "$PORTS_STR" 2>&1 ) > "$LOG" &
BPID=$!
sleep "$RUN_SEC"
kill -TERM -"$BPID" 2>/dev/null || kill "$BPID" 2>/dev/null || true
wait "$BPID" 2>/dev/null || true
pkill -f 'esp_bridge --ports' 2>/dev/null || true
sleep 1

COUNT=$(grep -c "ESP energy update" "$LOG" 2>/dev/null || echo 0)
COUNT=$(echo "$COUNT" | head -1 | tr -d '[:space:]')
COUNT=${COUNT:-0}
if grep -q "resource busy" "$LOG" 2>/dev/null && [[ "$COUNT" -eq 0 ]]; then
  echo "  Ports busy; run: just esp-c6-kill-ports && just esp-bridge-validate"
  exit 2
fi
if [[ "$COUNT" -ge "${#PORTS[@]}" ]]; then
  echo "  OK: $COUNT energy update(s) (need >= ${#PORTS[@]})"
else
  die "bridge saw $COUNT 'ESP energy update' lines, need >= ${#PORTS[@]} (ports: ${PORTS[*]})"
fi

echo "validate_esp_bridge: passed"
