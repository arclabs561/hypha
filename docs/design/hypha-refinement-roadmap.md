---
status: proposal
scope: hypha mechanism hardening
grounded_in: ADR-0001, README.md, ARCHITECTURE.md, docs/TOPOLOGY.md
perspective_inputs: out.md transcript
review_trigger: revisit after the first phase lands or before implementing any new routing, scoring, or LED semantics.
---

# Design: Hypha refinement roadmap

## Current Position

Hypha has a clearer public shape than the raw transcript assumed. Treat
`out.md` as an outside perspective that usefully points at possible weak spots,
not as authority. The README now describes power-aware coordination,
`ARCHITECTURE.md` separates the host crate from the embedded type surface, and
`docs/TOPOLOGY.md` already says topology is derived from capability and
metabolism rather than hardcoded as star or mesh. The firmware LED code is also
ahead of the critique: it is dark by default, has a small state vocabulary,
reserves magenta for locate, reserves amber/red style fault semantics, and
exposes rendered LED state in health telemetry.

The remaining issue is deeper than copy. Several public examples and internal
names still imply biological or distributed-systems guarantees that the code
does not yet earn. The transcript points to five families of work:

- mechanism claims: Physarum/conductivity, peer scoring, firefly sync, and
  pulse-gating need either the missing mechanisms or weaker names;
- semantic types: `EnergyStatus`, `Capability`, `Spike`, `Task`, and `Bid`
  compress distinct concepts into loose primitives;
- correctness planes: task messages and CRDT state do not share an explicit
  causality contract;
- measurement gaps: the eval suite emphasizes mesh-internal graph metrics, not
  energy per delivered observation, latency, partition behavior, or allocation
  regret;
- ESP32-C6 role design: the C6 LP core, LP I2C/UART, and sleep modes create a
  real sleeper-node path, but it should follow measurements and official API
  verification rather than becoming another unmeasured claim.

## Phase 1: Make Claims Boring

Progress 2026-06-18: the README no longer foregrounds the slime-mold
allocation output, `ARCHITECTURE.md` names the scoring/allocation/power
limitations, `docs/TOPOLOGY.md` uses operational input language instead of
"gradient computes" wording, and the allocation/synchrony examples now label
themselves as heuristic demos.

Consumer: readers, reviewers, and future agents changing the public API.

Gate: README, examples, and docs no longer lead with slime-mold allocation or
unqualified biological guarantees. Every strong claim points to a test,
measurement, or ADR.

Work:

1. Move `slime_mold_auction` out of the primary README path or rename it as a
   toy allocation heuristic.
2. Add limitations to `ARCHITECTURE.md` for peer scoring, task allocation,
   CRDT/task ordering, and evaluation metrics.
3. Audit public names that imply a proof: `conductivity`, `pheromone`,
   `mycelial gradient`, `Spike`, and `firefly` in docs and examples.

Reversibility: reversible. This is mostly documentation and naming hygiene.

## Phase 2: Measure The Power Story

Progress 2026-06-18: `docs/design/esp32-c6-power-measurement.md` defines the
bench matrix, required metadata, output shape, and gates. This does not satisfy
the measurement gate yet; it only defines the protocol for doing so.

Consumer: ESP32-C6 firmware and any future "power-aware" routing claim.

Gate: one repeatable bench reports current or energy for at least three states:
healthy idle, TX-only pulse behavior, and an RX-gated or sleep candidate. A
feature cannot claim savings until the bench shows them.

Work:

1. Add a power benchmark protocol to firmware docs: board, firmware version,
   radio mode, MQTT path, LED cap, sampling interval, and current measurement
   method.
2. Compare current behavior against a candidate RX-gate or modem-sleep mode.
3. Report energy per delivered observation alongside delivery and latency.

Reversibility: partially reversible. Measurement artifacts are cheap; firmware
power-mode changes need board validation.

## Phase 3: Decide The Security And Trust Model

Progress 2026-06-18: ADR-0002, ADR-0003, ADR-0004, and ADR-0005 are accepted
boundaries for peer scoring, conductivity, firefly, and task/state causality.

Consumer: anyone relying on mesh behavior in an open RF environment.

Gate: one ADR decides whether Hypha is hobbyist/cooperative only, or whether it
defends against malicious RF peers. Implementation follows that decision.

Work:

1. Decide whether peer scoring is removed, frozen as a local heuristic, or
   replaced with a GossipSub-like score with negative terms and thresholds.
2. Decide whether firefly pulses and Spike-like alerts must be signed.
3. If alerts remain, replace `Spike { intensity, pattern_id }` with a typed
   alert vocabulary and a quorum or trust rule before any action is taken.

Reversibility: partially reversible. Security surface changes affect message
formats and compatibility.

## Phase 4: Fix The Coordination Types

