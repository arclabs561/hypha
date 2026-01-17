#!/usr/bin/env bash
set -euo pipefail

#
# Deterministic "chaos monkey" harness for Hypha.
#
# Goals:
# - Reproducible: every run prints the full schedule and uses an explicit SEED.
# - Bounded: no infinite waits; every process is wrapped in timeout.
# - Focused: the invariant is "eventual delivery" under short partitions/churn.
#
# Requirements (Linux):
# - iproute2 (ip, tc)
# - sudo privileges for netns/tc operations
#

usage() {
  cat <<'EOF'
usage:
  scripts/chaos/netns_chaos.sh <pair|line|throttle|star> <tcp|quic> <seed> <duration_secs>

examples:
  scripts/chaos/netns_chaos.sh star tcp 1 20

examples:
  scripts/chaos/netns_chaos.sh pair tcp 1 15
  scripts/chaos/netns_chaos.sh line quic 42 20
  scripts/chaos/netns_chaos.sh throttle tcp 99 15

environment:
  HYPHA_NETEM_NODE_BIN  Path to netem_node binary (default: ./target/debug/examples/netem_node)
  HYPHA_CHAOS_DIR       Where to write schedule/logs (default: /tmp/hypha_chaos)
  HYPHA_CHAOS_METRICS   If "1", sample process RSS into metrics.csv
  HYPHA_CHAOS_SAMPLE_MS Sampling interval for metrics (default: 500)
  HYPHA_CHAOS_RELAY_HARD_KILL If "1", restart relay using SIGKILL (line topology)
  HYPHA_CHAOS_SUB_SLEEP_SECS  If nonzero, SIGSTOP/SIGCONT subscriber for this many seconds (pair topology)
  HYPHA_CHAOS_PUB_MALFORMED_FIRST If "1", set HYPHA_NETEM_PUB_MALFORMED_FIRST=1 for the publisher
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ $# -lt 4 ]]; then
  usage
  exit 2
fi

TOPOLOGY="$1"
TRANSPORT="$2"
SEED="$3"
DURATION_SECS="$4"

case "$TOPOLOGY" in
  pair|line|throttle|star) ;;
  *) echo "invalid topology: $TOPOLOGY (expected pair|line|throttle|star)" >&2; exit 2 ;;
esac

case "$TRANSPORT" in
  tcp|quic) ;;
  *) echo "invalid transport: $TRANSPORT (expected tcp|quic)" >&2; exit 2 ;;
esac

if ! [[ "$SEED" =~ ^[0-9]+$ ]]; then
  echo "seed must be an integer" >&2
  exit 2
fi

if ! [[ "$DURATION_SECS" =~ ^[0-9]+$ ]]; then
  echo "duration_secs must be an integer" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIN="${HYPHA_NETEM_NODE_BIN:-"$ROOT/target/debug/examples/netem_node"}"
OUTDIR="${HYPHA_CHAOS_DIR:-/tmp/hypha_chaos}"
RUN_ID="${TOPOLOGY}_${TRANSPORT}_seed${SEED}_$(date +%s)"
RUN_DIR="$OUTDIR/$RUN_ID"

mkdir -p "$RUN_DIR"
SCHEDULE="$RUN_DIR/schedule.txt"
LOG="$RUN_DIR/log.txt"
PID_DIR="$RUN_DIR/pids"
METRICS_CSV="$RUN_DIR/metrics.csv"

mkdir -p "$PID_DIR"

echo "hypha chaos: topology=$TOPOLOGY transport=$TRANSPORT seed=$SEED duration=${DURATION_SECS}s" | tee "$SCHEDULE" | tee -a "$LOG"
echo "run_dir=$RUN_DIR" | tee -a "$SCHEDULE" | tee -a "$LOG"

log() {
  # log "t=..." "message"
  echo "$*" | tee -a "$LOG"
}

schedule_line() {
  echo "$*" | tee -a "$SCHEDULE"
}

metrics_enabled() {
  [[ "${HYPHA_CHAOS_METRICS:-0}" == "1" ]]
}

