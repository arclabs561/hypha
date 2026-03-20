# Hypha Ecosystem Architecture

Hypha is the **L6 Coordination Layer** of the Tekne Stack. It provides the "Fungal P2P" substrate that connects other Tekne components.

## 1. Core Principles

-   **Bio-Inspired**: Systems model biological processes (mycelial growth, quorum sensing, metabolism).
-   **Resource-Aware**: "Metabolism" is a first-class citizen.
-   **Local-First**: Data and compute prefer to stay local; sync is eventual (CRDTs).
-   **Ecosystem-Integrated**: Hypha does not reinvent the wheel. It connects an external agent layer, an ANN/knowledge layer, and low-level SIMD/math crates.

## 2. Architecture

```mermaid
graph TD
    A[external agents (planning/orchestration)] --> B[hypha-core (P2P)]
    B --> C[hypha-compute (WASM)]
    B --> D[knowledge/ANN layer]
    C --> E[innr (Simd)]
```

### `hypha-core` (The Nervous System)
*   **Responsibility**: Networking, Mesh Topology, State Sync (CRDT), Resource Discovery.
*   **Key Types**: `SporeNode`, `Mycelium`, `TopicMesh`, `SharedState`, `Metabolism`.
*   **Status**: **Stable / Hardened**.

### `hypha-compute` (The Muscles)
*   **Responsibility**: Executing tasks safely within resource bounds.
*   **Technologies**: `wasmtime` (WASM sandbox).
*   **Key Features**:
    *   **Sandboxing**: Strict fuel limits mapped to `Metabolism`.
    *   **Isolation**: Prevents agentic tasks from crashing the node.

### External agent layer (The Brain)
*   **Responsibility**: Agentic reasoning, planning, model/tool orchestration.
*   **Integration**: Agents run *on* `hypha` nodes (or adjacent hosts) and use `hypha-core` for coordination.

### Knowledge / ANN layer (The Memory)
*   **Responsibility**: Vector storage, ANN search, recall.
*   **Integration**: `hypha` nodes use a local knowledge layer for semantic routing and retrieval.

## 3. Data Flow

1.  **Sensing**: `hypha-core` receives `EnergyStatus` and `Task` bundles.
2.  **Reasoning**: an external agent decides whether to bid/accept work.
3.  **Execution**: `hypha-compute` runs the task (WASM).
4.  **Accounting**: `Metabolism` tracks cost.

## 4. Roadmap

1.  **Phase 1 (Complete)**: Hardened P2P networking & CRDT sync.
2.  **Phase 2 (Current)**: WASM Compute runtime (`hypha-compute`).
3.  **Phase 3**: Integration with external agents and the knowledge/ANN layer.
