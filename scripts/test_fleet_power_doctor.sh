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
is_config=0
for arg in "$@"; do
  [[ $arg == "-G" ]] && is_config=1
  [[ $arg == "ConnectTimeout=1" ]] && saw_timeout=1
  case "$arg" in
    arcanine|metagross|arc@100.64.0.10|henry@100.64.0.20) host="$arg" ;;
  esac
done

if [[ $is_config -eq 1 ]]; then
  case "$host" in
    arcanine) printf 'user arc\n' ;;
    metagross) printf 'user henry\n' ;;
    *) printf 'user arc\n' ;;
  esac
  exit 0
fi

if [[ $saw_timeout -ne 1 ]]; then
  printf 'missing ConnectTimeout=1\n' >&2
  exit 2
fi

case "$host" in
  arcanine)
    printf 'ssh: Could not resolve hostname arcanine\n' >&2
    exit 255
    ;;
  arc@100.64.0.10)
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

cat >"$TMP/tailscale" <<'SH'
#!/usr/bin/env bash
if [[ ${1:-} == "status" ]]; then
  printf '100.64.0.10   arcanine   tagged-devices linux -\n'
  printf '100.64.0.20   metagross  henry@         macOS  -\n'
fi
SH
chmod 0755 "$TMP/tailscale"

OUT="$(
  PATH="$TMP:$PATH" HYPHA_FLEET_HOSTS="arcanine metagross" HYPHA_SSH_TIMEOUT=1 \
    bash "$ROOT/scripts/fleet_power_doctor.sh"
)"

grep -q '== arcanine ==' <<<"$OUT"
grep -q 'retry: arcanine via tailscale ip 100.64.0.10' <<<"$OUT"
grep -q 'host: arcanine' <<<"$OUT"
grep -q 'boot_id: boot-arcanine' <<<"$OUT"
grep -q '== metagross ==' <<<"$OUT"
grep -q 'unreachable: ssh failed or timed out' <<<"$OUT"

printf 'fleet power doctor parser: ok\n'