metrics_sample_ms() {
  local v="${HYPHA_CHAOS_SAMPLE_MS:-500}"
  if [[ "$v" =~ ^[0-9]+$ ]] && [[ "$v" -ge 10 ]]; then
    echo "$v"
  else
    echo "500"
  fi
}

pidfile_set() {
  local role="$1"
  local pid="$2"
  echo "$pid" > "$PID_DIR/$role.pid"
}

pidfile_get() {
  local role="$1"
  local path="$PID_DIR/$role.pid"
  if [[ -f "$path" ]]; then
    cat "$path"
  fi
}

start_metrics_sampler() {
  if ! metrics_enabled; then
    return 0
  fi

  local interval_ms
  interval_ms="$(metrics_sample_ms)"

  echo "t_ms,role,pid,rss_kb" > "$METRICS_CSV"
  schedule_line "metrics: enabled interval_ms=$interval_ms"

  (
    set -euo pipefail
    while true; do
      # GNU date supports %3N for milliseconds.
      local t_ms
      t_ms="$(date +%s%3N)"

      for role in sub pub relay; do
        local pid rss
        pid="$(pidfile_get "$role" || true)"
        if [[ -n "${pid:-}" ]] && kill -0 "$pid" 2>/dev/null; then
          rss="$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ' || true)"
        else
          rss=""
        fi
        echo "${t_ms},${role},${pid:-},${rss}" >> "$METRICS_CSV"
      done

      sleep "$(awk "BEGIN { print ${interval_ms}/1000 }")"
    done
  ) >>"$LOG" 2>&1 &

  echo "$!" > "$PID_DIR/metrics_sampler.pid"
}

stop_metrics_sampler() {
  if [[ -f "$PID_DIR/metrics_sampler.pid" ]]; then
    local pid
    pid="$(cat "$PID_DIR/metrics_sampler.pid")"
    kill "$pid" 2>/dev/null || true
  fi
}

require_linux() {
  if [[ "$(uname -s)" != "Linux" ]]; then
    echo "This harness requires Linux netns/tc. uname=$(uname -s)" >&2
    exit 1
  fi
  if ! command -v ip >/dev/null 2>&1; then
    echo "missing: ip" >&2
    exit 1
  fi
  if ! command -v tc >/dev/null 2>&1; then
    echo "missing: tc" >&2
    exit 1
  fi
  if [[ ! -x "$BIN" ]]; then
    echo "missing netem_node binary at: $BIN" >&2
    echo "hint: cargo build --example netem_node --locked" >&2
    exit 1
  fi
}

