#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d -t hypha-mesh-doctor-ssh.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

jq_path="$(command -v jq)"
ln -s "$jq_path" "$TMP/jq"

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

cat >"$TMP/ssh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

if [[ ${1:-} != "broker-host" ]]; then
  printf 'unexpected ssh host: %s\n' "${1:-}" >&2
  exit 2
fi

remote_cmd="${2:-}"
if [[ $remote_cmd == *"mosquitto_sub"* \
  && $remote_cmd == *"broker.lan"* \
  && $remote_cmd == *"-u operator"* \
  && $remote_cmd == *"-P secret"* \
  && $remote_cmd == *"hypha/+/health"* ]]; then
  cat <<'JSON'
hypha/hypha-remote/health {"board":"hypha-remote","fw":"0.16.1","boot":"remote","uptime_s":33,"power_source":"usb","wifi_rssi":-50,"peer_pulses":2,"led":"000000","led_state":"dark","mode":"auto","ota_state":"not_newer","loop_max_ms":40,"placement_state":"stable"}
JSON
else
  printf 'unexpected remote command: %s\n' "$remote_cmd" >&2
  exit 2
fi
SH
chmod 0755 "$TMP/ssh"

OUT="$(
  PATH="$TMP:/usr/bin:/bin" \
    HYPHA_HEALTH_COUNT=1 \
    HYPHA_MQTT_SSH_HOST=broker-host \
    HYPHA_MQTT_SSH_BROKER_HOST=broker.lan \
    HYPHA_MQTT_USER=operator \
    HYPHA_MQTT_PASS=secret \
    bash "$ROOT/scripts/mesh_doctor.sh" 192.0.2.1 1883
)"

grep -q 'via ssh: broker-host broker=broker.lan' <<<"$OUT"
if grep -q 'secret' <<<"$OUT"; then
  printf 'mesh doctor output leaked mqtt password\n' >&2
  exit 1
fi
grep -Eq 'hypha-remote.*healthy-dark' <<<"$OUT"
grep -Eq 'hypha-remote.*not_newer.*healthy-dark' <<<"$OUT"

printf 'mesh doctor ssh mqtt fallback: ok\n'
