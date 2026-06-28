#!/usr/bin/env bash
# Send momentary Hypha locate commands to one or more boards.

set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
usage: hypha_locate.sh on|off <board> [board...]

Environment:
  HYPHA_MQTT_HOST / HYPHA_MQTT_PORT       broker for local mosquitto_pub
  HYPHA_MQTT_SSH_HOST                     host with mosquitto_pub installed
  HYPHA_MQTT_SSH_BROKER_HOST              broker address from SSH host
  HYPHA_MQTT_USER or MQTT_USER            optional MQTT username
  HYPHA_MQTT_PASS or MQTT_PASS            optional MQTT password

Use with op interactively, for example:
  HYPHA_MQTT_PASS=op://... op run -- just hypha-locate on hypha-fc84
USAGE
  exit 2
}

have_cmd() {
  command -v "$1" >/dev/null 2>&1
}

json_bool_for() {
  case "$1" in
    on | true | 1 | yes)
      printf 'true'
      ;;
    off | false | 0 | no)
      printf 'false'
      ;;
    *)
      usage
      ;;
  esac
}

valid_board() {
  [[ $1 =~ ^hypha-[[:alnum:]_-]+$ ]]
}

publish_local() {
  local board=$1
  local payload=$2
  local auth_args=()
  [[ -n $MQTT_USER_VALUE ]] && auth_args+=("-u" "$MQTT_USER_VALUE")
  [[ -n $MQTT_PASS_VALUE ]] && auth_args+=("-P" "$MQTT_PASS_VALUE")
  mosquitto_pub \
    -h "$BROKER_HOST" \
    -p "$BROKER_PORT" \
    "${auth_args[@]}" \
    -t "hypha/${board}/cmd" \
    -m "$payload"
}

publish_ssh() {
  local board=$1
  local payload=$2
  local remote_cmd
  printf -v remote_cmd 'mosquitto_pub -h %q -p %q' \
    "$MQTT_SSH_BROKER_HOST" "$BROKER_PORT"
  if [[ -n $MQTT_USER_VALUE ]]; then
    printf -v remote_cmd '%s -u %q' "$remote_cmd" "$MQTT_USER_VALUE"
  fi
  if [[ -n $MQTT_PASS_VALUE ]]; then
    printf -v remote_cmd '%s -P %q' "$remote_cmd" "$MQTT_PASS_VALUE"
  fi
  printf -v remote_cmd '%s -t %q -m %q' \
    "$remote_cmd" "hypha/${board}/cmd" "$payload"
  ssh "$MQTT_SSH_HOST" "$remote_cmd"
}

[[ $# -ge 2 ]] || usage

MODE="$(json_bool_for "$1")"
shift

BROKER_HOST="${HYPHA_MQTT_HOST:-192.168.1.9}"
BROKER_PORT="${HYPHA_MQTT_PORT:-1883}"
MQTT_SSH_HOST="${HYPHA_MQTT_SSH_HOST:-}"
MQTT_SSH_BROKER_HOST="${HYPHA_MQTT_SSH_BROKER_HOST:-localhost}"
MQTT_USER_VALUE="${HYPHA_MQTT_USER:-${MQTT_USER:-}}"
MQTT_PASS_VALUE="${HYPHA_MQTT_PASS:-${MQTT_PASS:-}}"
PAYLOAD="{\"locate\":${MODE}}"

for board in "$@"; do
  if ! valid_board "$board"; then
    printf 'error: invalid board id: %s\n' "$board" >&2
    exit 2
  fi
done

for board in "$@"; do
  if have_cmd mosquitto_pub; then
    publish_local "$board" "$PAYLOAD"
  elif [[ -n $MQTT_SSH_HOST ]] && have_cmd ssh; then
    publish_ssh "$board" "$PAYLOAD"
  else
    printf 'error: mosquitto_pub not installed; set HYPHA_MQTT_SSH_HOST to publish through the broker host\n' >&2
    exit 1
  fi
  printf 'sent locate=%s to %s\n' "$MODE" "$board"
done