tc_apply_profile() {
  # tc_apply_profile <netns> <dev> <profile_name>
  local ns="$1"
  local dev="$2"
  local profile="$3"

  case "$profile" in
    baseline)
      # "Ugly but connected". Use netem seed to keep the stochastic parts reproducible.
      sudo ip netns exec "$ns" tc qdisc replace dev "$dev" root netem \
        delay 80ms 40ms distribution normal \
        loss 10% 25% \
        reorder 2% 50% \
        duplicate 1% \
        corrupt 0.05% \
        seed "$SEED"
      ;;
    jitter_spike)
      sudo ip netns exec "$ns" tc qdisc replace dev "$dev" root netem \
        delay 250ms 150ms distribution normal \
        loss 15% 25% \
        reorder 5% 50% \
        duplicate 2% \
        corrupt 0.1% \
        seed "$SEED"
      ;;
    flap)
      # Fast flapping: toggles loss 100% and baseline every 500ms for 3s
      for _ in {1..3}; do
        sudo ip netns exec "$ns" tc qdisc replace dev "$dev" root netem loss 100%
        sleep 0.5
        sudo ip netns exec "$ns" tc qdisc replace dev "$dev" root netem \
          delay 80ms 40ms distribution normal loss 10% 25% seed "$SEED"
        sleep 0.5
      done
      ;;
    throttle_low)
      # Sudden bandwidth drop (Token Bucket Filter).
      # rate 50kbit, burst 10k (allow small bursts), latency 50ms (queue limit)
      sudo ip netns exec "$ns" tc qdisc replace dev "$dev" root tbf \
        rate 50kbit burst 10k latency 50ms
      ;;
    throttle_fluctuate)
      # Fluctuate between 100kbit and 1mbit
      for _ in {1..2}; do
        sudo ip netns exec "$ns" tc qdisc replace dev "$dev" root tbf \
          rate 100kbit burst 10k latency 50ms
        sleep 1.0
        sudo ip netns exec "$ns" tc qdisc replace dev "$dev" root tbf \
          rate 1mbit burst 32k latency 50ms
        sleep 1.0
      done
      ;;
    throttle_low)
      # Sudden bandwidth drop (Token Bucket Filter).
      # rate 50kbit, burst 10k (allow small bursts), latency 50ms (queue limit)
      sudo ip netns exec "$ns" tc qdisc replace dev "$dev" root tbf \
        rate 50kbit burst 10k latency 50ms
      ;;
    throttle_fluctuate)
      # Fluctuate between 100kbit and 1mbit
      for _ in {1..2}; do
        sudo ip netns exec "$ns" tc qdisc replace dev "$dev" root tbf \
          rate 100kbit burst 10k latency 50ms
        sleep 1.0
        sudo ip netns exec "$ns" tc qdisc replace dev "$dev" root tbf \
          rate 1mbit burst 32k latency 50ms
        sleep 1.0
      done
      ;;
    partition_burst)
      sudo ip netns exec "$ns" tc qdisc replace dev "$dev" root netem loss 100%
      ;;
    recover)
      # Back to baseline.
      tc_apply_profile "$ns" "$dev" baseline
      ;;
    *)
      echo "unknown tc profile: $profile" >&2
      exit 2
      ;;
  esac
}

cleanup_pair() {
  sudo ip netns del hypha_pub 2>/dev/null || true
  sudo ip netns del hypha_sub 2>/dev/null || true
}

cleanup_line() {
  sudo ip netns del hypha_pub 2>/dev/null || true
  sudo ip netns del hypha_relay 2>/dev/null || true
  sudo ip netns del hypha_sub 2>/dev/null || true
  sudo ip link del br_hypha 2>/dev/null || true
}

cleanup_star() {
  sudo ip netns del hypha_hub 2>/dev/null || true
  sudo ip netns del hypha_leaf1 2>/dev/null || true
  sudo ip netns del hypha_leaf2 2>/dev/null || true
  sudo ip netns del hypha_leaf3 2>/dev/null || true
  sudo ip link del br_hypha 2>/dev/null || true
}

setup_pair() {
  sudo ip netns add hypha_sub
  sudo ip netns add hypha_pub

  sudo ip link add veth_sub type veth peer name veth_pub
  sudo ip link set veth_sub netns hypha_sub
  sudo ip link set veth_pub netns hypha_pub

  sudo ip -n hypha_sub addr add 10.10.0.2/24 dev veth_sub
  sudo ip -n hypha_pub addr add 10.10.0.1/24 dev veth_pub
  sudo ip -n hypha_sub link set lo up
  sudo ip -n hypha_pub link set lo up
  sudo ip -n hypha_sub link set veth_sub up
  sudo ip -n hypha_pub link set veth_pub up
}

setup_line() {
  sudo ip netns add hypha_pub
  sudo ip netns add hypha_relay
  sudo ip netns add hypha_sub

  sudo ip link add name br_hypha type bridge
  sudo ip link set br_hypha up

  # pub
  sudo ip link add veth_pub type veth peer name veth_pub_br
  sudo ip link set veth_pub netns hypha_pub
  sudo ip link set veth_pub_br master br_hypha
  sudo ip link set veth_pub_br up
  sudo ip -n hypha_pub addr add 10.30.0.1/24 dev veth_pub
  sudo ip -n hypha_pub link set lo up
  sudo ip -n hypha_pub link set veth_pub up

  # relay
  sudo ip link add veth_relay type veth peer name veth_relay_br
  sudo ip link set veth_relay netns hypha_relay
  sudo ip link set veth_relay_br master br_hypha
  sudo ip link set veth_relay_br up
  sudo ip -n hypha_relay addr add 10.30.0.2/24 dev veth_relay
  sudo ip -n hypha_relay link set lo up
  sudo ip -n hypha_relay link set veth_relay up

  # sub
  sudo ip link add veth_sub type veth peer name veth_sub_br
  sudo ip link set veth_sub netns hypha_sub
  sudo ip link set veth_sub_br master br_hypha
  sudo ip link set veth_sub_br up
  sudo ip -n hypha_sub addr add 10.30.0.3/24 dev veth_sub
  sudo ip -n hypha_sub link set lo up
  sudo ip -n hypha_sub link set veth_sub up
}

