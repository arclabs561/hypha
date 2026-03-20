#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  ./run.sh netem-tcp
  ./run.sh netem-quic

Notes:
  - Linux only (requires: iproute2, tc, network namespaces).
  - Uses sudo to create netns/veth and apply tc netem.
EOF
}

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1" >&2
    exit 1
  }
}

require_linux() {
  local os
  os="$(uname -s)"
  if [[ "$os" != "Linux" ]]; then
    echo "This harness is Linux-only (ip netns + tc netem)." >&2
    echo "On macOS: run it inside a Linux VM (lima/colima) or a spare Linux machine." >&2
    exit 1
  fi
}

cleanup_netns() {
  sudo ip netns del hypha_sub 2>/dev/null || true
  sudo ip netns del hypha_pub 2>/dev/null || true
}

setup_netns() {
  cleanup_netns
  sudo ip netns add hypha_sub
  sudo ip netns add hypha_pub
  trap cleanup_netns EXIT
}

setup_veth() {
  local sub_ip="$1"
  local pub_ip="$2"

  sudo ip link add veth_sub type veth peer name veth_pub
  sudo ip link set veth_sub netns hypha_sub
  sudo ip link set veth_pub netns hypha_pub

  sudo ip -n hypha_sub addr add "${sub_ip}/24" dev veth_sub
  sudo ip -n hypha_pub addr add "${pub_ip}/24" dev veth_pub
  sudo ip -n hypha_sub link set lo up
  sudo ip -n hypha_pub link set lo up
  sudo ip -n hypha_sub link set veth_sub up
  sudo ip -n hypha_pub link set veth_pub up
}

build() {
  cargo build --example netem_node
}

run_pair() {
  local mode="$1"         # tcp|quic
  local sub_ip="$2"
  local pub_ip="$3"
  local netem_args="$4"   # e.g. "loss 5% delay 30ms"

  local outfile
  outfile="/tmp/hypha_listen_${mode}.txt"
  rm -f "$outfile"

  setup_netns
  setup_veth "$sub_ip" "$pub_ip"

  sudo ip netns exec hypha_pub tc qdisc add dev veth_pub root netem ${netem_args}

  sudo ip netns exec hypha_sub env RUST_LOG=info \
    ./target/debug/examples/netem_node sub "${mode}" "${sub_ip}" "/tmp/hypha_sub_${mode}" "$outfile" &
  local sub_pid=$!

  for _ in $(seq 1 50); do
    if [[ -s "$outfile" ]]; then
      break
    fi
    sleep 0.1
  done

  if [[ ! -s "$outfile" ]]; then
    echo "subscriber did not write listen addr" >&2
    kill "$sub_pid" || true
    exit 1
  fi

  local peer_addr
  peer_addr="$(<"$outfile")"
  echo "Dialing: $peer_addr"

  sudo ip netns exec hypha_pub env RUST_LOG=info \
    ./target/debug/examples/netem_node pub "${mode}" "${pub_ip}" "/tmp/hypha_pub_${mode}" "$peer_addr"

  wait "$sub_pid"
}

main() {
  case "${1:-}" in
    netem-tcp)
      require_linux
      need sudo
      need ip
      need tc
      need cargo
      build
      run_pair tcp 10.10.0.2 10.10.0.1 "loss 5% delay 30ms"
      ;;
    netem-quic)
      require_linux
      need sudo
      need ip
      need tc
      need cargo
      build
      run_pair quic 10.20.0.2 10.20.0.1 "loss 3% delay 80ms"
      ;;
    -h|--help|"")
      usage
      exit 0
      ;;
    *)
      echo "unknown command: ${1:-}" >&2
      usage
      exit 1
      ;;
  esac
}

main "$@"

