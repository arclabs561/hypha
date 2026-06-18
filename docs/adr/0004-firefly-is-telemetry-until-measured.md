---
id: 0004
status: proposed
governs: crates/hypha-firefly/**, firmware/hypha_esp_c6_idf/src/firefly.rs, firmware/hypha_esp_c6_idf/src/mqtt.rs, firmware/hypha_esp_c6_idf/src/led.rs, docs/**
why: Hypha currently uses firefly pulses for visible liveness and synchronization diagnostics; pulses are not authenticated and do not gate radio receive or sleep behavior.
rejected: treating firefly phase as a security-relevant clock; claiming radio power savings from TX-only pulse behavior; making firefly the main ambient LED language.
supersedes: none
superseded_by: none
extends: 0001, 0002
confidence: medium
review_trigger: revisit before using phase to control radio power, before signing or quorum-checking pulses, or before making firefly sync a public power/security claim.
---

# ADR-0004: firefly is telemetry until measured

**Status**: Proposed
**Date**: 2026-06-18
**Deciders**: arc

## Context

Hypha has two firefly implementations: the `hypha-firefly` no-std crate and the
ESP-IDF firmware port. Both implement a pulse-coupled oscillator with a
refractory period. In the IDF firmware, each board publishes its board id to
`hypha/sync/pulse` when the oscillator fires, and other boards increment a peer
pulse counter when they see a pulse from a different board.

That behavior is useful as telemetry. A synchronized flash or watch diagnostic
shows that the boards, MQTT bus, and coupling loop are alive. The current LED
design also keeps healthy boards dark by default and exposes explicit LED
states in health telemetry, so firefly is no longer the only visual language.

The same behavior is not yet a radio power-control primitive. The firmware does
not turn receive off during refractory windows, does not record an energy delta
from phase behavior, and does not state an urgent-message latency budget. It is
also not a security-relevant clock. Pulses are plain MQTT payloads containing a
board id, and receivers trust the broker topic rather than verifying signed
pulse sources or quorum.

## Decision

Hypha treats firefly synchronization as telemetry and diagnostics only until two
separate gates pass:

1. A power gate shows measured energy savings from a radio power mode tied to
   phase, including delivery and urgent-message latency.
2. A trust gate decides whether pulses need signatures, source allowlists,
   quorum behavior, or no adversarial protection at all.

Before those gates pass, firefly phase must not be used as a security boundary,
an authorization signal, or evidence of power savings. It may continue to drive
LED heartbeat overlays, fleet watch diagnostics, and local synchronization
experiments.

## Options considered

Keep firefly as telemetry. Chosen for now. It matches the current firmware,
keeps the visible behavior useful for debugging, and avoids coupling radio
power or security semantics to an unauthenticated signal.

Use firefly phase for RX gating now. Deferred. That path may be valuable, but it
needs the ESP32-C6 power measurement protocol to show a useful current delta and
it needs a latency budget for alerts or control traffic.

Treat firefly as a security-relevant clock. Rejected for the current design.
The pulse channel does not authenticate sources or bound one bad participant's
influence, so using it as a clock for trusted behavior would create a false
security boundary.

Remove firefly entirely. Rejected for now because the synchronized pulse is a
useful diagnostic and does not carry much complexity when scoped as telemetry.

## Consequences

Firmware docs should describe firefly as a liveness and coupling diagnostic.
Power docs should route through `docs/design/esp32-c6-power-measurement.md`
before claiming savings. Security docs should not imply that synchronized phase
means peers are trusted.

If a later implementation ties phase to Wi-Fi modem sleep, automatic light
sleep, or any receive gate, it must commit the measurement artifact and note the
worst-case latency for urgent messages. If a later implementation treats pulses
as adversarially relevant, it must specify source identity, signatures or
broker-side trust, and the rule for accepting or ignoring pulse input.

This ADR does not block oscillator tests, LED diagnostics, or watch tooling. It
only blocks stronger claims and control uses until the missing gates exist.

## Lineage

Extends ADR-0001's evidence rule and ADR-0002's trust-boundary rule. Firefly
power control and firefly pulse trust may later split into separate ADRs if one
is accepted without the other.
