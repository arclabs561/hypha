#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d -t hypha-http-ota-test.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

BIN="$TMP/firmware.bin"
KEY="$ROOT/firmware/mesh_ota/keys/priv.pem"

printf 'test firmware image' >"$BIN"

bash "$ROOT/scripts/sign_http_ota.sh" "$BIN" "$KEY" "9.9.9" "$TMP/out" >/dev/null 2>&1

test -f "$TMP/out/firmware.bin.manifest.json"
test -f "$TMP/out/firmware.bin.sig"
test -f "$TMP/out/pubkey.hex"

RUSTC_WRAPPER= cargo run --quiet --manifest-path "$ROOT/firmware/mesh_ota/Cargo.toml" -- \
  --verify \
  --manifest "$TMP/out/firmware.bin.manifest.json" \
  --pubkey "$TMP/out/pubkey.hex" >/dev/null 2>&1

grep -q '"v": "9.9.9"' "$TMP/out/firmware.bin.manifest.json"

printf 'http ota signing wrapper: ok\n'
