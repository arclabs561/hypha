#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d -t hypha-mesh-doctor.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/tailscale" <<'SH'
#!/usr/bin/env bash
if [[ ${1:-} == "status" ]]; then
  printf '100.71.158.25    dratini         tagged-devices              linux  -\n'
fi
SH
chmod 0755 "$TMP/tailscale"

cat >"$TMP/nc" <<'SH'
#!/usr/bin/env bash
exit 0
SH
chmod 0755 "$TMP/nc"

cat >"$TMP/curl" <<'SH'
#!/usr/bin/env bash
for arg in "$@"; do
  if [[ $arg == *firmware.bin.manifest.json ]]; then
    printf '{"v":"0.16.1","n":3,"h":"abcdef0123456789"}\n'
    exit 0
  fi
done
if [[ $* == *'%{http_code}'* ]]; then
  printf '200'
fi
SH
chmod 0755 "$TMP/curl"

cat >"$TMP/mosquitto_sub" <<'SH'
#!/usr/bin/env bash
cat <<'JSON'
hypha/hypha-good/health {"board":"hypha-good","fw":"0.16.1","boot":"goodboot","uptime_s":60,"power_source":"usb","wifi_rssi":-55,"peer_pulses":4,"led":"000000","led_state":"dark","mode":"auto","ota_state":"not_newer","loop_max_ms":50,"placement_state":"stable"}
hypha/hypha-old/health {"board":"hypha-old","fw":"0.16.0","uptime_s":70,"wifi_rssi":-80,"led":"000000","led_state":"dark","mode":"auto","loop_max_ms":55}
JSON
SH
chmod 0755 "$TMP/mosquitto_sub"

OUT="$(
  PATH="$TMP:$PATH" HYPHA_HEALTH_COUNT=2 bash "$ROOT/scripts/mesh_doctor.sh" 192.0.2.1 1883
)"

grep -q 'ok: manifest v=0.16.1' <<<"$OUT"
grep -Eq 'hypha-good.*healthy-dark' <<<"$OUT"
grep -Eq 'hypha-old.*fw-not-ota-version' <<<"$OUT"
grep -Eq 'hypha-old.*weak-wifi' <<<"$OUT"

printf 'mesh doctor ota health integration: ok\n'
