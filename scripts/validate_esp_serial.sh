#!/usr/bin/env bash
# Validate ESP32-C6 devices by capturing serial output and asserting on expected lines.
# No manual checks: runs espflash monitor briefly per port, greps for boot + JSON.
#
# Usage:
#   bash scripts/validate_esp_serial.sh              # all /dev/cu.usbmodem*
#   bash scripts/validate_esp_serial.sh /dev/cu.usbmodem111201 /dev/cu.usbmodem111301
#
# Requires: espflash, firmware built (just esp-c6-build-led or esp-c6-flash-all).
# Skips ports that cannot be opened. Exit: 0 if at least one port passes (WIRELESS_UP + JSON) and none fail.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIRMWARE_DIR="$ROOT/firmware/hypha_esp_c6"
CAPTURE_SEC="${VALIDATE_ESP_CAPTURE_SEC:-14}"

die() {
  echo "error: $*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

need_cmd espflash

# Ports: from args or discover all usbmodem
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

echo "validate_esp_serial: ${#PORTS[@]} port(s), capture ${CAPTURE_SEC}s each"

PASSED=0
FAILED=0
ANY_RX=0
declare -a FAIL_REASONS

for port in "${PORTS[@]}"; do
  if [[ ! -e "$port" ]]; then
    FAIL_REASONS+=("$port: device not present")
    ((FAILED++)) || true
    continue
  fi

  LOG=$(mktemp -t hypha_esp_validate.XXXXXX.log)

  ( set -m; cd "$FIRMWARE_DIR" && espflash monitor --port "$port" --chip esp32c6 --non-interactive 2>&1 | tee "$LOG" ) &
  MPID=$!
  sleep "$CAPTURE_SEC"
  kill -TERM -"$MPID" 2>/dev/null || kill "$MPID" 2>/dev/null || true
  wait "$MPID" 2>/dev/null || true
  pkill -f "espflash monitor --port $port" 2>/dev/null || true
  sleep 1

  if grep -q "Failed to open serial port\|Error while connecting" "$LOG" 2>/dev/null; then
    echo "  SKIP $port (could not open; run just esp-c6-kill-ports if busy)"
    rm -f "$LOG"
    continue
  fi

  HAS_WIRELESS_UP=
  HAS_JSON=
  HAS_HEARTBEAT=
  HAS_NEW_FIELDS=
  if grep -q "WIRELESS_UP" "$LOG" 2>/dev/null; then
    HAS_WIRELESS_UP=1
  fi
  if grep -q '"source_id"' "$LOG" 2>/dev/null && grep -q '"energy_score"' "$LOG" 2>/dev/null; then
    HAS_JSON=1
  fi
  if grep -q "HEARTBEAT" "$LOG" 2>/dev/null; then
    HAS_HEARTBEAT=1
  fi
  if grep -q '"uptime_ms"' "$LOG" 2>/dev/null && grep -q '"tx_ok"' "$LOG" 2>/dev/null; then
    HAS_NEW_FIELDS=1
  fi
  if grep -qE 'RX .* rssi=' "$LOG" 2>/dev/null; then
    ANY_RX=1
  fi

  if [[ -n "$HAS_WIRELESS_UP" && -n "$HAS_JSON" ]]; then
    extra=""
    [[ -n "$HAS_HEARTBEAT" ]] && extra=" + HEARTBEAT"
    [[ -n "$HAS_NEW_FIELDS" ]] && extra="${extra} + uptime/tx_ok"
    echo "  OK $port (WIRELESS_UP + JSON${extra})"
    ((PASSED++)) || true
  else
    [[ -z "$HAS_WIRELESS_UP" ]] && FAIL_REASONS+=("$port: no WIRELESS_UP in log")
    [[ -z "$HAS_JSON" ]] && FAIL_REASONS+=("$port: no EnergyStatus JSON in log")
    ((FAILED++)) || true
    echo "  FAIL $port"
  fi
  rm -f "$LOG"
done

if [[ $PASSED -eq 0 ]]; then
  for r in "${FAIL_REASONS[@]}"; do
    echo "  $r" >&2
  done
  die "validation failed: no port passed (run just esp-c6-kill-ports if ports busy)"
fi

if [[ $FAILED -gt 0 ]]; then
  for r in "${FAIL_REASONS[@]}"; do
    echo "  $r" >&2
  done
  die "validation failed: $PASSED passed, $FAILED failed"
fi

if [[ $PASSED -ge 2 && "$ANY_RX" -eq 0 ]]; then
  die "multi-device: no peer RX (rssi=) seen on any port; ESP-NOW between devices may not be working"
fi

# Leave no serial validation processes behind (in case process-group kill missed a child).
pkill -f 'espflash monitor.*esp32c6' 2>/dev/null || true
sleep 1
echo "validate_esp_serial: all $PASSED port(s) passed"
echo "LED introspection: dim red = alone (no peers); Y/G/B = 1/2/3+ peers; boot identity color = match board to source_id on serial."
