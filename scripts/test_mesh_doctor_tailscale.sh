#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$(mktemp -t hypha-mesh-doctor-ts.XXXXXX)"
trap 'rm -f "$OUT"' EXIT

awk -f "$ROOT/scripts/mesh_doctor_tailscale.awk" >"$OUT" <<'STATUS'
100.99.153.36    dratini-initrd  henry@                      linux  offline, last seen 11d ago
100.71.158.25    dratini         tagged-devices              linux  idle, tx 23064 rx 24452
100.75.234.20    metagross       henry@                      macOS  -
STATUS

grep -q 'standby: dratini-initrd' "$OUT"
grep -q 'ok: no non-initrd offline peers reported' "$OUT"
grep -q 'note: initrd peers are fallback boot identities' "$OUT"
if grep -q 'offline: dratini-initrd' "$OUT"; then
  echo "initrd identity should not be reported as a live offline peer" >&2
  exit 1
fi

awk -f "$ROOT/scripts/mesh_doctor_tailscale.awk" >"$OUT" <<'STATUS'
100.64.0.1        arcanine        tagged-devices              linux  offline, last seen 2h ago
STATUS

grep -q 'offline: arcanine' "$OUT"

printf 'mesh doctor tailscale parser: ok\n'
