---
status: proposal
scope: hypha next implementation sequence
grounded_in: ADR-0001, ADR-0002, ADR-0003, ADR-0004, ADR-0005, ADR-0006, docs/design/hypha-refinement-roadmap.md, docs/design/esp32-c6-power-measurement.md, docs/FLEET_POWER.md, firmware/README.md
review_trigger: revisit after the ADR-0006 naming migration lands, after the first committed power measurement run, or before adding a new transport, trust boundary, or sleep role.
---

# Design: Hypha Next Roadmap

## Current Position

The repo is clean and CI is green. The recent work made the operator path more
introspectable: retained MQTT health is summarized, Healthchecks pings are
tested, power-summary validation exists, secure HTTP OTA manifests are signed
and verified in tests, and firmware docs distinguish WiFi, MQTT, ESP-NOW peers,
mesh delivery, and placement fingerprints.

The architectural state is less finished than the test state. ADR-0001 through
ADR-0006 are accepted guardrails. They deliberately downgrade peer scoring,
conductivity, firefly, and tasks to local heuristics or telemetry until their
missing mechanisms are added. The lowest-risk ADR-0006 implementation already
exists: `EnergyStatus` can carry optional facts, compute and storage
capabilities use capacity matching, and tests cover those semantics. The
remaining coordination work is to keep the public contract narrow before
widening the task API.

The power story has a protocol, not evidence. `docs/measurements/power/` has a
schema and validator but no committed run summaries. That means ESP32-C6 sleep,
RX-gating, LP-core, and battery-life claims remain blocked on measurement.

The BLE story is direct-observation telemetry, not routed BLE mesh. The
XIAO/IDF boards advertise a compact Hypha manufacturer-data marker, scan nearby
markers, and publish the resulting direct adjacency view through MQTT. They do
not yet exchange neighbor summaries over BLE, relay Hypha messages over BLE, or
run ESP-BLE-MESH provisioning/models/friend/proxy behavior.

Fleet power recovery is split correctly: Hypha keeps generic scripts and docs,
while host-specific room, outlet, UPS, Wake-on-LAN, Healthchecks, and RTO facts
belong in the private infra repo.

## Phase 1: Close The Operator Loop

Consumer: the person debugging Hypha from `charizard` after a board, broker, or
power event fails.

Work:

- Keep `just mesh-doctor` as the first-line board diagnostic and make each
  warning actionable.
- Keep `just fleet-power-doctor` as the first-line host diagnostic and move
  private host facts into infra rather than this repo.
- Add only generic tests here: parsing, env handling, and failure
  classification.

Gate: from one machine, without opening serial monitors by hand, we can answer:
which boards have fresh retained health, which are legacy, which firmware is
behind the signed OTA version, whether MQTT is reachable, and which hosts need
power-path attention.

Reversibility: reversible. This is scripts, tests, and docs.

## Phase 2: Settle ADR-0006

Consumer: task allocation, examples, bridge code, and future firmware messages.

Progress 2026-06-21: ADR-0006 is accepted. Code now exposes explicit
`evaluate_task_with_quorum` and `process_task_bundle_best_bid` names while
keeping compatibility wrappers for the old method names.

Work:

- Name the local bidding contract. `evaluate_task` and `process_task_bundle`
  can coexist only if docs and names make their different roles explicit.
- Decide the sensing vocabulary path: closed enum, stable URI labels, or exact
  strings as an intentional prototype limit.
- Keep task/state causality out of scope unless a real consumer needs tasks to
  reference shared state.

Gate: schema compatibility tests and examples describe the current task plane
as local advisory heuristics, not a distributed auction. Any new public task
schema change cites ADR-0006 or a superseding ADR.

Reversibility: partially reversible. Public schema changes are harder to
unwind than docs and tests.

## Phase 3: Define Direct-RF Peer Sharing

Consumer: board placement diagnostics and any future store-carry-forward path
that needs peer facts when the MQTT broker is unavailable.

Progress 2026-06-24: `just mesh-doctor` summarizes direct BLE in/out sightings
from `hypha/<board>/ble`, and missing expected boards now show whether they were
heard by others or only heard others.

Work:

- Keep the current MQTT-shared BLE observations as the diagnostic baseline.
- Decide whether the next transport step is a lighter Hypha manufacturer-data
  neighbor-summary frame or ESP-BLE-MESH with provisioning, keys, models, relay,
  friend, and proxy roles.
