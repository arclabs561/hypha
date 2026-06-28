#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d -t hypha-locate.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/mosquitto_pub" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$HYPHA_TEST_PUB_ARGS"
SH
chmod 0755 "$TMP/mosquitto_pub"

LOCAL_OUT="$(
  HYPHA_TEST_PUB_ARGS="$TMP/local.args" \
    HYPHA_MQTT_HOST=broker.lan \
    HYPHA_MQTT_PORT=1884 \
    HYPHA_MQTT_USER=operator \
    HYPHA_MQTT_PASS=secret \
    PATH="$TMP:$PATH" \
    bash "$ROOT/scripts/hypha_locate.sh" on hypha-fc84 hypha-b4bc
)"

grep -q 'sent locate=true to hypha-fc84' <<<"$LOCAL_OUT"
grep -q 'sent locate=true to hypha-b4bc' <<<"$LOCAL_OUT"
grep -Fq -- '-h broker.lan -p 1884 -u operator -P secret -t hypha/hypha-fc84/cmd -m {"locate":true}' "$TMP/local.args"
grep -Fq -- '-t hypha/hypha-b4bc/cmd -m {"locate":true}' "$TMP/local.args"
if grep -q 'secret' <<<"$LOCAL_OUT"; then
  printf 'locate output leaked mqtt password\n' >&2
  exit 1
fi

rm -f "$TMP/mosquitto_pub"
cat >"$TMP/ssh" <<'SH'
#!/usr/bin/env bash
printf 'host=%s\ncmd=%s\n' "$1" "$2" >>"$HYPHA_TEST_SSH_ARGS"
SH
chmod 0755 "$TMP/ssh"

SSH_OUT="$(
  HYPHA_TEST_SSH_ARGS="$TMP/ssh.args" \
    HYPHA_MQTT_SSH_HOST=broker-host \
    HYPHA_MQTT_SSH_BROKER_HOST=broker.lan \
    HYPHA_MQTT_USER=operator \
    HYPHA_MQTT_PASS=secret \
    PATH="$TMP:/usr/bin:/bin" \
    bash "$ROOT/scripts/hypha_locate.sh" off hypha-2808
)"

grep -q 'sent locate=false to hypha-2808' <<<"$SSH_OUT"
grep -q '^host=broker-host$' "$TMP/ssh.args"
grep -Fq -- 'mosquitto_pub -h broker.lan -p 1883 -u operator -P secret -t hypha/hypha-2808/cmd -m \{\"locate\":false\}' "$TMP/ssh.args"
if grep -q 'secret' <<<"$SSH_OUT"; then
  printf 'ssh locate output leaked mqtt password\n' >&2
  exit 1
fi

if PATH="$TMP:$PATH" bash "$ROOT/scripts/hypha_locate.sh" on bad/topic 2>"$TMP/bad.err"; then
  printf 'expected invalid board id to fail\n' >&2
  exit 1
fi
grep -q 'invalid board id' "$TMP/bad.err"

printf 'hypha locate wrapper: ok\n'