setup_star() {
  sudo ip netns add hypha_hub
  sudo ip netns add hypha_leaf1
  sudo ip netns add hypha_leaf2
  sudo ip netns add hypha_leaf3

  sudo ip link add name br_hypha type bridge
  sudo ip link set br_hypha up

  # Hub
  sudo ip link add veth_hub type veth peer name veth_hub_br
  sudo ip link set veth_hub netns hypha_hub
  sudo ip link set veth_hub_br master br_hypha
  sudo ip link set veth_hub_br up
  sudo ip -n hypha_hub addr add 10.50.0.1/24 dev veth_hub
  sudo ip -n hypha_hub link set lo up
  sudo ip -n hypha_hub link set veth_hub up

  # Leaf 1
  sudo ip link add veth_l1 type veth peer name veth_l1_br
  sudo ip link set veth_l1 netns hypha_leaf1
  sudo ip link set veth_l1_br master br_hypha
  sudo ip link set veth_l1_br up
  sudo ip -n hypha_leaf1 addr add 10.50.0.2/24 dev veth_l1
  sudo ip -n hypha_leaf1 link set lo up
  sudo ip -n hypha_leaf1 link set veth_l1 up

  # Leaf 2
  sudo ip link add veth_l2 type veth peer name veth_l2_br
  sudo ip link set veth_l2 netns hypha_leaf2
  sudo ip link set veth_l2_br master br_hypha
  sudo ip link set veth_l2_br up
  sudo ip -n hypha_leaf2 addr add 10.50.0.3/24 dev veth_l2
  sudo ip -n hypha_leaf2 link set lo up
  sudo ip -n hypha_leaf2 link set veth_l2 up

  # Leaf 3
  sudo ip link add veth_l3 type veth peer name veth_l3_br
  sudo ip link set veth_l3 netns hypha_leaf3
  sudo ip link set veth_l3_br master br_hypha
  sudo ip link set veth_l3_br up
  sudo ip -n hypha_leaf3 addr add 10.50.0.4/24 dev veth_l3
  sudo ip -n hypha_leaf3 link set lo up
  sudo ip -n hypha_leaf3 link set veth_l3 up
}

wait_for_file() {
  local path="$1"
  local attempts="$2"
  local delay="$3"
  for _ in $(seq 1 "$attempts"); do
    if [[ -s "$path" ]]; then
      return 0
    fi
    sleep "$delay"
  done
  return 1
}

