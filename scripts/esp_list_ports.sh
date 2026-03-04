#!/usr/bin/env bash
# List serial ports that might be ESP32-C6 (for flashing when 3rd device has different name).
# Use: just esp-c6-flash-all (uses usbmodem* only); if you have 3 boards but only 2 usbmodem,
# run this and flash the 3rd manually: just esp-c6-flash port=/dev/cu.XXXX
set -euo pipefail
echo "Usbmodem (used by esp-c6-flash-all):"
for p in /dev/cu.usbmodem*; do
  [[ -e "$p" ]] && echo "  $p"
done
echo "Other serial (if 3rd ESP appears here, flash with: just esp-c6-flash port=<path>):"
for p in /dev/cu.*; do
  [[ -e "$p" ]] || continue
  [[ "$p" == *usbmodem* ]] && continue
  [[ "$p" == *Bluetooth* ]] && continue
  echo "  $p"
done
