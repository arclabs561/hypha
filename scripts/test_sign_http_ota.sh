#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d -t hypha-http-ota-test.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

BIN="$TMP/firmware.bin"
KEY="$TMP/key.hex"

printf 'test firmware image' >"$BIN"
printf '%064d\n' 0 >"$KEY"

(
  cd "$ROOT/firmware/hypha_esp_c6_idf"
  bash "$ROOT/scripts/sign_http_ota.sh" "$BIN" "$KEY" "9.9.9" "$TMP/out" >/dev/null
)

test -f "$TMP/out/firmware.bin.manifest.json"
test -f "$TMP/out/firmware.bin.sig"
test -f "$TMP/out/pubkey.hex"

RUSTC_WRAPPER= cargo run --quiet --manifest-path "$ROOT/firmware/mesh_ota/Cargo.toml" -- \
  --pubkey-from-key \
  --key "$KEY" \
  --out-dir "$TMP/derived" >/dev/null 2>&1

cmp "$TMP/out/pubkey.hex" "$TMP/derived/pubkey.hex"

RUSTC_WRAPPER= cargo run --quiet --manifest-path "$ROOT/firmware/mesh_ota/Cargo.toml" -- \
  --verify \
  --manifest "$TMP/out/firmware.bin.manifest.json" \
  --pubkey "$TMP/out/pubkey.hex" >/dev/null 2>&1

grep -q '"v": "9.9.9"' "$TMP/out/firmware.bin.manifest.json"

printf 'http ota signing wrapper: ok\n'
