/offline/ {
  state = $0
  sub(/^[[:space:]]*[0-9.]+[[:space:]]+/, "", state)
  if (state ~ /^[^[:space:]]+-initrd[[:space:]]/) {
    print "standby: " state
    standby++
    next
  }
  print "offline: " state
  offline++
}

END {
  if (offline == 0) {
    print "ok: no non-initrd offline peers reported"
  }
  if (standby > 0) {
    print "note: initrd peers are fallback boot identities"
  }
}