run_pair() {
  trap cleanup_pair EXIT
  setup_pair

  tc_apply_profile hypha_pub veth_pub baseline

  local sub_out="$RUN_DIR/sub_addr.txt"
  rm -f "$sub_out"

  # Sleepy subscriber: delayed start is part of the test.
  schedule_line "sub: start_delay_ms=1200 recv_secs=$((DURATION_SECS + 10))"
  sudo ip netns exec hypha_sub env RUST_LOG=info \
    HYPHA_NETEM_SUB_START_DELAY_MS=1200 \
    HYPHA_NETEM_SUB_RECV_SECS="$((DURATION_SECS + 10))" \
    timeout "$((DURATION_SECS + 20))"s "$BIN" sub "$TRANSPORT" 10.10.0.2 "$RUN_DIR/sub_store" "$sub_out" \
    >>"$LOG" 2>&1 &
  local sub_pid=$!
  pidfile_set sub "$sub_pid"

  if ! wait_for_file "$sub_out" 80 0.1; then
    log "subscriber did not write listen addr"
    kill "$sub_pid" 2>/dev/null || true
    exit 1
  fi

  local peer_addr
  peer_addr="$(<"$sub_out")"
  schedule_line "pub: settle_secs=10 retries=35 burst=3"

  local pub_malformed="${HYPHA_CHAOS_PUB_MALFORMED_FIRST:-0}"
  sudo ip netns exec hypha_pub env RUST_LOG=info \
    HYPHA_NETEM_PUB_SETTLE_SECS=10 \
    HYPHA_NETEM_PUB_PUBLISH_RETRIES=35 \
    HYPHA_NETEM_PUB_BURST=3 \
    HYPHA_NETEM_PUB_FLUSH_MS=1200 \
    HYPHA_NETEM_PUB_MALFORMED_FIRST="$pub_malformed" \
    timeout "$((DURATION_SECS + 10))"s "$BIN" pub "$TRANSPORT" 10.10.0.1 "$RUN_DIR/pub_store" "$peer_addr" \
    >>"$LOG" 2>&1 &
  local pub_pid=$!
  pidfile_set pub "$pub_pid"

  start_metrics_sampler

  local sub_sleep_secs="${HYPHA_CHAOS_SUB_SLEEP_SECS:-0}"
  if [[ "$sub_sleep_secs" != "0" ]]; then
    schedule_line "sub: sleep ${sub_sleep_secs}s (SIGSTOP/SIGCONT)"
    sudo kill -STOP "$sub_pid" 2>/dev/null || true
    sleep "$sub_sleep_secs"
    sudo kill -CONT "$sub_pid" 2>/dev/null || true
  fi

  # Deterministic schedule. Times are relative to "now".
  schedule_line "t=+1.5s tc=partition_burst (1.0s)"
  sleep 1.5
  tc_apply_profile hypha_pub veth_pub partition_burst
  sleep 1.0
  schedule_line "t=+2.5s tc=recover"
  tc_apply_profile hypha_pub veth_pub recover

  schedule_line "t=+5.0s tc=jitter_spike (2.0s)"
  sleep 2.5
  tc_apply_profile hypha_pub veth_pub jitter_spike
  sleep 2.0
  schedule_line "t=+7.0s tc=recover"
  tc_apply_profile hypha_pub veth_pub recover

  wait "$pub_pid"
  wait "$sub_pid"
  stop_metrics_sampler
}

