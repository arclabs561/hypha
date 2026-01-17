#!/usr/bin/env bash
set -euo pipefail

# Local CI runner that mirrors `.github/workflows/ci.yml`.
#
# Goals:
# - One command locally ≈ one job in CI.
# - Keep the actual logic in this repo (not in YAML) to reduce drift.
# - Fail loudly with actionable errors.
#
# Usage:
#   bash scripts/ci.sh rust
#   bash scripts/ci.sh netem
#   bash scripts/ci.sh all
#
# Notes:
# - `netem` requires Linux + sudo + iproute2 (`ip`, `tc`).
# - On macOS, use `bash scripts/ci_podman.sh netem` if you want the netns/tc job.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

die() {
  echo "error: $*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

is_linux() {
  [[ "$(uname -s)" == "Linux" ]]
}

run_rust_job() {
  echo "== rust job (mirrors .github/workflows/ci.yml:jobs.rust) =="
  (cd "$ROOT" && cargo check --all-targets --locked)
  (cd "$ROOT" && cargo build --examples --locked)
  (cd "$ROOT" && cargo fmt --check)
  (cd "$ROOT" && cargo clippy --all-targets --locked -- -D warnings)
  (cd "$ROOT" && cargo test --all-targets --locked)
  echo "== rust job ok =="
}

run_netem_job() {
  echo "== netem job (mirrors .github/workflows/ci.yml:jobs.netem) =="
  if ! is_linux; then
    die "netem job requires Linux (netns/tc). On macOS: bash scripts/ci_podman.sh netem"
  fi

  need_cmd sudo
  need_cmd ip
  need_cmd tc
  need_cmd timeout
  need_cmd seq

  (cd "$ROOT" && cargo build --example netem_node --locked)

  # The following blocks are intentionally kept close to CI YAML.

  echo "== netem: Netns + tc netem (TCP) =="
  (
    cd "$ROOT"
    set -euo pipefail

    sudo ip netns add hypha_sub
    sudo ip netns add hypha_pub

    cleanup() {
      sudo ip netns del hypha_sub 2>/dev/null || true
      sudo ip netns del hypha_pub 2>/dev/null || true
    }
    trap cleanup EXIT

    sudo ip link add veth_sub type veth peer name veth_pub
    sudo ip link set veth_sub netns hypha_sub
    sudo ip link set veth_pub netns hypha_pub

    sudo ip -n hypha_sub addr add 10.10.0.2/24 dev veth_sub
    sudo ip -n hypha_pub addr add 10.10.0.1/24 dev veth_pub
    sudo ip -n hypha_sub link set lo up
    sudo ip -n hypha_pub link set lo up
    sudo ip -n hypha_sub link set veth_sub up
    sudo ip -n hypha_pub link set veth_pub up

    sudo ip netns exec hypha_pub tc qdisc add dev veth_pub root netem \
      delay 80ms 40ms distribution normal \
      loss 10% 25% \
      reorder 2% 50% \
      duplicate 1% \
      corrupt 0.05%

    OUTFILE=/tmp/hypha_listen_tcp.txt
    rm -f "$OUTFILE"

    sudo ip netns exec hypha_sub env RUST_LOG=info \
      HYPHA_NETEM_SUB_START_DELAY_MS=1200 \
      HYPHA_NETEM_SUB_RECV_SECS=35 \
      timeout 55s ./target/debug/examples/netem_node sub tcp 10.10.0.2 /tmp/hypha_sub_tcp "$OUTFILE" &
    SUB_PID=$!

    for _ in $(seq 1 50); do
      if [ -s "$OUTFILE" ]; then
        break
      fi
      sleep 0.1
    done
    if [ ! -s "$OUTFILE" ]; then
      echo "subscriber did not write listen addr" >&2
      kill "$SUB_PID" || true
      exit 1
    fi

    PEER_ADDR="$(<"$OUTFILE")"
    echo "Dialing: $PEER_ADDR"

    sudo ip netns exec hypha_pub env RUST_LOG=info \
      HYPHA_NETEM_PUB_SETTLE_SECS=8 \
      HYPHA_NETEM_PUB_PUBLISH_RETRIES=25 \
      HYPHA_NETEM_PUB_BURST=3 \
      HYPHA_NETEM_PUB_MALFORMED_FIRST=1 \
      timeout 45s ./target/debug/examples/netem_node pub tcp 10.10.0.1 /tmp/hypha_pub_tcp "$PEER_ADDR" &
    PUB_PID=$!

    sudo ip netns exec hypha_pub tc qdisc replace dev veth_pub root netem loss 100%
    sleep 1
    sudo ip netns exec hypha_pub tc qdisc replace dev veth_pub root netem \
      delay 80ms 40ms distribution normal \
      loss 10% 25% \
      reorder 2% 50% \
      duplicate 1% \
      corrupt 0.05%

    wait "$PUB_PID"
    wait "$SUB_PID"
  )

  echo "== netem: Netns + tc netem (QUIC) =="
  (
    cd "$ROOT"
    set -euo pipefail

    sudo ip netns add hypha_sub
    sudo ip netns add hypha_pub

    cleanup() {
      sudo ip netns del hypha_sub 2>/dev/null || true
      sudo ip netns del hypha_pub 2>/dev/null || true
    }
    trap cleanup EXIT

    sudo ip link add veth_sub type veth peer name veth_pub
    sudo ip link set veth_sub netns hypha_sub
    sudo ip link set veth_pub netns hypha_pub

    sudo ip -n hypha_sub addr add 10.20.0.2/24 dev veth_sub
    sudo ip -n hypha_pub addr add 10.20.0.1/24 dev veth_pub
    sudo ip -n hypha_sub link set lo up
    sudo ip -n hypha_pub link set lo up
    sudo ip -n hypha_sub link set veth_sub up
    sudo ip -n hypha_pub link set veth_pub up

    sudo ip netns exec hypha_pub tc qdisc add dev veth_pub root netem \
      delay 120ms 60ms distribution normal \
      loss 7% 25% \
      reorder 2% 50% \
      duplicate 1% \
      corrupt 0.05%

    OUTFILE=/tmp/hypha_listen_quic.txt
    rm -f "$OUTFILE"

    sudo ip netns exec hypha_sub env RUST_LOG=info \
      HYPHA_NETEM_SUB_RECV_SECS=35 \
      timeout 55s ./target/debug/examples/netem_node sub quic 10.20.0.2 /tmp/hypha_sub_quic "$OUTFILE" &
    SUB_PID=$!

    for _ in $(seq 1 50); do
      if [ -s "$OUTFILE" ]; then
        break
      fi
      sleep 0.1
    done
    if [ ! -s "$OUTFILE" ]; then
      echo "subscriber did not write listen addr" >&2
      kill "$SUB_PID" || true
      exit 1
    fi

    PEER_ADDR="$(<"$OUTFILE")"
    echo "Dialing: $PEER_ADDR"

    sudo ip netns exec hypha_pub env RUST_LOG=info \
      HYPHA_NETEM_PUB_SETTLE_SECS=10 \
      HYPHA_NETEM_PUB_PUBLISH_RETRIES=30 \
      HYPHA_NETEM_PUB_BURST=3 \
      timeout 45s ./target/debug/examples/netem_node pub quic 10.20.0.1 /tmp/hypha_pub_quic "$PEER_ADDR" &
    PUB_PID=$!

    sudo ip netns exec hypha_pub tc qdisc replace dev veth_pub root netem loss 100%
    sleep 1
    sudo ip netns exec hypha_pub tc qdisc replace dev veth_pub root netem \
      delay 120ms 60ms distribution normal \
      loss 7% 25% \
      reorder 2% 50% \
      duplicate 1% \
      corrupt 0.05%

    wait "$PUB_PID"
    wait "$SUB_PID"
  )

  echo "== netem: Netns line Flapping + Churn (QUIC) =="
  (
    cd "$ROOT"
    set -euo pipefail
    sudo -E env HYPHA_CHAOS_DIR=/tmp/hypha_chaos_nasty \
      bash scripts/chaos/netns_chaos.sh line quic 42 25
  )

  echo "== netem job ok =="
}

usage() {
  cat <<'EOF'
usage:
  bash scripts/ci.sh <rust|netem|all>
EOF
}

main() {
  if [[ $# -ne 1 ]]; then
    usage
    exit 2
  fi

  case "$1" in
    rust) run_rust_job ;;
    netem) run_netem_job ;;
    all)
      run_rust_job
      if is_linux; then
        run_netem_job
      else
        echo "note: netem job skipped (requires Linux). On macOS: bash scripts/ci_podman.sh netem"
      fi
      ;;
    *)
      usage
      exit 2
      ;;
  esac
}

main "$@"

