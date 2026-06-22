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
cleanup() {
  rm -f "$payloads"
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
    done <<<"$rows"
  else
    printf '%-18s %-18s %-5s %-4s %s\n' none "" "" "" "no-direct-peer-adverts"
  fi
else
  printf '%-18s %-18s %-5s %-4s %s\n' none "" "" "" "no-ble-payloads"
fi

exit "$status"
