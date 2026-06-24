#!/usr/bin/env bash
# Inspect host boot history after a suspected power event.

set -euo pipefail

HOSTS="${HYPHA_FLEET_HOSTS:-arcanine omastar starmie dratini metagross snorlax}"
SSH_TIMEOUT="${HYPHA_SSH_TIMEOUT:-5}"
TAILSCALE_STATUS="${HYPHA_TAILSCALE_STATUS:-}"

section() {
  printf '\n== %s ==\n' "$1"
}

tailscale_status() {
  if [[ -n $TAILSCALE_STATUS ]]; then
    printf '%s\n' "$TAILSCALE_STATUS"
  elif command -v tailscale >/dev/null 2>&1; then
    tailscale status 2>/dev/null || true
  fi
}

tailscale_ip_for() {
  local host=$1
  tailscale_status | awk -v host="$host" '$2 == host { print $1; exit }'
}

ssh_user_for() {
  local host=$1
  ssh -G "$host" 2>/dev/null | awk '$1 == "user" { print $2; exit }'
}

ssh_probe() {
  local target=$1
  local out rc
  set +e
  out="$(ssh -o BatchMode=yes -o ConnectTimeout="$SSH_TIMEOUT" "$target" "$remote_probe" 2>&1)"
  rc=$?
  set -e
  if [[ $rc -eq 0 ]]; then
    printf '%s\n' "$out"
    return 0
  fi
  return "$rc"
}

remote_probe='
set -eu

host="$(hostname 2>/dev/null || scutil --get LocalHostName 2>/dev/null || uname -n)"
os="$(uname -s)"
printf "host: %s\n" "$host"
printf "os: %s\n" "$os"
printf "now: %s\n" "$(date "+%F %T %z")"

if [ "$os" = "Linux" ]; then
  printf "uptime: %s\n" "$(uptime 2>/dev/null || true)"
  if [ -r /proc/sys/kernel/random/boot_id ]; then
    printf "boot_id: %s\n" "$(cat /proc/sys/kernel/random/boot_id)"
  fi
  if command -v who >/dev/null 2>&1; then
    printf "boot_time: %s\n" "$(who -b 2>/dev/null | sed "s/^[[:space:]]*//")"
  fi

  if command -v journalctl >/dev/null 2>&1; then
    printf "boots:\n"
    journalctl --list-boots --no-pager 2>/dev/null | tail -4 || true

    printf "previous_tail_shutdown_markers:\n"
    journalctl -b -1 -n 40 --no-pager -o short-iso 2>/dev/null \
      | grep -E "Reached target (System Power Off|Reboot|Shutdown)|systemd-shutdown|Powering Off|Rebooting|Shutting down" \
      | tail -5 || true

    printf "previous_tail:\n"
    journalctl -b -1 -n 8 --no-pager -o short-iso 2>/dev/null || true

    printf "previous_boot_link_loss:\n"
    journalctl -b -1 --since "72 hours ago" --no-pager -o short-iso \
      --grep "NIC Link is Down|Lost carrier|LinkChange: all links down|DHCP lease lost" \
      2>/dev/null | tail -20 || true

    if command -v upsc >/dev/null 2>&1; then
      printf "ups_client: upsc present\n"
    fi
    if command -v apcaccess >/dev/null 2>&1; then
      printf "ups_client: apcaccess present\n"
    fi
    systemctl --no-pager --type=service --state=running 2>/dev/null \
      | grep -Ei "nut|ups|apc" || true

    printf "wake_on_lan:\n"
    if command -v ethtool >/dev/null 2>&1; then
      for path in /sys/class/net/*; do
        iface="${path##*/}"
        [ "$iface" = "lo" ] && continue
        ethtool "$iface" 2>/dev/null \
          | awk -v iface="$iface" "/Wake-on:/ {print iface \": \" \$0}"
      done
    else
      printf "ethtool missing\n"
    fi
  fi
elif [ "$os" = "Darwin" ]; then
  printf "uptime: %s\n" "$(uptime 2>/dev/null || true)"
  printf "boot_time: %s\n" "$(sysctl -n kern.boottime 2>/dev/null || true)"
  printf "recent_reboots:\n"
  last reboot 2>/dev/null | head -5 || true
  printf "recent_power_log:\n"
  pmset -g log 2>/dev/null \
    | grep -Ei "power|sleep|wake|shutdown|restart|failure" \
    | tail -30 || true
  printf "wake_and_restore_settings:\n"
  pmset -g custom 2>/dev/null \
    | grep -Ei "womp|autorestart|powernap|standby|hibernatemode" || true
else
  printf "uptime: %s\n" "$(uptime 2>/dev/null || true)"
fi
'

for host in $HOSTS; do
  section "$host"
  if ssh_probe "$host"; then
    continue
  fi
  ip="$(tailscale_ip_for "$host")"
  if [[ -n $ip ]]; then
    user="$(ssh_user_for "$host")"
    target="$ip"
    [[ -n $user ]] && target="${user}@${ip}"
    printf 'retry: %s via tailscale ip %s\n' "$host" "$ip"
    if ssh_probe "$target"; then
      continue
    fi
  fi
  printf 'unreachable: ssh failed or timed out\n'
done
