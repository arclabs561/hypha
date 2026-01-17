#!/usr/bin/env bash
set -euo pipefail

# Run CI inside a Linux container (podman preferred).
#
# This is mainly for macOS: it lets you run the Linux-only `netem` job locally.
#
# Usage:
#   bash scripts/ci_podman.sh netem
#   bash scripts/ci_podman.sh rust
#   bash scripts/ci_podman.sh all

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

die() {
  echo "error: $*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

usage() {
  cat <<'EOF'
usage:
  bash scripts/ci_podman.sh <rust|netem|all>

notes:
  - requires podman
  - runs a privileged container because `netns` + `tc netem` need CAP_NET_ADMIN
EOF
}

main() {
  if [[ $# -ne 1 ]]; then
    usage
    exit 2
  fi
  need_cmd podman

  local job="$1"
  local image="${HYPHA_CI_IMAGE:-rust:1.91-bookworm}"

  # Keep the container logic very simple. We install the minimum needed packages.
  # - iproute2 provides `ip` and `tc`
  # - sudo is used by existing harness scripts
  # - ca-certificates for HTTPS dependency fetches
  podman run --rm --privileged \
    -e RUST_BACKTRACE=1 \
    -v "$ROOT":/work:Z \
    -w /work \
    "$image" \
    bash -lc "
      set -euo pipefail
      apt-get update
      apt-get install -y --no-install-recommends ca-certificates iproute2 sudo
      bash scripts/ci.sh '$job'
    "
}

main "$@"

