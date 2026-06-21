#!/usr/bin/env bash
# Send a host heartbeat to a pre-rendered Healthchecks.io ping URL.

set -euo pipefail

MODE="${1:-}"
PING_URL="${HEALTHCHECKS_URL:-}"
PROC_ROOT="${HYPHA_PROC_ROOT:-/proc}"

if [[ -z $PING_URL ]]; then
  printf 'error: HEALTHCHECKS_URL is required\n' >&2
  exit 2
fi

case "$MODE" in
  ""|"start"|"fail")
    ;;
  *)
    printf 'error: mode must be one of: start, fail\n' >&2
    exit 2
    ;;
esac

if [[ -n $MODE ]]; then
  PING_URL="${PING_URL%/}/$MODE"
fi

host="$(hostname 2>/dev/null || uname -n)"
os="$(uname -s)"
boot_id="unknown"
uptime_s="unknown"

if [[ -r "$PROC_ROOT/sys/kernel/random/boot_id" ]]; then
  boot_id="$(cat "$PROC_ROOT/sys/kernel/random/boot_id")"
fi

if [[ -r "$PROC_ROOT/uptime" ]]; then
  uptime_s="$(cut -d. -f1 "$PROC_ROOT/uptime")"
elif [[ $os == Darwin ]]; then
  boot_epoch="$(
    sysctl -n kern.boottime 2>/dev/null \
      | sed -n 's/^{ sec = \([0-9][0-9]*\),.*/\1/p'
  )"
  now_epoch="$(date +%s)"
  if [[ -n $boot_epoch ]]; then
    uptime_s="$((now_epoch - boot_epoch))"
  fi
fi

payload="$(printf 'host=%s\nos=%s\nboot_id=%s\nuptime_s=%s\n' "$host" "$os" "$boot_id" "$uptime_s")"

curl -fsS --retry 2 --max-time 10 --data-binary "$payload" "$PING_URL" >/dev/null
if [[ -n $MODE ]]; then
  printf 'ok: pinged %s for %s\n' "$MODE" "$host"
else
  printf 'ok: pinged %s\n' "$host"
fi