Progress 2026-06-18: ADR-0006 is proposed for splitting observed facts from
computed scores, replacing exact-equality capability matching, and selecting
one local bidding contract before public schema changes.

Progress 2026-06-18: capability capacity matching is implemented, the two local
bidding paths share the same energy/reach/capability gate, and `EnergyStatus`
now carries optional observed facts while preserving legacy JSON compatibility.

Consumer: task allocation, bridge code, and downstream applications consuming
observations.

Gate: task allocation has one algorithmic contract, one capability-matching
contract, and one causality boundary between task messages and shared state.

Work:

1. Split `EnergyStatus` into sender-observed facts and receiver-computed score:
   state of charge, mains flag, optional remaining capacity, and optional
   projected drain.
2. Replace exact-equality capability matching with typed capability semantics.
   `Compute(101)` must satisfy a `Compute(50)` requirement if compute remains a
   capacity.
3. Pick one auction behavior. Quorum-sensing and CBBA-style local best-bid
   logic should not both masquerade as the same coordination protocol.
4. Decide whether `yrs` state vectors or a separate app clock are authoritative
   for tasks that reference shared state.

Reversibility: mixed. Type changes are public API changes and should be batched
behind ADRs and migration tests.

## Phase 5: Design The ESP32-C6 Sleeper Role

Consumer: battery-powered Hypha boards.

Gate: official ESP-IDF references are checked for the exact C6 APIs, and a bench
shows the LP-core/sleep design improves useful lifetime for a named sensing
workflow.

Work:

1. Define the sleeper-node role: LP core samples a sensor, HP core wakes only
   when a threshold or reporting window fires, then MQTT/BLE/802.15.4 transmits
   and returns to sleep.
2. Document C6-specific constraints before code: LP-domain GPIO wake sources,
   no touch wake, LP memory limits, USB-serial-JTAG deep-sleep behavior, and
   WiFi 6 TWT dependence on AP support.
3. Keep the first implementation narrow: one sensor, one wake condition, one
   transport, one measurement harness.

Reversibility: partially reversible. Firmware experiments are cheap; a hardware
roster decision is not.

## Decision-Required ADR Stubs

### ADR-0002: Decide the peer scoring trust model

Governs: `src/core/mesh.rs`, `tests/gossip_storm.rs`, `docs/**`

Question: Is Hypha's peer score a local scheduling hint, a real adversarial
defense, or dead weight?

Options:

- Delete or freeze scoring as a local heuristic. Lowest complexity, weakest
  claim.
- Add signed energy attestation plus negative validation penalties and
  thresholds. More defensible, more protocol surface.
- Defer until a real hostile-peer deployment appears. Honest, but docs must
  drop security language.

Recommended: freeze as local heuristic until validation verdicts and signatures
exist.

### ADR-0003: Formalize or rename conductivity

Governs: `src/core/mesh.rs`, examples, eval docs

Question: Is conductivity a real flow variable or just usage history?

Options:

- Implement a Physarum-like flow model with decay, edge cost, pressure
  differential, and conservation.
- Rename it to `usage_ewma` or a pressure heuristic and drop Physarum claims.
- Keep the name and add caveats.

Recommended: rename unless a small Laplacian or flow-model spike proves cheap
and useful.

### ADR-0004: Decide the firefly role

Governs: `crates/hypha-firefly/**`, `firmware/hypha_esp_c6_idf/src/firefly.rs`,
`firmware/hypha_esp_c6_idf/src/led.rs`

Question: Is firefly sync an ambient telemetry feature, a radio power-control
primitive, or a security-relevant clock?

Options:

- Keep it as visual telemetry only. Lowest risk, matches current LED design.
- Use it for RX gating after current measurements prove savings and latency is
  acceptable.
- Treat it as a control clock and sign/quorum pulses before relying on it.

Recommended: keep as telemetry until the power bench and threat model both pass.

### ADR-0005: Decide task/state causality

Governs: `src/sync.rs`, task and bid types, examples

Question: Which plane is authoritative when a task references shared state?

Options:

- Tasks carry a `yrs` state-vector floor.
- Tasks carry an application-level sequence or epoch.
- Tasks cannot reference shared-state versions until a real consumer needs it.

Recommended: defer broad coordination until a consumer appears, but document the
current absence as a limitation now.

## Non-goals

- Do not implement the C6 sleeper path before the measurement harness exists.
- Do not add a new routing algorithm just to preserve the Physarum name.
- Do not make LEDs carry arbitrary debug telemetry. Keep the vocabulary small.
- Do not treat `out.md` as authoritative research. It is a triage source; claims
  that drive code need official docs, papers, or measurements.
- Do not broaden Hypha into a full distributed scheduler until a deployment has
  a real task-placement consumer.

## Next Action

Start with Phase 1. It is reversible, aligns public claims with the code, and
creates the review surface needed before any protocol or firmware changes.
