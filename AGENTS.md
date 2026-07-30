# AGENTS.md

## Purpose of this repository

`mind-universe` contains the trusted bootstrap kernel for a graph-native universe.

The universe itself is the long-term source of truth:

- entities and relations;
- ontology and physical mappings;
- CodeDefinitions and Graph IR programs;
- objectives, policies, triggers, loops, and behaviors;
- Moments, evidence, health, and maintenance;
- the current physical and causal state.

This repository exists only for the code that must run before the graph can load,
validate, execute, persist, observe, and repair itself.

The intended boundary is:

```text
Native bootstrap kernel
→ loads the Universe
→ validates and executes graph-native programs
→ materializes local physical situations
→ commits measured results back into the Universe
```

Do not turn the bootstrap kernel into a second hidden application layer.

---

## The bootstrap exception

The general Mind rule remains graph-first: behavior belongs in graph nodes,
relations, CodeDefinitions, loops, policies, and ChangeSets.

This repository is the explicit bootstrap exception. Direct source code is
allowed here only when the behavior cannot yet be expressed or executed by the
Universe itself.

Native bootstrap code may implement:

- stable identifiers and generational handles;
- dense entity and relation storage;
- content segments, snapshots, event logs, and recovery;
- transactions, revisions, clocks, and deterministic ordering;
- the physics solver host and physical residency machinery;
- local query primitives;
- the Graph IR validator, compiler, and virtual machine;
- scheduling, fuel, cancellation, and bounded execution;
- capability enforcement and real effect transports;
- IPC, streaming, authentication, and backpressure;
- generic rendering primitives and debugging surfaces;
- health, evidence capture, replay, and crash recovery.

Native bootstrap code must not contain:

- organisation- or Citizen-specific objectives;
- hard-coded workflows or business rules;
- ontology-specific decision policies;
- predicate-to-physics values that belong in the graph mapping;
- query programs that can be represented in Graph IR;
- NodeCode behavior that can be represented in Graph IR;
- tool-selection policies;
- cognitive roles or personalities;
- visual meaning for individual canonical predicates;
- hidden fallbacks that silently replace missing graph state.

If a rule can vary without changing the trusted computing base, it belongs in
the graph.

---

## Authority before and after bootstrap

Before the first Universe can boot, the following are permitted bootstrap
artifacts:

- this `AGENTS.md`;
- `TODO.md`;
- Rust workspace and build configuration;
- the trusted kernel source;
- generic TypeScript/Tauri renderer infrastructure;
- tests, fixtures, benchmarks, and recovery tools;
- a loader for a signed Genesis Graph.

The Genesis data is graph data. The loader is native code.

After graph-native execution exists:

1. change graph authority first for behavior;
2. validate the affected CodeDefinition or loop;
3. compile or materialize the derived artifact;
4. execute it through the real runtime;
5. read the result back independently;
6. record fresh evidence and health.

Never keep a native behavior merely because it was convenient during
bootstrap. Migrate it into Graph IR when the language can express it.

---

## Universe-as-truth invariants

### One authoritative state

A committed `UniverseSnapshot` plus its subsequent valid event log is the
authority for a Universe revision.

Rapier is an internal numerical solver. A running Rapier world by itself is not
the complete Universe.

Repository files, JSONL segments, bytecode, indexes, and renderer state are not
independently authoritative unless the committed Universe references them and
their hashes match.

### Stable identity

Canonical entity and relation identity must never depend on:

- a process memory address;
- a Rapier handle;
- a JSONL line number;
- an array slot without a generation;
- a renderer object ID.

Rapier `user_data` carries a stable packed Universe handle. That handle resolves
through the kernel to the current entity or relation record.

### Local queries only

Every runtime query must declare:

- an origin such as an Actor, Observer, Entity, or Space;
- a spatial, relational, causal, and temporal budget;
- a maximum number of entities and relations;
- a timeout or tick budget;
- whether approximate cluster summaries are acceptable.

No hot-path query may scan the complete Universe.

Results must distinguish:

- complete;
- partial;
- budget exhausted;
- frontier exhausted;
- stale;
- unknown;
- measurement failed.

### Bounded physical materialization

All entities may exist in the Universe, but they must not all be active Rapier
bodies.

Physical residency is explicit:

```text
Hot
Sleeping
Aggregated
Dormant
```

Only bounded local working sets may be materialized into the active solver.

### Atomic mutation

Graph IR programs and external callers produce write sets or intents. They do
not mutate the Universe directly.

The normal mutation flow is:

```text
Intent
→ validation
→ capability check
→ conflict check
→ tick-boundary commit
→ Universe receipt
→ physical delta
→ independent observation
```

### Real effects require receipts

Text claiming that an effect occurred is not evidence.

Every external effect must follow:

```text
EffectIntent
→ authorized transport
→ actual transport result
→ EffectReceipt
→ reinjection into the Universe
```

Timeout, cancellation, idempotency, cooldown, and causal depth are mandatory.

---

## Graph IR rules

Graph IR is the canonical language for NodeCode behavior.

