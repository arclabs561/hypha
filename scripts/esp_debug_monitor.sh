#!/usr/bin/env bash
# Stream serial from all connected ESP32-C6 boards to one timestamped log; tail -f so you can debug live.
# Run: just esp-c6-debug   or   bash scripts/esp_debug_monitor.sh
# Log: ESP_DEBUG_LOG or /tmp/esp-debug.log. Ctrl+C stops monitors and tail.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="${ESP_DEBUG_LOG:-/tmp/esp-debug.log}"
cd "$ROOT"

# Free serial ports
pkill -f 'espflash monitor.*esp32c6' 2>/dev/null || true
pkill -f 'esp_bridge --ports' 2>/dev/null || true
pkill -f 'esp_bridge --port' 2>/dev/null || true
sleep 2

PORTS=()
for p in /dev/cu.usbmodem*; do
  [[ -e "$p" ]] && PORTS+=("$p")
done

if [[ ${#PORTS[@]} -eq 0 ]]; then
  echo "No /dev/cu.usbmodem* found. Plug in boards and re-run." >&2
  exit 1
fi

: > "$LOG"
PIDS=()
for p in "${PORTS[@]}"; do
  name=$(basename "$p")
  (
    espflash monitor --port "$p" --chip esp32c6 2>&1 | while IFS= read -r line; do
      echo "[$(date +%H:%M:%S)] [$name] $line" >> "$LOG"
    done
  ) &
  PIDS+=($!)
done

trap 'kill "${PIDS[@]}" 2>/dev/null; exit' INT TERM
echo "Log: $LOG  (${#PORTS[@]} port(s)). Ctrl+C to stop."
tail -f "$LOG"
