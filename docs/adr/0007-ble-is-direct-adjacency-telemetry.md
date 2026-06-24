---
id: 0007
status: accepted
governs: firmware/hypha_esp_c6_idf/src/ble.rs, firmware/hypha_esp_c6_idf/src/mqtt.rs, firmware/README.md, docs/design/hypha-next-roadmap.md, scripts/hypha_ble_peers_snapshot.sh, scripts/mesh_doctor.sh
why: Hypha's current XIAO/C6 BLE implementation advertises a compact self marker, scans nearby markers, and reports direct sightings through MQTT; it does not run Bluetooth Mesh, relay BLE messages, or exchange neighbor summaries over BLE.
rejected: calling the current manufacturer-data marker Bluetooth Mesh; silently adopting ESP-BLE-MESH without a provisioning/security/role decision; adding BLE-carried neighbor summaries before the direct-adjacency diagnostic baseline is reliable.
supersedes: none
superseded_by: none
extends: 0001, 0004
confidence: high
review_trigger: revisit before changing the BLE manufacturer-data payload, before adding BLE-carried neighbor summaries, before using BLE as a routed transport, or before enabling ESP-BLE-MESH.
---

# ADR-0007: BLE is direct-adjacency telemetry

**Status**: Accepted
**Date**: 2026-06-24
**Deciders**: arc

## Context

The XIAO/IDF firmware currently uses BLE for local observation. Each board
advertises a small Hypha manufacturer-data marker containing its board suffix.
Other boards passively scan for those markers, aggregate the strongest RSSI per
window, and publish the resulting direct sightings to `hypha/<board>/ble` over
MQTT.

That gives useful placement and RF-adjacency telemetry. It can answer questions
such as "who directly heard this board" and "which board is publishing its own
BLE window." It does not provide routed BLE delivery, store-carry-forward
behavior, Bluetooth Mesh provisioning, mesh keys, friend/low-power buffering,
proxy access from phones, or standardized models.

The ESP-IDF ESP-BLE-MESH stack solves a broader problem. Its documentation
describes provisioning, NetKey/AppKey configuration, relay, friend, low-power,
proxy, segmentation/reassembly, network management, and model layers. The
Bluetooth SIG overview frames Bluetooth Mesh as a full networking layer with
publish/subscribe addressing, managed flooding or directed forwarding, and
mandatory security. Those are real mechanisms, not just a different
advertisement payload.

ADR-0001 requires mechanism claims to match the implementation. ADR-0004 also
keeps firefly and related board-to-board signals as telemetry until power and
trust gates are measured and decided. The BLE path needs the same boundary.

## Decision

Hypha treats the current BLE path as direct-adjacency telemetry only.

The existing manufacturer-data marker may continue to identify a board to
nearby Hypha scanners, and `just mesh-doctor` may summarize those direct
sightings. Docs, diagnostics, and examples must not describe this as Bluetooth
Mesh or routed BLE delivery.

There are two future forks, and either one needs an explicit design decision
before firmware payload semantics change:

- A Hypha-specific neighbor-summary frame: compact manufacturer-data or scan
  response payloads with self id, sequence, age, TTL, bounded recent-peer
  summaries, and duplicate suppression.
- ESP-BLE-MESH adoption: provisioning, keys, models, relay/friend/proxy roles,
  and the ESP-IDF/Bluetooth Mesh lifecycle.

Until one fork is accepted, the BLE payload should stay small and diagnostic.
MQTT remains the current shared view for BLE observations.

## Options considered

Keep the current marker and name it direct-adjacency telemetry. Chosen. It
matches the implementation, keeps the operator diagnostic useful, and avoids
importing Bluetooth Mesh claims that the code does not satisfy.

Add Hypha neighbor summaries to manufacturer-data beacons now. Deferred. This
could be the smallest next step toward brokerless peer sharing, but it needs a
versioned payload, sequence and age rules, duplicate suppression, and tests that
distinguish direct, second-hand, and stale observations.

Adopt ESP-BLE-MESH now. Deferred. It is the right tool if Hypha needs standard
provisioning, phone proxy access, friend/low-power buffering, or routed BLE
delivery. It also changes firmware roles, keys, provisioning, storage, and
security posture enough to deserve its own ADR and bench work.

Call the current path Bluetooth Mesh because it uses BLE advertisements.
Rejected. ESP-BLE-MESH and Bluetooth Mesh are networking stacks with
provisioning, security material, relay behavior, and models. A Hypha
manufacturer-data marker plus MQTT reporting is not that stack.

## Consequences

Operator tools can keep improving around the current signal. For example, a
missing outbound BLE report can show `heard-by=...` when other boards observed
the board, while a missing inbound report can show `hears=...` when the board's
own feed contains peers that did not report it.

Firmware changes that add second-hand peer facts must carry their own
compatibility story. The current direct sighting means "this board heard that
board." A neighbor summary would mean "this board claims it heard that board at
some earlier time." Those are different facts and must not be collapsed into one
edge.

ESP-BLE-MESH remains available, but adopting it should be justified by a need
that the current telemetry path cannot satisfy. The likely triggers are
brokerless routed delivery, phone provisioning/proxy behavior, standard mesh
models, or low-power friend buffering.

## Lineage

Extends ADR-0001 by keeping the BLE name tied to the implemented mechanism.
Extends ADR-0004 by keeping board-to-board timing and RF signals in the
telemetry bucket until stronger power or trust claims are measured and designed.
