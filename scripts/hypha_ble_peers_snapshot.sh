#!/usr/bin/env bash
# Summarize direct Hypha board sightings from hypha/<board>/ble MQTT payloads.

set -euo pipefail

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

json_from_line() {
  local line=$1
  if [[ $line == \{* ]]; then
    printf '%s\n' "$line"
    return
  fi
  printf '%s\n' "${line#* }"
}

need_cmd jq

payloads="$(mktemp -t hypha-ble-peers.XXXXXX)"
observed_sources="$(mktemp -t hypha-ble-sources.XXXXXX)"
observed_peers="$(mktemp -t hypha-ble-peers-seen.XXXXXX)"
observed_edges="$(mktemp -t hypha-ble-edges.XXXXXX)"
cleanup() {
  rm -f "$payloads" "$observed_sources" "$observed_peers" "$observed_edges"
}
trap cleanup EXIT

status=0
while IFS= read -r line; do
  [[ -n $line ]] || continue
  topic=""
  if [[ $line != \{* ]]; then
    topic="${line%% *}"
  fi
  json=$(json_from_line "$line")
  if ! compact="$(
    jq -c --arg topic "$topic" '
      def topic_board:
        (try ($topic | capture("^hypha/(?<board>[^/]+)/ble$").board) catch "");
      if ((.board // "") == "") and topic_board != ""
      then . + {board: topic_board}
      else .
      end
    ' <<<"$json" 2>/dev/null
  )"; then
    printf 'warn: skipped malformed ble line: %s\n' "$line" >&2
    status=1
    continue
  fi
  printf '%s\n' "$compact" >>"$payloads"
done < <(if [[ $# -gt 0 ]]; then cat "$@"; else cat; fi)

printf '%-18s %-18s %-5s %-4s %s\n' source peer rssi seen notes

if [[ -s $payloads ]]; then
  rows="$(
    jq -s -r '
      [
        .[]
        | .board as $source
        | (.adverts // [])[]
        | select((.peer // "") != "" and (.peer // "") != $source)
        | {
            source: $source,
            peer: .peer,
            rssi: (.r // -127)
          }
      ]
      | sort_by(.source, .peer)
      | group_by(.source, .peer)
      | .[]
      | {
          source: .[0].source,
          peer: .[0].peer,
          rssi: (map(.rssi) | max),
          seen: length
        }
      | [.source, .peer, (.rssi | tostring), (.seen | tostring),
         (if .rssi < -85 then "weak-direct-rssi" else "direct" end)]
      | @tsv
    ' "$payloads"
  )"
  if [[ -n $rows ]]; then
    while IFS=$'\t' read -r source peer rssi seen notes; do
      printf '%-18s %-18s %-5s %-4s %s\n' "$source" "$peer" "$rssi" "$seen" "$notes"
      printf '%s\n' "$source" >>"$observed_sources"
      printf '%s\n' "$peer" >>"$observed_peers"
      printf '%s %s\n' "$source" "$peer" >>"$observed_edges"
    done <<<"$rows"
  else
    printf '%-18s %-18s %-5s %-4s %s\n' none "" "" "" "no-direct-peer-adverts"
  fi
else
  printf '%-18s %-18s %-5s %-4s %s\n' none "" "" "" "no-ble-payloads"
fi

if [[ -n ${HYPHA_EXPECTED_BOARDS:-} ]]; then
  expected="${HYPHA_EXPECTED_BOARDS//,/ }"
  expected_file="$(mktemp -t hypha-ble-expected.XXXXXX)"
  reached_file="$(mktemp -t hypha-ble-reached.XXXXXX)"
  trap 'rm -f "$payloads" "$observed_sources" "$observed_peers" "$observed_edges" "$expected_file" "$reached_file"' EXIT
  for board in $expected; do
    [[ -n $board ]] || continue
    printf '%s\n' "$board" >>"$expected_file"
    if ! grep -Fxq "$board" "$observed_sources"; then
      heard_by="$(
        awk -v board="$board" '$2 == board && !seen[$1]++ { print $1 }' "$observed_edges" \
          | paste -sd, -
      )"
      note="no-direct-out"
      if [[ -n $heard_by ]]; then
        note="${note},heard-by=${heard_by}"
      fi
      printf '%-18s %-18s %-5s %-4s %s\n' "$board" "" "" "0" "$note"
      [[ -n ${HYPHA_REQUIRE_DIRECT:-} ]] && status=2
    fi
    if ! grep -Fxq "$board" "$observed_peers"; then
      hears="$(
        awk -v board="$board" '$1 == board && !seen[$2]++ { print $2 }' "$observed_edges" \
          | paste -sd, -
      )"
      note="not-directly-heard"
      if [[ -n $hears ]]; then
        note="${note},hears=${hears}"
      fi
      printf '%-18s %-18s %-5s %-4s %s\n' "none" "$board" "" "0" "$note"
      [[ -n ${HYPHA_REQUIRE_DIRECT:-} ]] && status=2
    fi
  done

  if [[ -n ${HYPHA_REQUIRE_DIRECT:-} && -s $expected_file ]]; then
    head -n 1 "$expected_file" >"$reached_file"
    changed=1
    while [[ $changed -eq 1 ]]; do
      changed=0
      while read -r source peer; do
        grep -Fxq "$source" "$expected_file" || continue
        grep -Fxq "$peer" "$expected_file" || continue
        if grep -Fxq "$source" "$reached_file" && ! grep -Fxq "$peer" "$reached_file"; then
          printf '%s\n' "$peer" >>"$reached_file"
          changed=1
        fi
        if grep -Fxq "$peer" "$reached_file" && ! grep -Fxq "$source" "$reached_file"; then
          printf '%s\n' "$source" >>"$reached_file"
          changed=1
        fi
      done <"$observed_edges"
    done
    while read -r board; do
      if ! grep -Fxq "$board" "$reached_file"; then
        printf '%-18s %-18s %-5s %-4s %s\n' "none" "$board" "" "0" "direct-graph-partition"
        status=2
      fi
    done <"$expected_file"
  fi
fi

exit "$status"