- If Hypha keeps manufacturer-data beacons, define a compact versioned payload:
  self id, sequence, age, TTL, and a bounded recent-peer summary with duplicate
  suppression.
- If Hypha adopts ESP-BLE-MESH, write an ADR first. That stack solves a broader
  problem than current adjacency telemetry and changes provisioning, security,
  and firmware surface area.

Gate: a design note or ADR states whether Hypha is building direct-adjacency
telemetry, BLE-carried neighbor summaries, or routed BLE delivery. Tests must
distinguish those three cases.

Reversibility: mixed. MQTT-side diagnostics are cheap. Firmware message-format
changes and ESP-BLE-MESH provisioning are compatibility commitments.

## Phase 4: Capture Power Evidence

Consumer: ESP32-C6 firmware, sleeper-node design, and any future power claim.

Work:

- Commit sanitized baseline and dark-baseline measurement summaries for at
  least one C6 board.
- Measure current behavior before adding RX gating, modem-save, deep sleep, or
  LP-core code.
- Report energy per delivered observation, not just idle current.

Gate: at least two validated JSON summaries exist in
`docs/measurements/power/`: baseline active and dark baseline for the same
board and firmware SHA.

Reversibility: partially reversible. Measurement files are cheap; firmware
power-mode changes are not.

## Phase 5: Prove The Update Path

Consumer: deployed C6 boards that should recover without USB flashing.

Work:

- Keep HTTP OTA as the near-term path for XIAO/IDF boards.
- Use retained health to prove each board reports `ota_state`, `ota_checks`,
  `ota_failures`, firmware version, and boot ID.
- Treat ESP-NOW mesh OTA as a separate experiment until its sender chunk
  serving, receiver stream-to-flash, and end-to-end boot path are proven.

Gate: one board updates from signed HTTP OTA and reports the new firmware
version and sane OTA health afterward. For ESP-NOW mesh OTA, the gate is an
end-to-end device update, not only host-side protocol tests.

Reversibility: partially reversible. Bad OTA behavior can strand boards.

## Phase 6: Decide The Sleeper Role

Consumer: battery-powered C6 nodes.

Work:

- Pick one named workflow: one sensor, one wake condition, one transport.
- Check the exact ESP32-C6 APIs and constraints before code: LP-domain GPIO,
  LP memory, LP I2C/UART, USB-serial-JTAG deep-sleep behavior, and WiFi power
  save limits.
- Implement only after Phase 3 shows the active baseline and the target role is
  measurably different.

Gate: a short ADR or design note chooses the sleeper workflow and cites the
power measurements it expects to improve.

Reversibility: partially reversible. Firmware experiments are cheap; hardware
and deployment promises are not.

## Decision-Required Forks

### Coordination contract

Question: keep ADR-0006 as one contract, or split sensing vocabulary and
task/state causality later?

Recommended: keep ADR-0006 as the current contract. Split later only if sensing
vocabulary or task/state causality becomes a real consumer requirement.

### OTA path

Question: prioritize HTTP OTA for XIAO/IDF boards or ESP-NOW mesh OTA?

Recommended: HTTP OTA first. It already has signed manifest support and health
telemetry. ESP-NOW mesh OTA remains useful, but its design doc still records
partial sender and receiver work.

### BLE peer sharing

Question: add compact Hypha neighbor summaries to the existing BLE marker or
adopt ESP-BLE-MESH?

Recommended: keep the current manufacturer-data marker for direct adjacency and
add a Hypha-specific neighbor-summary frame only after the diagnostics prove the
need. ESP-BLE-MESH is the right fork if Hypha needs standard provisioning,
friend/low-power buffering, proxy access from phones, or routed BLE delivery.

### Power control

Question: implement modem-save/RX-gating now or measure first?

Recommended: measure first. ADR-0004 and the power measurement design both
block power-savings claims until current and delivery are measured.

## Guardrails

Do not start sleeper-node implementation until Phase 4 has baseline
measurements.

Do not expose task allocation as a distributed auction until the examples name
one local contract and a later ADR supersedes ADR-0005/ADR-0006 for distributed
coordination.

Do not treat retained MQTT health as liveness proof unless the payload reports
enough freshness fields to support that claim.

Do not describe direct BLE sightings as routed BLE mesh delivery.

Do not add host-specific outlet, UPS, Healthchecks, or room facts to this repo.
Those belong in infra.
