#!/usr/bin/env bash
# Print comma-separated list of /dev/cu.usbmodem* devices (for --ports).
set -euo pipefail
first=1
for p in /dev/cu.usbmodem*; do
  [[ -e "$p" ]] || continue
  [[ $first -eq 0 ]] && echo -n ","
  echo -n "$p"
  first=0
done
[[ $first -eq 1 ]] && exit 1
echo