run_line() {
  trap cleanup_line EXIT
  setup_line

  local relay_sleep_secs="${HYPHA_CHAOS_RELAY_SLEEP_SECS:-0}"
  local relay_hard_kill="${HYPHA_CHAOS_RELAY_HARD_KILL:-0}"

  # Make pub noisy; relay clean; sub mildly delayed by default.
  tc_apply_profile hypha_pub veth_pub baseline
  sudo ip netns exec hypha_sub tc qdisc replace dev veth_sub root netem delay 25ms 10ms distribution normal seed "$SEED"

  local sub_out="$RUN_DIR/sub_addr.txt"
  local relay_out="$RUN_DIR/relay_addr.txt"
  rm -f "$sub_out" "$relay_out"

  schedule_line "sub: start_delay_ms=800 recv_secs=$((DURATION_SECS + 15))"
  sudo ip netns exec hypha_sub env RUST_LOG=info \
    HYPHA_NETEM_SUB_START_DELAY_MS=800 \
    HYPHA_NETEM_SUB_RECV_SECS="$((DURATION_SECS + 15))" \
    timeout "$((DURATION_SECS + 25))"s "$BIN" sub "$TRANSPORT" 10.30.0.3 "$RUN_DIR/sub_store" "$sub_out" \
    >>"$LOG" 2>&1 &
  local sub_pid=$!
  pidfile_set sub "$sub_pid"

  if ! wait_for_file "$sub_out" 80 0.1; then
    log "sub did not write addr"
    kill "$sub_pid" 2>/dev/null || true
    exit 1
  fi
  local sub_addr
  sub_addr="$(<"$sub_out")"

  schedule_line "relay: run_ms=$((DURATION_SECS * 1000))"
  local relay_store="$RUN_DIR/relay_store"
  local relay_cmd=(sudo ip netns exec hypha_relay env RUST_LOG=info \
    timeout "$((DURATION_SECS + 20))"s "$BIN" relay "$TRANSPORT" 10.30.0.2 "$relay_store" "$relay_out" "$sub_addr" "$((DURATION_SECS * 1000))")
  
  "${relay_cmd[@]}" >>"$LOG" 2>&1 &
  local relay_pid=$!
  pidfile_set relay "$relay_pid"

  if ! wait_for_file "$relay_out" 80 0.1; then
    log "relay did not write addr"
    kill "$sub_pid" 2>/dev/null || true
    kill "$relay_pid" 2>/dev/null || true
    exit 1
  fi
  local relay_addr
  relay_addr="$(<"$relay_out")"

  schedule_line "pub: settle_secs=12 retries=40 burst=3"
  local pub_malformed="${HYPHA_CHAOS_PUB_MALFORMED_FIRST:-0}"
  sudo ip netns exec hypha_pub env RUST_LOG=info \
    HYPHA_NETEM_PUB_SETTLE_SECS=12 \
    HYPHA_NETEM_PUB_PUBLISH_RETRIES=40 \
    HYPHA_NETEM_PUB_BURST=3 \
    HYPHA_NETEM_PUB_FLUSH_MS=1500 \
    HYPHA_NETEM_PUB_MALFORMED_FIRST="$pub_malformed" \
    timeout "$((DURATION_SECS + 10))"s "$BIN" pub "$TRANSPORT" 10.30.0.1 "$RUN_DIR/pub_store" "$relay_addr" \
    >>"$LOG" 2>&1 &
  local pub_pid=$!
  pidfile_set pub "$pub_pid"

  start_metrics_sampler

  # Fault schedule: flap pub egress, optionally pause relay, restart relay, then jitter spike.
  schedule_line "t=+1.0s tc=flap (3.0s)"
  sleep 1.0
  tc_apply_profile hypha_pub veth_pub flap

  if [[ "$relay_sleep_secs" != "0" ]]; then
    schedule_line "relay: sleep ${relay_sleep_secs}s (SIGSTOP/SIGCONT)"
    sudo kill -STOP "$relay_pid" 2>/dev/null || true
    sleep "$relay_sleep_secs"
    sudo kill -CONT "$relay_pid" 2>/dev/null || true
  fi

  schedule_line "t=+4.5s relay=restart"
  if [[ "$relay_hard_kill" == "1" ]]; then
    sudo kill -KILL "$relay_pid" 2>/dev/null || true
  else
    sudo kill "$relay_pid" 2>/dev/null || true
  fi
  sleep 0.5
  "${relay_cmd[@]}" >>"$LOG" 2>&1 &
  relay_pid=$!
  pidfile_set relay "$relay_pid"

  schedule_line "t=+6.0s tc=jitter_spike (2.5s)"
  sleep 1.0
  tc_apply_profile hypha_pub veth_pub jitter_spike
  sleep 2.5
  schedule_line "t=+8.5s tc=recover"
  tc_apply_profile hypha_pub veth_pub recover

  wait "$pub_pid"
  wait "$sub_pid"
  wait "$relay_pid"
  stop_metrics_sampler
}