The native kernel implements only primitive opcode semantics. Programs,
conditions, loops, triggers, parameters, and composition live in graph
CodeDefinitions.

Graph IR must support:

- local queries;
- typed values and collections;
- filters and transformations;
- explicit boolean condition subgraphs;
- branches;
- bounded loops;
- triggers and scheduled events;
- write-set construction;
- physical commands;
- capability intents;
- error paths and receipts.

Graph IR must not support unbounded implicit traversal or unrestricted loops.

Every execution is pinned to:

- one CodeDefinition revision;
- one starting Universe revision;
- one trigger event;
- one fuel and mutation budget.

A CodeDefinition changed during execution affects only later executions.

Hot CodeDefinitions may compile to compact bytecode. Bytecode is a
content-addressed cache, not canonical code.

---

## Physics rules

Ontology and physics are connected through graph-defined mappings consumed by a
generic native adapter.

Do not hard-code the canonical predicates in Rapier dispatch code.

A relation may materialize as:

- a joint;
- a force law;
- a sensor rule;
- a field;
- an event rule;
- a visual-only relation;
- no physical object until locally activated.

Do not create one permanent joint per relation by default.

Queries may create temporary runtime entities such as:

- `ReadField`;
- `TopologicalFold`;
- counterfactual ghost state;
- local cluster expansions.

These runtime entities must have bounded energy, lifetime, wake cost, and
materialization budgets.

Detect and contain:

- NaN and infinite values;
- kinetic explosions;
- wake storms;
- giant active islands;
- high-frequency link oscillation;
- trigger storms;
- non-convergent counterfactual simulations.

---

## Storage and scale rules

The design target is at least 10 million entities and 10 million relations in
the Universe truth layer.

Use:

- structure-of-arrays for hot entity and relation columns;
- generational arenas for stable local handles;
- compact symbol IDs;
- CSR-style adjacency for stable relation snapshots;
- bounded mutable overlays and tombstones for recent changes;
- append-only immutable content segments;
- lazy temporal evaluation;
- event-driven causal frontiers;
- chunk summaries and physical residency levels.

Do not use:

- one HashMap entry per field per entity in the hot path;
- one active rigid body per Universe entity;
- one active constraint per Universe relation;
- per-tick global decay scans;
- JSONL scans during queries;
- content hydration before filtering and Top-K selection.

All scale claims require fresh benchmarks with memory, p50, p95, and p99
evidence.

---

## Epistemic discipline

Always distinguish:

- `observed`;
- `measured`;
- `known_absent`;
- `unknown`;
- `not_measured`;
- `measurement_failed`.

Never interpret:

- missing data as zero;
- an empty local result as global absence;
- a compiled CodeDefinition as a wired execution;
- a running process as healthy behavior;
- a committed intent as a completed external effect;
- a physical equilibrium as semantic truth;
- a renderer animation as a runtime receipt.

---

## Completion criteria

A task is not complete because:

- code compiles;
- a node or file exists;
- a unit test passes;
- a process starts;
- a physics step runs;
- an intent is emitted.

A task is complete only when the relevant level is proven:

1. contracts and invariants are explicit;
2. targeted validation passes;
3. the real bootstrap/runtime path executes;
4. the result is independently read back;
5. errors and missing evidence remain distinguishable;
6. no forbidden effect occurred;
7. performance is measured when scale is part of the promise;
8. graph-native behavior contains no hidden native duplicate.

The required first vertical slice is:

```text
Actor graph_read
→ ReadField
→ local Space resonance
→ TopologicalFold
→ graph-defined CodeDefinition execution
→ bounded result
→ committed Moment
→ independent local readback
→ fold release
```

---

## Working in this repository

### Inspect before editing

Read:

- this file;
- `TODO.md`;
- current contracts and schemas;
- tests for the component being changed;
- current git status.

Preserve concurrent work. Do not overwrite or revert another agent's changes.

### Respect ownership

`TODO.md` assigns primary ownership to five agents. Work only in the paths owned
by the assigned role unless the coordinator explicitly authorizes a shared
contract change.

Shared contract changes must be communicated before dependent implementations
are updated.

### Keep native semantics minimal

When implementing an opcode or adapter, ask:

1. Is this a primitive the graph cannot define itself?
2. Is its contract generic across Citizens and organisations?
3. Can policy and parameter choices remain graph data?
4. Does it return evidence rather than self-declared success?

If not, move the behavior into Graph IR.

### Verification

Prefer:

- deterministic fixtures;
- property tests;
- corruption and crash tests;
- replay tests;
- live local-query readback;
- end-to-end effect receipts;
- measured benchmarks.

Do not substitute static structure checks for runtime proof.

### Git

Do not create branches, commits, pushes, or pull requests unless the user asks.

Do not stage another agent's work. Do not run destructive git commands.

---

## Emergency recovery

Emergency native changes are permitted only when the Universe cannot boot or
repair itself.

Every emergency change must:

1. identify the failed invariant;
2. remain minimal and reversible;
3. produce recovery evidence;
4. be represented as a graph Problem or maintenance record after recovery;
5. be removed or reconciled once graph-native authority is restored.
