#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d -t hypha-fleet-power.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/ssh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

saw_timeout=0
host=""
for arg in "$@"; do
  [[ $arg == "ConnectTimeout=1" ]] && saw_timeout=1
  case "$arg" in
    arcanine|metagross) host="$arg" ;;
  esac
done

if [[ $saw_timeout -ne 1 ]]; then
  printf 'missing ConnectTimeout=1\n' >&2
  exit 2
fi

case "$host" in
  arcanine)
    printf 'host: arcanine\n'
    printf 'os: Linux\n'
    printf 'boot_id: boot-arcanine\n'
    ;;
  metagross)
    exit 255
    ;;
  *)
    printf 'unexpected host in ssh args: %s\n' "$*" >&2
    exit 2
    ;;
esac
SH
chmod 0755 "$TMP/ssh"

OUT="$(
  PATH="$TMP:$PATH" HYPHA_FLEET_HOSTS="arcanine metagross" HYPHA_SSH_TIMEOUT=1 \
    bash "$ROOT/scripts/fleet_power_doctor.sh"
)"

grep -q '== arcanine ==' <<<"$OUT"
grep -q 'host: arcanine' <<<"$OUT"
grep -q 'boot_id: boot-arcanine' <<<"$OUT"
grep -q '== metagross ==' <<<"$OUT"
grep -q 'unreachable: ssh failed or timed out' <<<"$OUT"

printf 'fleet power doctor parser: ok\n'
