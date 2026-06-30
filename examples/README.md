# hypha examples

Each example answers one question and is runnable from the repo root. Output
excerpts below are real, captured from a run. Several evaluation examples write
JSON or HTML reports to the current directory; run them from a scratch directory
if you do not want report artifacts in the checkout.

## Node basics

### `basic_node`: what does starting a host node look like?

Starts a libp2p-backed `SporeNode` with a local fjall store. This is a live node
process, not a finite report example; stop it with Ctrl-C.

```bash
cargo run --release --example basic_node
```
```text
INFO fjall::db: Creating database at ...
INFO libp2p_swarm: local_peer_id=12D3Koo...
INFO hypha: Hypha Spore active peer_id=12D3Koo...
```

## Local evaluation

### `fast_eval`: how do delivery and energy change under local stress?

Runs an in-memory sweep over dead-node rates, packet drops, partitions, and a
combined stress case, then writes `hypha_fast_eval.json`.

```bash
cargo run --release --example fast_eval
```
```text
Hypha Fast Evaluation Suite
===========================

Baseline:
  Delivery: 100.0%, p99: Some(49.851ms)

Percolation Threshold:
   0% dead: delivery=100.0%, exhausted= 0
  50% dead: delivery= 50.0%, exhausted=50
  90% dead: delivery= 10.0%, exhausted=90

Combined Stress (30% dead + 20% drop + partition):
  Stress: delivery=15.9%, exhausted=30

FAILURE MODES (delivery < 50%):
  - percolation_60pct: 40.0%
  - degradation_90pct: 9.6%
  - combined_stress: 15.9%

Detailed report: hypha_fast_eval.json
```

### `mycelium_eval`: how does low-power fraction affect bidding?

Sweeps synthetic low-power node fractions and reports how many nodes still bid
for work. Writes `hypha_eval_report.json`.

```bash
cargo run --release --example mycelium_eval
```
```text
Evaluating heuristic bidding with 0% low-power nodes...
Evaluating heuristic bidding with 20% low-power nodes...
Evaluating heuristic bidding with 40% low-power nodes...
Evaluating heuristic bidding with 60% low-power nodes...
Evaluating heuristic bidding with 80% low-power nodes...
Evaluation complete. Data written to hypha_eval_report.json
--- RESULT SUMMARY ---
0% Dead -> 10 Bids
20% Dead -> 8 Bids
40% Dead -> 6 Bids
60% Dead -> 4 Bids
80% Dead -> 2 Bids
```

### `mesh_eval`: where do mesh heuristics fail?

Runs mesh-maintenance scenarios, packet-loss sweeps, and a path-thickening check.
Writes `hypha_mesh_eval.json`.

```bash
cargo run --release --example mesh_eval
```
```text
MESH EVALUATION RESULTS
======================================================================

Scenario                            Delivery   MeshSz MedScore   Recovery
----------------------------------------------------------------------
baseline                              100.0%        4    0.542          -
partition_recovery                    100.0%        4    0.632        1hb
energy_drain                          100.0%        4    0.504          -

PACKET LOSS SWEEP (Percolation Threshold)
======================================================================
Loss Rate         Delivery     Status
----------------------------------------------------------------------
0.0            %     100.0%    PERFECT
50.0           %      92.0%   DEGRADED
80.0           %      44.4%     FAILED
90.0           %      12.8%     FAILED

PATH THICKENING (Mycelial Conductivity)
======================================================================
Initial mesh: 4 honest (Group A), 0 lower-energy (Group B)
Final mesh: 0 honest (Group A), 4 high-flow (Group B)
  STATUS: SUCCESS - Mesh migrated to high-flow paths despite lower energy.
```

## Allocation heuristics

### `slime_mold_auction`: how does a diffusion-scored allocation pick a winner?

Simulates score diffusion over a mesh and ranks local bids. This is a heuristic
demo, not a formal flow or consensus protocol.

```bash
cargo run --release --example slime_mold_auction
```
```text
Running diffusion-scored task allocation demo...
Injecting task at node-29. Diffusion score spreading through mesh...
Wave 1: Task score reached 4 nodes.
Wave 2: Task score reached 14 nodes.
Wave 3: Task score reached 27 nodes.
Wave 5: Task score reached 29 nodes.

Bidding Results (Top 5):
  Bidder: node-18, Weighted Score: 0.5800
  Bidder: node-24, Weighted Score: 0.5503
  Bidder: node-9, Weighted Score: 0.3944

Winner: node-18 - task selected by the local diffusion heuristic.
```

### `emergent_auction_live`: does work move toward a high-energy hub?

Runs a sparse-line allocation heuristic where a task starts at the edge and bids
move toward a high-energy node.

```bash
cargo run --release --example emergent_auction_live
```
```text
Running live allocation heuristic demo...
Stabilizing sparse peer line...
Injecting task at node-9. Simulating diffusion waves...
Wave 1:
  Node 8 bid: Weighted Score 0.2533
  Node 0 bid: Weighted Score 1.0000
Wave 10:
  Node 0 bid: Weighted Score 1.0000

Final Allocation Summary:
  Rank 1: Bidder 12D3KooWHRnhKjuWTPcXtJ84J2ZJxvi8tdrPTMvnfV9Fz5V7A1Hi, Score 1.0000

Winner: 12D3KooWHRnhKjuWTPcXtJ84J2ZJxvi8tdrPTMvnfV9Fz5V7A1Hi - task successfully pulled toward the high-energy hub.
```

## Synchrony

### `mycelial_synchrony`: do local synchrony and pressure heuristics settle?

Runs a synchrony and pressure-heuristic experiment, then writes
`hypha_sync_eval.json` and `hypha_sync_dashboard.html`.

```bash
cargo run --release --example mycelial_synchrony
```
```text
Running synchrony and pressure-heuristic experiment...
Tick 0: Variance=0.0896, Avg Pressure=0.0098
Tick 50: Variance=0.0673, Avg Pressure=0.8633
Tick 100: Variance=0.0885, Avg Pressure=1.8426
Tick 150: Variance=0.0499, Avg Pressure=2.8226
Results saved to hypha_sync_eval.json
Synchrony dashboard generated: hypha_sync_dashboard.html
```

## More

`rigorous_eval` and `generate_dashboard` are longer report generators.
`netem_node` is a network-namespace harness endpoint for external netem tests,
not a standalone demo.
