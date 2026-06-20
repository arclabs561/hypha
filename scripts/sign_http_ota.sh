#!/usr/bin/env bash
# Sign an ESP-IDF HTTP OTA image and place the manifest where firmware fetches it.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  printf 'usage: sign_http_ota.sh <firmware.bin> <private-key> [version] [out-dir]\n' >&2
  printf 'writes: <out-dir>/<firmware.bin>.manifest.json, <out-dir>/<firmware.bin>.sig, <out-dir>/pubkey.hex\n' >&2
}

[ "$#" -ge 2 ] || { usage; exit 2; }

BIN="$1"
KEY="$2"
VERSION="${3:-}"
OUT_DIR="${4:-}"

[ -f "$BIN" ] || { printf 'error: firmware image not found: %s\n' "$BIN" >&2; exit 1; }
[ -f "$KEY" ] || { printf 'error: signing key not found: %s\n' "$KEY" >&2; exit 1; }

if [ -z "$VERSION" ]; then
  VERSION="$(awk -F\" '/^version =/ {print $2; exit}' "$ROOT/firmware/hypha_esp_c6_idf/Cargo.toml")"
fi
[ -n "$VERSION" ] || { printf 'error: could not determine firmware version\n' >&2; exit 1; }

if [ -z "$OUT_DIR" ]; then
  OUT_DIR="$(cd "$(dirname "$BIN")" && pwd -P)"
fi
mkdir -p "$OUT_DIR"

TMP="$(mktemp -d -t hypha-http-ota-sign.XXXXXX)"
cleanup() {
  rm -rf "$TMP"
}
trap cleanup EXIT

(
  cd "$ROOT"
  RUSTC_WRAPPER= cargo run --quiet --manifest-path "$ROOT/firmware/mesh_ota/Cargo.toml" -- \
    --bin "$BIN" \
    --version "$VERSION" \
    --key "$KEY" \
    --out-dir "$TMP" >/dev/null
)

base="$(basename "$BIN")"
install -m 0644 "$TMP/manifest.json" "$OUT_DIR/$base.manifest.json"
install -m 0644 "$TMP/firmware.sig" "$OUT_DIR/$base.sig"
install -m 0644 "$TMP/pubkey.hex" "$OUT_DIR/pubkey.hex"

printf 'signed %s as version %s\n' "$BIN" "$VERSION"
printf 'manifest: %s\n' "$OUT_DIR/$base.manifest.json"
