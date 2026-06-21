#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d -t hypha-healthchecks.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/hostname" <<'SH'
#!/usr/bin/env bash
printf 'charizard\n'
SH

cat >"$TMP/uname" <<'SH'
#!/usr/bin/env bash
printf 'Darwin\n'
SH

cat >"$TMP/sysctl" <<'SH'
#!/usr/bin/env bash
if [[ $* == '-n kern.boottime' ]]; then
  printf '{ sec = 1000, usec = 0 } Sun Jun 21 00:00:00 2026\n'
else
  exit 2
fi
SH

cat >"$TMP/date" <<'SH'
#!/usr/bin/env bash
if [[ ${1:-} == '+%s' ]]; then
  printf '1123\n'
else
  exit 2
fi
SH

cat >"$TMP/curl" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

payload=""
url=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --data-binary)
      payload="$2"
      shift 2
      ;;
    -*)
      shift
      ;;
    *)
      url="$1"
      shift
      ;;
  esac
done

printf '%s\n' "$url" >"$HYPHA_TEST_CURL_URL"
printf '%s' "$payload" >"$HYPHA_TEST_CURL_PAYLOAD"
SH

chmod 0755 "$TMP/hostname" "$TMP/uname" "$TMP/sysctl" "$TMP/date" "$TMP/curl"

if HEALTHCHECKS_URL="" PATH="$TMP:$PATH" bash "$ROOT/scripts/healthchecks_ping.sh" 2>"$TMP/missing.err"; then
  echo "expected missing HEALTHCHECKS_URL to fail" >&2
  exit 1
fi
grep -q 'HEALTHCHECKS_URL is required' "$TMP/missing.err"

if HEALTHCHECKS_URL="https://hc-ping.com/uuid" PATH="$TMP:$PATH" bash "$ROOT/scripts/healthchecks_ping.sh" bad 2>"$TMP/mode.err"; then
  echo "expected invalid mode to fail" >&2
  exit 1
fi
grep -q 'mode must be one of' "$TMP/mode.err"

OUT="$(
  HYPHA_TEST_CURL_URL="$TMP/url" \
    HYPHA_TEST_CURL_PAYLOAD="$TMP/payload" \
    HEALTHCHECKS_URL="https://hc-ping.com/uuid/" \
    PATH="$TMP:$PATH" \
    bash "$ROOT/scripts/healthchecks_ping.sh" start
)"

grep -q 'ok: pinged start for charizard' <<<"$OUT"
grep -qx 'https://hc-ping.com/uuid/start' "$TMP/url"
grep -q '^host=charizard$' "$TMP/payload"
grep -q '^os=Darwin$' "$TMP/payload"
grep -q '^boot_id=unknown$' "$TMP/payload"
grep -q '^uptime_s=123$' "$TMP/payload"

printf 'healthchecks ping wrapper: ok\n'
