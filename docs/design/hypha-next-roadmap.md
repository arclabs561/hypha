---
status: proposal
scope: hypha next implementation sequence
grounded_in: ADR-0001, ADR-0002, ADR-0003, ADR-0004, ADR-0005, ADR-0006, docs/design/hypha-refinement-roadmap.md, docs/design/esp32-c6-power-measurement.md, docs/FLEET_POWER.md, firmware/README.md
review_trigger: revisit after ADR-0006 is accepted or rejected, after the first committed power measurement run, or before adding a new transport, trust boundary, or sleep role.
---

# Design: Hypha Next Roadmap

## Current Position

The repo is clean and CI is green. The recent work made the operator path more
introspectable: retained MQTT health is summarized, Healthchecks pings are
tested, power-summary validation exists, secure HTTP OTA manifests are signed
and verified in tests, and firmware docs distinguish WiFi, MQTT, ESP-NOW peers,
mesh delivery, and placement fingerprints.

The architectural state is less finished than the test state. ADR-0001 through
ADR-0005 are accepted guardrails. They deliberately downgrade peer scoring,
conductivity, firefly, and tasks to local heuristics or telemetry until their
missing mechanisms are added. ADR-0006 is still proposed, but much of its
lowest-risk implementation already exists: `EnergyStatus` can carry optional
facts, compute and storage capabilities use capacity matching, and tests cover
those semantics. The remaining coordination work is to settle the public
contract before widening the task API.

The power story has a protocol, not evidence. `docs/measurements/power/` has a
schema and validator but no committed run summaries. That means ESP32-C6 sleep,
RX-gating, LP-core, and battery-life claims remain blocked on measurement.

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

Work:

- Decide whether ADR-0006 is accepted as written or split into smaller ADRs.
- Name the local bidding contract. `evaluate_task` and `process_task_bundle`
  can coexist only if docs and names make their different roles explicit.
- Decide the sensing vocabulary path: closed enum, stable URI labels, or exact
  strings as an intentional prototype limit.
- Keep task/state causality out of scope unless a real consumer needs tasks to
  reference shared state.

Gate: ADR-0006 has accepted status, or a replacement ADR exists. Schema
compatibility tests and examples describe one local bidding contract instead of
two half-named auction stories.

Reversibility: partially reversible. Public schema changes are harder to
unwind than docs and tests.

## Phase 3: Capture Power Evidence

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

## Phase 4: Prove The Update Path

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

## Phase 5: Decide The Sleeper Role

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

Question: accept ADR-0006 as one ADR, or split it?

Recommended: accept ADR-0006 after tightening the text to reflect already-built
capacity matching and optional energy facts. Split later only if sensing
vocabulary or task/state causality becomes a real consumer requirement.

### OTA path

Question: prioritize HTTP OTA for XIAO/IDF boards or ESP-NOW mesh OTA?

Recommended: HTTP OTA first. It already has signed manifest support and health
telemetry. ESP-NOW mesh OTA remains useful, but its design doc still records
partial sender and receiver work.

### Power control

Question: implement modem-save/RX-gating now or measure first?

Recommended: measure first. ADR-0004 and the power measurement design both
block power-savings claims until current and delivery are measured.

## Guardrails

Do not start sleeper-node implementation until Phase 3 has baseline
measurements.

Do not expose task allocation as a distributed auction until ADR-0006 is
accepted and the examples name one local contract.

Do not treat retained MQTT health as liveness proof unless the payload reports
enough freshness fields to support that claim.

Do not add host-specific outlet, UPS, Healthchecks, or room facts to this repo.
Those belong in infra.