run_throttle() {
  trap cleanup_pair EXIT
  setup_pair

  tc_apply_profile hypha_pub veth_pub baseline

  local sub_out="$RUN_DIR/sub_addr.txt"
  rm -f "$sub_out"

  # Run subscriber longer
  sudo ip netns exec hypha_sub env RUST_LOG=info \
    timeout "$((DURATION_SECS + 15))"s "$BIN" sub "$TRANSPORT" 10.10.0.2 "$RUN_DIR/sub_store" "$sub_out" \
    >>"$LOG" 2>&1 &
  local sub_pid=$!
  pidfile_set sub "$sub_pid"

  if ! wait_for_file "$sub_out" 80 0.1; then
    log "sub no addr"
    kill "$sub_pid" 2>/dev/null || true
    exit 1
  fi
  local peer_addr
  peer_addr="$(<"$sub_out")"

  start_metrics_sampler

  # Pub sends messages while bandwidth chokes
  sudo ip netns exec hypha_pub env RUST_LOG=info \
    HYPHA_NETEM_PUB_PUBLISH_RETRIES=50 \
    HYPHA_NETEM_PUB_BURST=1 \
    HYPHA_NETEM_PUB_FLUSH_MS=2000 \
    timeout "$((DURATION_SECS + 10))"s "$BIN" pub "$TRANSPORT" 10.10.0.1 "$RUN_DIR/pub_store" "$peer_addr" \
    >>"$LOG" 2>&1 &
  local pub_pid=$!
  pidfile_set pub "$pub_pid"

  schedule_line "t=+2.0s tc=throttle_low (2.0s)"
  sleep 2.0
  tc_apply_profile hypha_pub veth_pub throttle_low
  sleep 2.0

  schedule_line "t=+4.0s tc=throttle_fluctuate (4.0s)"
  tc_apply_profile hypha_pub veth_pub throttle_fluctuate
  
  schedule_line "t=+8.0s tc=recover"
  tc_apply_profile hypha_pub veth_pub recover

  wait "$pub_pid"
  wait "$sub_pid"
  stop_metrics_sampler
}

