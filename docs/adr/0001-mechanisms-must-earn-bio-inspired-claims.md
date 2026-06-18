---
id: 0001
status: accepted
governs: README.md, ARCHITECTURE.md, docs/**, src/core/mesh.rs, crates/hypha-core/**, crates/hypha-firefly/**, firmware/**
why: Hypha uses biological language for routing, scoring, signaling, and LEDs; if the implementation omits the mechanism that gives a biological model its behavior, the name becomes misleading design debt.
rejected: keeping metaphor-first names when the code is only an EWMA, weighted sum, or local heuristic; claiming power savings without current measurements; treating a visual pulse as useful telemetry without a state vocabulary.
supersedes: none
superseded_by: none
extends: none
confidence: high
review_trigger: revisit if Hypha adopts a formal model with equations, measurements, and tests that justify one of the previously downgraded biological claims.
---

# ADR-0001: mechanisms must earn bio-inspired claims

**Status**: Accepted
**Date**: 2026-06-18
**Deciders**: arc

## Context

Hypha deliberately borrows biological terms: spore, metabolism, mycelium,
pheromone, conductivity, slime mold, firefly, and spike. Some of those names
carry real mechanisms. A pulse-coupled oscillator can be defended as firefly
synchronization if the coupling model, assumptions, and limits are explicit.
Power-aware behavior can be defended as metabolism if the data model keeps the
physical quantities that downstream consumers need.

The same language becomes harmful when it labels ordinary local heuristics. A
weighted peer score is not a GossipSub defense. A monotone conductivity value
without decay, edge cost, flow, or conservation is not a Physarum routing model.
An LED phase animation is not useful human telemetry unless it maps to a small,
readable state vocabulary. The outside-perspective transcript in `out.md`
surfaced the same pattern across peer scoring, task diffusion, pulse gating,
Spike, capability matching, and evaluation metrics. It is an input for triage,
not evidence by itself.

## Decision

Hypha keeps bio-inspired names only when the implementation preserves the
mechanism that makes the source model useful, or when measurements show the
heuristic earns the claimed behavior. Otherwise the public API, docs, examples,
and test names use plain operational terms.

This is a naming and evidence rule, not a ban on biological inspiration. A
future implementation can re-earn a name by carrying the load-bearing mechanism:
decay for path reinforcement, signed and bounded inputs for adversarial
scoring, RX-side radio gating for power claims, or measured energy cost for
power-aware routing.

## Options considered

Keep the current language and add caveats. Rejected because caveats do not
protect examples, API names, or future design work from importing a proof that
the code does not actually satisfy.

Remove all biological language. Rejected because some terms still carry useful
local meaning. `Metabolism` names a physical power model, and firefly
synchronization can remain a valid model when its assumptions are explicit.

Wait until implementation changes. Rejected because the words are already
steering the roadmap. The evidence rule should land before more features build
on overstated names.

## Consequences

Docs and examples should lead with the operational behavior: power-aware local
coordination, telemetry, signed OTA, and measured radio behavior. Existing names
that are only metaphors become candidates for rename or downgrade. For example,
`conductivity` should either become a real flow quantity with decay and an edge
model, or be renamed to a usage or pressure heuristic.

Evaluation must measure the claimed property. A power-aware feature needs
energy or current measurements, not only delivery rate or graph reachability. A
security or adversarial-resilience claim needs validation verdicts, penalties,
or signatures, not only positive self-reported peer fields.

This ADR does not decide the final routing, scoring, LED, or task-allocation
designs. It sets the bar those designs must clear before their names and docs
make strong claims.

## Lineage

This is Hypha's first repo-local ADR. It should be linked by future ADRs that
choose whether to formalize or rename Physarum routing, peer scoring, firefly
sync, Spike, and capability/task semantics.
