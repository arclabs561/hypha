# Fleet power recovery

This file records the operational contract for Hypha hosts during house power
events. Deployment-specific secrets, outlet labels, and exact room placement
belong in infra; this repo keeps the generic checks and the current evidence
classification.

## Current incident reading

The June 2026 event appears to contain at least two separate failures.

- `arcanine`, `omastar`, and `starmie` all lost Ethernet carrier around
  `2026-06-19 03:06-03:08`, then recovered DHCP without rebooting. That is a
  network path event: switch, router, UPS output feeding network gear, or cabling.
- The same three NUCs have previous journal entries ending around
  `2026-06-19 08:51:05-08:51:07` with no shutdown markers. They later booted
  around `2026-06-20 08:00`. That is consistent with hard power removal or a hard
  cutoff on their UPS-backed power path.
- `dratini` continued past the 03:06 network event and stopped around
  `2026-06-19 10:21:23`, also without a clean shutdown marker. It booted again
  when manually powered on.
- `metagross` booted around `2026-06-19 10:52`, consistent with manual recovery
  after being off downstairs.
- `snorlax` remained up for more than 11 days, so the failure was not fleet-wide.

The current logs do not prove whether the NUC UPS battery exhausted, the UPS
output dropped, a PDU or strip was switched off, or the NUCs are on a different
outlet path than expected. That requires UPS telemetry and a checked outlet map.

## Runtime checks

Run this from `charizard` or another machine with SSH and Tailscale access:

```bash
just fleet-power-doctor
```

The doctor prints, per host:

- current boot ID and uptime
- recent boot history
- previous journal-tail shutdown markers, when present
- previous journal tail
- recent network link-loss evidence
- whether common UPS clients (`upsc`, `apcaccess`) or UPS services are present
- Wake-on-LAN and AC-restore evidence (`ethtool` on Linux, `pmset` on macOS)

Interpretation:

- Link-loss entries without a reboot point at network gear or Ethernet path.
- A previous boot with no shutdown markers and a later manual boot points at hard
  power loss or forced cutoff.
- Multiple hosts stopping within seconds are one power failure domain until
  proved otherwise.
- Wake-on-LAN only helps after the network path returns and another host is up
  to send the packet. AC-restore/auto-restart is the stronger setting for hosts
  that should come back without another machine intervening.

## Healthchecks

Hosts should ping Healthchecks.io at boot and on a timer. The runtime script
expects a pre-rendered env file, not a live `op` call:

```bash
HEALTHCHECKS_URL=https://hc-ping.com/<uuid> just healthchecks-ping start
HEALTHCHECKS_URL=https://hc-ping.com/<uuid> just healthchecks-ping
```

Install shape for Linux hosts:

```ini
[Service]
Type=oneshot
EnvironmentFile=/etc/hypha/healthchecks.env
ExecStart=/path/to/hypha/scripts/healthchecks_ping.sh
```

```ini
[Timer]
OnBootSec=2min
OnUnitActiveSec=5min
Persistent=true
```

For boot starts, run the same service once with `ExecStart=.../healthchecks_ping.sh
start`.

## Secrets

The ping URL is a secret. Provision it into a `0600` env file before boot-time
services run:

```bash
umask 077
op inject -i healthchecks.env.tpl -o healthchecks.env
```

1Password CLI is acceptable for interactive provisioning. It is not a runtime
dependency for launchd, cron, or systemd jobs that must recover unattended after a
power event. The official 1Password CLI docs describe `op run` for loading
secret references into a subprocess environment and `op inject` for resolving
templated config files; use those at provisioning time for this path.

## Power topology data to keep in infra

Each host should have these facts recorded in the private infra repo:

- host name and role
- room and physical outlet
- UPS or non-UPS power path
- switch or network path
- expected restore behavior after AC returns
- Wake-on-LAN support and MAC address
- Healthchecks.io check name
- expected RTO

For the NUC group, the next physical audit should verify that `arcanine`,
`omastar`, and `starmie` are actually on the battery-backed UPS outlets, that
the switch/router path is also UPS-backed, and that BIOS AC-recovery is set to
power on.
