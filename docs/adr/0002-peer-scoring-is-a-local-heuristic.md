---
id: 0002
status: proposed
governs: src/core/mesh.rs, tests/gossip_storm.rs, tests/adversarial_inputs.rs, ARCHITECTURE.md, docs/**
why: Hypha's current peer score is a positive weighted sum over local observations and self-reported energy; it is useful for mesh maintenance but does not defend against malicious peers.
rejected: treating the score as adversarial resilience; replacing it with a GossipSub-style security score before Hypha has validation verdicts, signed inputs, and a hostile-peer deployment requirement.
supersedes: none
superseded_by: none
extends: 0001
confidence: medium
review_trigger: revisit before adding negative peer penalties, signed energy claims, pulse signatures, alert quorum behavior, or public security claims.
---

# ADR-0002: peer scoring is a local heuristic

**Status**: Proposed
**Date**: 2026-06-18
**Deciders**: arc

## Context

`TopicMesh` keeps a `MeshPeer` table and computes a score from four terms:
energy, activity, conductivity, and pressure. That score drives local graft,
prune, opportunistic graft, and forwarding choices. It is not the same thing as
libp2p GossipSub v1.1 peer scoring, and it does not currently have negative
terms for invalid messages, spam, duplicate behavior, IP colocation, or graylist
thresholds.

The inputs also do not have the trust boundary needed for an adversarial score.
Energy can be self-reported through `EnergyStatus`; activity rises when messages
are observed; conductivity is updated from local pressure changes; and pressure
is a local load hint. These are acceptable inputs for a scheduling heuristic.
They are not enough to decide that a peer is honest, safe, or authorized.

ADR-0001 requires Hypha to downgrade names and claims when the implementation
does not carry the mechanism that would make the claim true. For peer scoring,
the missing mechanisms are signed or otherwise verifiable inputs, validation
verdicts, negative penalties, decay rules tied to behavior, and thresholds that
gate what a peer may do after bad behavior.

## Decision

Hypha treats the current peer score as a local mesh-maintenance heuristic only.
It may choose which known peers to graft, prune, or forward to, but it must not
be described as a security boundary, Sybil defense, malicious-peer defense, or
GossipSub-equivalent score.

Until a later ADR supersedes this decision, implementation work should keep the
score simple and local. The code may be renamed or documented to make that
scope clearer, and tests may assert survival under malformed or high-volume
inputs, but those tests do not prove adversarial resilience.

## Options considered

Freeze the score as a local heuristic. Chosen for now. It matches the code that
exists, keeps the public claim honest, and avoids adding security-shaped surface
without the data needed to make it work.

Delete peer scoring entirely. Rejected for now because the score still carries
local scheduling information for mesh maintenance, especially energy and load.
Deletion remains available if future tests show that a simpler `last_seen` or
fixed-degree policy performs as well.

Replace it with a GossipSub-style adversarial score. Deferred. A defensible
version would need signed or otherwise verifiable energy claims, application
validation verdicts, negative penalties, decay and recovery behavior, and hard
thresholds such as graylist or publish gates. Adding those pieces now would
create protocol and compatibility cost before Hypha has a hostile-peer
deployment requirement.

Treat score as a best-effort security signal with caveats. Rejected because it
would invite future code and docs to rely on a boundary the implementation does
not provide.

## Consequences

Docs and examples should call this a local peer-selection score or mesh
heuristic, not a trust score. Any public statement about Sybil resistance,
malicious-peer resistance, or security posture must point somewhere else or be
removed.

Security-sensitive behavior needs a separate path. If Hypha keeps alert-like
messages, firmware pulses, or remote control topics, those mechanisms need
their own authentication and quorum rules rather than inheriting trust from
`MeshPeer::score()`.

The next implementation step is narrow: update naming and docs around mesh
score semantics, then add tests that protect the local behavior this ADR keeps.
The later decision to add signed scoring remains open and should supersede this
ADR if accepted.

## Lineage

Extends ADR-0001 by applying the "mechanisms must earn claims" rule to Hypha's
peer score. Related future ADRs should cover alert signing/quorum and firefly
pulse trust separately, because those decisions can change independently of the
local mesh-maintenance score.
