#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

OUT="$(
  bash "$ROOT/scripts/hypha_ble_peers_snapshot.sh" <<'EOF'
hypha/hypha-a/ble {"board":"hypha-a","boot":"a","seq":1,"window_ms":2000,"adverts":[{"peer":"hypha-b","r":-70},{"peer":"hypha-a","r":-1},{"a":"aa","r":-40}]}
hypha/hypha-a/ble {"board":"hypha-a","boot":"a","seq":2,"window_ms":2000,"adverts":[{"peer":"hypha-b","r":-65},{"peer":"hypha-c","r":-91}]}
hypha/hypha-b/ble {"boot":"b","seq":1,"window_ms":2000,"adverts":[{"peer":"hypha-a","r":-72}]}
EOF
)"

grep -q '^hypha-a[[:space:]]\+hypha-b[[:space:]]\+-65[[:space:]]\+2[[:space:]]\+direct$' <<<"$OUT"
grep -q '^hypha-a[[:space:]]\+hypha-c[[:space:]]\+-91[[:space:]]\+1[[:space:]]\+weak-direct-rssi$' <<<"$OUT"
grep -q '^hypha-b[[:space:]]\+hypha-a[[:space:]]\+-72[[:space:]]\+1[[:space:]]\+direct$' <<<"$OUT"
if grep -q '^hypha-a[[:space:]]\+hypha-a' <<<"$OUT"; then
  echo "self peer should be filtered" >&2
  exit 1
fi

EMPTY="$(bash "$ROOT/scripts/hypha_ble_peers_snapshot.sh" <<'EOF'
hypha/hypha-a/ble {"board":"hypha-a","adverts":[{"a":"aa","r":-40}]}
EOF
)"
grep -q 'no-direct-peer-adverts' <<<"$EMPTY"

EXPECTED="$(
  HYPHA_EXPECTED_BOARDS="hypha-a,hypha-b,hypha-d" \
    bash "$ROOT/scripts/hypha_ble_peers_snapshot.sh" <<'EOF'
hypha/hypha-a/ble {"board":"hypha-a","adverts":[{"peer":"hypha-b","r":-66}]}
EOF
)"
grep -q '^hypha-b[[:space:]]\+[[:space:]]\+[[:space:]]\+0[[:space:]]\+no-direct-out$' <<<"$EXPECTED"
grep -q '^none[[:space:]]\+hypha-a[[:space:]]\+[[:space:]]\+0[[:space:]]\+not-directly-heard$' <<<"$EXPECTED"
grep -q '^hypha-d[[:space:]]\+[[:space:]]\+[[:space:]]\+0[[:space:]]\+no-direct-out$' <<<"$EXPECTED"
grep -q '^none[[:space:]]\+hypha-d[[:space:]]\+[[:space:]]\+0[[:space:]]\+not-directly-heard$' <<<"$EXPECTED"

echo "hypha BLE peer snapshot parser: ok"