run_star() {
  trap cleanup_star EXIT
  setup_star

  # Hub is a relay
  local hub_out="$RUN_DIR/hub_addr.txt"
  local leaf1_out="$RUN_DIR/leaf1_addr.txt"
  rm -f "$hub_out" "$leaf1_out"

  start_metrics_sampler

  # Start Hub
  schedule_line "hub: relay mode"
  sudo ip netns exec hypha_hub env RUST_LOG=info \
    timeout "$((DURATION_SECS + 20))"s "$BIN" relay "$TRANSPORT" 10.50.0.1 "$RUN_DIR/hub_store" "$hub_out" "none" "$((DURATION_SECS * 1000))" \
    >>"$LOG" 2>&1 &
  local hub_pid=$!
  pidfile_set hub "$hub_pid"

  if ! wait_for_file "$hub_out" 80 0.1; then
    log "hub did not write addr"
    kill "$hub_pid" || true
    exit 1
  fi
  local hub_addr
  hub_addr="$(<"$hub_out")"

  # Start Leaf 1 (Subscriber) connected to Hub
  schedule_line "leaf1: sub mode, dial hub"
  sudo ip netns exec hypha_leaf1 env RUST_LOG=info \
    HYPHA_NETEM_SUB_RECV_SECS="$((DURATION_SECS + 10))" \
    timeout "$((DURATION_SECS + 20))"s "$BIN" sub "$TRANSPORT" 10.50.0.2 "$RUN_DIR/l1_store" "$leaf1_out" \
    >>"$LOG" 2>&1 &
  local l1_pid=$!
  pidfile_set l1 "$l1_pid"

  if ! wait_for_file "$leaf1_out" 80 0.1; then
    log "leaf1 did not write addr"
    kill "$hub_pid" "$l1_pid" || true
    exit 1
  fi
  
  # Note: netem_node sub doesn't dial automatically unless we teach it.
  # Currently `sub` mode listens. `pub` dials.
  # To form a star, leaves must dial the hub? Or hub dials leaves?
  # If hub is relay, leaves usually dial hub.
  # But `netem_node` `sub` logic just listens.
  # `pub` logic dials.
  # I'll treat Leaf 2 as Pub (dials Hub).
  # Leaf 3 as Pub (dials Hub).
  # Leaf 1 is Sub (listens).
  # BUT Leaf 1 needs to be connected to Hub to receive gossip.
  # If Leaf 1 is isolated, it won't get messages from Leaf 2 via Hub.
  
  # Limitation of `netem_node`: `sub` mode doesn't dial.
  # Workaround: Hub dials Leaf 1? Hub logic in `netem_node` (`relay` mode) takes optional `dial_peer`.
  # So I can tell Hub to dial Leaf 1.
  
  # Restart Hub with dial to Leaf 1? No, Hub is already running.
  # I'll restart Hub? No.
  
  # I'll modify `netem_node` to allow `sub` to dial a bootnode?
  # Or just rely on Leaf 2 and Leaf 3 dialing Hub, and hope Hub gossips to... nobody?
  # Wait, if Leaf 1 never connects to Hub, it's isolated.
  
  # I will use Leaf 2 (Pub) to dial Hub.
  # I will use Leaf 3 (Pub) to dial Hub.
  # I will use Leaf 1 (Sub) ... 
  # Actually, `netem_node` `relay` mode can dial *one* peer.
  # So Hub can dial Leaf 1.
  # Let's do that. Hub dials Leaf 1. Leaf 2 dials Hub. Leaf 3 dials Hub.
  # Star formed.
  
  # Wait, I started Hub first. It's waiting for Leaf 1 address?
  # No, Hub started without dial arg.
  # I should start Leaf 1 first, get its address, then start Hub dialing Leaf 1.
  
  # RESTART Hub logic for Star topology setup sequence:
  kill "$hub_pid" || true
  
  # 1. Start Leaf 1 (Sub)
  # Already running as l1_pid.
  local leaf1_addr
  leaf1_addr="$(<"$leaf1_out")"
  
  # 2. Start Hub (Relay), dialing Leaf 1
  schedule_line "hub: dialing leaf1"
  sudo ip netns exec hypha_hub env RUST_LOG=info \
    timeout "$((DURATION_SECS + 20))"s "$BIN" relay "$TRANSPORT" 10.50.0.1 "$RUN_DIR/hub_store" "$hub_out" "$leaf1_addr" "$((DURATION_SECS * 1000))" \
    >>"$LOG" 2>&1 &
  hub_pid=$!
  pidfile_set hub "$hub_pid"
  
  if ! wait_for_file "$hub_out" 80 0.1; then
    log "hub failed to restart"
    exit 1
  fi
  hub_addr="$(<"$hub_out")"

  # 3. Start Leaf 2 (Pub), dialing Hub
  schedule_line "leaf2: pub, dialing hub"
  sudo ip netns exec hypha_leaf2 env RUST_LOG=info \
    HYPHA_NETEM_PUB_PUBLISH_RETRIES=100 \
    HYPHA_NETEM_PUB_BURST=1 \
    timeout "$((DURATION_SECS + 10))"s "$BIN" pub "$TRANSPORT" 10.50.0.3 "$RUN_DIR/l2_store" "$hub_addr" \
    >>"$LOG" 2>&1 &
  local l2_pid=$!
  pidfile_set l2 "$l2_pid"

  # 4. Kill Hub mid-run
  schedule_line "t=+5s kill hub"
  sleep 5.0
  sudo kill -KILL "$hub_pid"
  
  # 5. Restart Hub
  schedule_line "t=+8s restart hub"
  sleep 3.0
  # Hub restarts, dialing Leaf 1 again (recovery)
  sudo ip netns exec hypha_hub env RUST_LOG=info \
    timeout "$((DURATION_SECS + 10))"s "$BIN" relay "$TRANSPORT" 10.50.0.1 "$RUN_DIR/hub_store" "$hub_out" "$leaf1_addr" "$((DURATION_SECS * 1000))" \
    >>"$LOG" 2>&1 &
  hub_pid=$!
  pidfile_set hub "$hub_pid"
  
  # Leaf 2 should ideally reconnect to Hub? `libp2p` swarm might retry dialing if address is same?
  # Or Leaf 2 fails.
  # This tests recovery.
  
  wait "$l1_pid"
  wait "$l2_pid"
  wait "$hub_pid"
  stop_metrics_sampler
}

main() {
  require_linux
  log "BEGIN"

  case "$TOPOLOGY" in
    pair) run_pair ;;
    line) run_line ;;
    throttle) run_throttle ;;
    star) run_star ;;
  esac

  log "END ok"
  schedule_line "result=ok"
}

main

