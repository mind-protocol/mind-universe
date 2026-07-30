# mind-universe bootstrap TODO

## Objective

Build the smallest trusted native kernel capable of loading, executing,
persisting, physically materializing, observing, and repairing a graph-native
Universe.

The bootstrap is successful when behavior can move out of Python/native
dispatch and into graph CodeDefinitions without losing determinism, bounded
execution, real effect receipts, crash recovery, or independent readback.

## Status legend

- `[ ]` not started
- `[~]` in progress
- `[x]` completed and evidenced
- `[!]` blocked; blocker and attempted recovery must be recorded

## Coordination rules

- Agent 1 owns shared primitive contracts and publishes them before dependent
  agents rely on them.
- Agents 2 through 5 must not independently redefine IDs, revisions, receipts,
  commands, or wire formats.
- Each agent works primarily inside the paths listed in its ownership section.
- Shared changes require a message to the coordinator and all affected agents.
- No agent may mark an E2E item complete using only its own component tests.
- No graph behavior may be implemented as hidden Rust or TypeScript policy.

## Dependency map

```text
Agent 1 — Core truth and persistence
     ├── Agent 2 — Physics and local queries
     ├── Agent 3 — Graph IR and VM
     └── Agent 4 — Supervisor, transactions, effects, protocol
             └── Agent 5 — Desktop, scale, and complete E2E

Agents 2 + 3 + 4
             └── First graph_read vertical slice
```

---

## Agent 1 — Core truth and persistence

### Ownership

```text
Cargo.toml
crates/universe-core/
crates/universe-store/
crates/universe-testkit/
fixtures/genesis/
```

### P0 contracts

- [x] Create the Rust workspace and shared crate skeletons.
- [x] Define `UniverseId`, `EntityKey`, `RelationKey`, `ChunkId`, `Tick`, and
  `Revision`.
- [x] Define stable packing/unpacking for Rapier `user_data: u128`.
- [x] Define `ContentPtr { segment, offset, length }`.
- [x] Define the canonical error taxonomy.
- [x] Define epistemic result states: measured, unknown, not measured, failed.
- [x] Define the shared serialization/version envelope.
- [x] Publish contract fixtures for Agents 2–5.

### P0 entity and relation stores

- [ ] Implement a generational entity arena.
- [ ] Implement structure-of-arrays entity columns.
- [ ] Implement compact symbol interning at ingestion boundaries.
- [ ] Implement a dense relation arena.
- [ ] Implement CSR adjacency for immutable snapshots.
- [ ] Implement bounded mutable adjacency overlays.
- [ ] Implement relation tombstones.
- [ ] Implement deterministic overlay compaction.
- [ ] Add property tests for allocation, reuse, stale handles, and endpoints.

### P0 content store

- [ ] Implement immutable append-only JSONL segments.
- [ ] Return a stable `ContentPtr` after durable append.
- [ ] Compute and verify full content hashes.
- [ ] Implement direct offset/length reads without scans.
- [ ] Implement a bounded hydration cache.
- [ ] Detect truncated records and corrupted segments.
- [ ] Track orphan content created before failed commits.
- [ ] Add safe orphan collection based on committed references.

### P0 snapshots and replay

- [x] Define `UniverseSnapshot`.
- [x] Define `UniverseEventLog` records with checksum and idempotency key.
- [ ] Implement atomic checkpoints.
- [x] Implement replay from snapshot plus log.
- [x] Detect and recover a truncated final log record.
- [ ] Refuse mid-log corruption.
- [ ] Add snapshot format migrations.
- [x] Provide a signed/minimal Genesis Graph fixture.
- [ ] Prove boot → mutate → checkpoint → crash → replay equivalence.

### P1 scale proof

- [ ] Measure exact per-entity and per-relation resident bytes.
- [ ] Generate 1 M entities / 1 M relations.
- [ ] Generate 10 M entities / 10 M relations.
- [ ] Measure load time, checkpoint time, replay time, and memory.
- [ ] Publish benchmark data for Agent 5.

### Agent 1 done criteria

- [ ] All shared contracts have stable versioned fixtures.
- [ ] 10 M truth-layer entities can load without any Rapier body.
- [ ] Snapshot/replay produces the same IDs, revisions, and hashes.
- [ ] Dependent agents can use the contracts without private replacements.

---

## Agent 2 — Physics, locality, fields, and folds

### Ownership

```text
crates/universe-physics/
crates/universe-query/
crates/universe-fields/
benches/physics/
```

### P0 Rapier host

- [x] Wrap Rapier behind `UniversePhysics`.
- [x] Map `EntityKey` to bodies and colliders.
- [ ] Map `RelationKey` to physical bindings.
- [ ] Define `PhysicsCommand`, `PhysicsDelta`, and `PhysicsEvent` using Agent 1
  contracts.
- [x] Implement fixed timestep and deterministic command ordering.
- [ ] Detect NaN, infinities, excessive velocity, and energy explosions.
- [ ] Add bounded local rollback for faulty physical bindings.
- [ ] Expose active bodies, contacts, sensors, and solver health.

### P0 residency

- [ ] Implement `Hot`, `Sleeping`, `Aggregated`, and `Dormant`.
- [ ] Materialize dormant entities into Rapier.
- [ ] Dematerialize stable entities into compact state.
- [ ] Implement Space aggregate bodies and local child transforms.
- [ ] Add warm-residency hysteresis.
- [ ] Add body, collider, joint, and wake budgets.
- [ ] Detect wake storms and giant active islands.

### P0 local query engine

- [x] Define `QueryOrigin`, `QueryBudget`, and `LocalSituation`.
- [ ] Collect spatial candidates through Rapier queries.
- [ ] Collect contact and sensor candidates.
- [x] Traverse bounded relation adjacency.
- [ ] Collect active causal fields.
- [ ] Deduplicate with reusable visit stamps.
- [ ] Implement bounded Top-K.
- [ ] Hydrate content only after selection.
- [x] Return complete/partial/frontier/budget/error metadata.
- [x] Prove no local query scans all Universe entities.

### P0 ReadField and TopologicalFold

- [x] Define `ReadField` lifecycle.
- [x] Consume graph-provided selectors and budgets without native policy.
- [ ] Define and consume `SpaceSummary`.
- [ ] Rank resonating Spaces using graph-provided scoring parameters.
- [x] Implement `TopologicalFold`.
- [ ] Implement fold appearance, stabilization, decay, cancellation, and reuse.
- [ ] Allow multiple Actors to fold toward one Space without duplicating it.
- [x] Emit `read_started` through `read_released` events.

### P1 field and counterfactual engines

- [ ] Implement attraction, inhibition, and attention field primitives.
- [ ] Implement lazy temporal laws.
- [ ] Implement event-driven propagation frontiers.
- [ ] Implement cluster summaries with declared error bounds.
- [ ] Expand clusters when requested precision exceeds the bound.
- [ ] Implement local ghost/counterfactual simulation.
- [ ] Guarantee ghost simulations cannot emit external effects.

### P1 physics scale proof

- [ ] Benchmark 10 k active bodies.
- [ ] Benchmark 50 k active bodies.
- [ ] Benchmark 100 k active bodies.
- [ ] Benchmark dense contacts and sparse contacts separately.
- [ ] Benchmark large joint components and wake storms.
- [ ] Report tick p50, p95, p99, memory, and active constraint counts.

### Agent 2 done criteria

- [ ] A local query is bounded by its working set, not total Universe size.
- [ ] A Space can fold, expand, hydrate, contract, and return to warm/dormant.
- [ ] Physics failures remain local and produce measured health.
- [ ] No ontology-specific predicate policy is hidden in Rust.

---

## Agent 3 — Graph IR, compiler, VM, and triggers

### Ownership

```text
crates/universe-ir/
crates/universe-vm/
crates/universe-compiler/
fixtures/graph-ir/
benches/vm/
```

### P0 Graph IR specification

- [x] Define versioned value types.
- [x] Define local query opcodes.
- [ ] Define set/filter/transform opcodes.
- [ ] Define boolean and comparison opcodes.
- [ ] Define branch, call, and return.
- [ ] Define bounded `FOR_EACH`, `REPEAT_N`, and `UNTIL_WITH_LIMIT`.
- [x] Define write-set construction opcodes.
- [ ] Define physics command opcodes.
- [ ] Define capability intent opcodes.
- [ ] Define error and `ON_ERROR` paths.
- [ ] Define `NEXT`, `INPUT`, `OUTPUT`, `BODY`, `TRUE_NEXT`, and `FALSE_NEXT`.
- [x] Keep all program parameters and composition in graph data.

### P0 validator

- [x] Validate entrypoints.
- [x] Validate input/output types.
- [ ] Detect unbounded cycles.
- [ ] Require budgets for queries and loops.
- [ ] Require declared capabilities.
- [ ] Detect orphan operators and unreachable exits.
- [x] Detect registers read before assignment.
- [x] Produce a canonical CodeDefinition hash.
- [ ] Produce a graph-readable validation report.

### P0 compiler

- [x] Compile Graph IR to compact bytecode.
- [x] Allocate registers deterministically.
- [ ] Resolve branches and constants.
- [x] Produce source mappings from instruction to canonical node.
- [ ] Store bytecode by content hash.
- [ ] Invalidate artifacts after CodeDefinition revision changes.
- [ ] Reject mismatched artifacts.

### P0 VM

- [x] Implement typed execution frames.
- [x] Pin code and starting Universe revisions.
- [ ] Implement fuel, call depth, loop, mutation, and tick budgets.
- [ ] Implement cancellation and timeout.
- [x] Produce a write set, never a direct mutation.
- [x] Implement deterministic traps for budget exhaustion.
- [x] Emit structured per-op traces.
- [x] Produce `ExecutionReceipt`.
- [ ] Keep a debug interpreter and compare it with compiled execution.

### P0 triggers

- [ ] Define trigger subscription records.
- [ ] Convert Universe events into bounded execution requests.
- [ ] Implement idempotency keys and causal ancestry.
- [ ] Enforce cooldown, debounce, and maximum causal depth.
- [ ] Detect trigger cycles and storms.
- [ ] Produce DMZ/quarantine intents instead of silently dropping failures.

### P1 migration fixture

- [ ] Represent local Narrative discovery entirely in Graph IR.
- [x] Open a ReadField through Agent 2 primitives.
- [ ] Await stable/partial completion.
- [x] Filter, Top-K, hydrate, and return results without Python or Cypher.
- [x] Create a proposed result Moment write set.

### Agent 3 done criteria

- [ ] A graph-defined program validates, compiles, executes, and traces.
- [ ] The same fixture has equivalent interpreter and bytecode results.
- [ ] Self-modification creates a later CodeDefinition revision without changing
  the current execution.
- [ ] No Python NodeCode is required for the migration fixture.

---

## Agent 4 — Transactions, supervisor, capabilities, and protocol

### Ownership

```text
crates/universe-transactions/
crates/universe-supervisor/
crates/universe-capabilities/
crates/universe-protocol/
apps/universe-server/
```

### P0 transaction manager

- [x] Define `UniverseCommand`, `UniverseWriteSet`, and
  `UniverseTransaction`.
- [ ] Implement base-revision and precondition checks.
- [ ] Validate endpoints, content references, and primitive invariants.
- [x] Commit only at tick boundaries.
- [x] Prevent partially visible mutations.
- [ ] Produce commit, conflict, validation, and rollback receipts.
- [ ] Connect committed mutations to Agent 2 physical commands.
- [ ] Preserve causal ancestry from trigger to receipt.

### P0 supervisor

- [x] Implement the boot state machine.
- [x] Own the Universe clock and deterministic phase ordering.
- [x] Schedule Graph VM execution.
- [x] Schedule transaction commits.
- [x] Schedule field and physics phases.
- [ ] Implement CPU, memory, wake, and backlog budgets.
- [ ] Implement degraded operation without renderer or optional adapters.
- [ ] Implement checkpoints and crash recovery.
- [ ] Expose tick, revision, state, backlog, and health.
- [ ] Detect stuck scheduler, log, checkpoint, and physics phases.

### P0 capability host

- [ ] Define `EffectIntent` and `EffectReceipt`.
- [ ] Implement a versioned capability registry.
- [ ] Keep secrets outside graph snapshots and logs.
- [ ] Implement timeout, cancellation, and idempotency.
- [ ] Capture actual transport success/failure.
- [ ] Reinject receipts as Universe events.
- [ ] Add one safe test adapter.
- [ ] Add explicit human gating for irreversible adapters.

### P0 protocol

- [ ] Define versioned snapshot, delta, event, query, read, and receipt messages.
- [ ] Implement binary channels for high-volume streams.
- [ ] Add sequence numbers and resynchronization.
- [ ] Add backpressure and bounded queues.
- [ ] Add Actor/Observer authentication and capabilities.
- [ ] Enforce content visibility before hydration.
- [ ] Implement asynchronous `graph_read` handles and event streams.

### P0 real boot

- [ ] Load configuration and secrets.
- [ ] Open stores and content segments.
- [x] Load or create the Genesis revision.
- [x] Replay and validate the Universe.
- [x] Load, validate, and compile CodeDefinitions.
- [ ] Restore hot chunks and physics.
- [ ] Start VM, scheduler, clock, and capabilities.
- [ ] Open IPC/network only after coherent readiness.
- [ ] Expose honest `Ready`, `Degraded`, `Recovering`, and `Blocked`.

### Agent 4 done criteria

- [ ] No effect can bypass capability validation and receipt capture.
- [ ] A crash during every transaction phase recovers coherently.
- [ ] Clients can miss deltas and resynchronize.
- [ ] The supervisor contains no graph behavior or ontology-specific decisions.

---

## Agent 5 — Desktop, benchmarks, and complete E2E

### Ownership

```text
apps/mind-desktop/
crates/universe-e2e/
benches/universe/
tests/e2e/
artifacts/verification/
```

### P0 integration harness

- [x] Consume Agent 1 Genesis and store fixtures.
- [x] Consume Agent 2 query/read event stream.
- [x] Consume Agent 3 Graph IR migration fixture.
- [x] Consume Agent 4 supervisor/protocol.
- [x] Build one command that launches the headless complete system.
- [x] Record correlation IDs across all components.
- [x] Add independent readback helpers.

### P0 first vertical slice

- [x] Boot from Genesis.
- [ ] Connect an Actor.
- [x] Issue graph-defined `graph_read`.
- [x] Observe `ReadField` creation.
- [x] Observe Space resonance.
- [x] Observe `TopologicalFold`.
- [x] Observe local entity materialization.
- [x] Execute the graph-defined Narrative query.
- [x] Commit a result Moment.
- [x] Read the Moment back through a new local query.
- [x] Observe fold release and physical stabilization.
- [x] Prove no Python or Cypher path ran.
- [x] Report partial/budget/frontier state honestly.

### P1 Mind Desktop

- [ ] Create the Tauri shell.
- [ ] Implement snapshot/delta/event consumption.
- [ ] Render Actors, Spaces, Things, Moments, and Narratives generically.
- [ ] Render aggregate Space bodies and progressive expansion.
- [ ] Render folds and ReadFields.
- [ ] Render CodeDefinitions as SDF text panels.
- [ ] Implement semantic LOD.
- [ ] Add ObserverCamera separate from ActorInstance.
- [ ] Add inspector, timeline, execution trace, and health overlay.
- [ ] Read visual mappings from graph data.
- [ ] Do not hard-code the canonical predicates in TypeScript.

### P1 system benchmarks

- [ ] Run Agent 1's 10 M truth-layer benchmark.
- [ ] Run Agent 2's active physics benchmarks.
- [ ] Measure local-query p50, p95, and p99.
- [ ] Measure content hydration latency.
- [ ] Measure snapshot/replay and cold boot.
- [ ] Measure fold expansion/contraction.
- [ ] Measure multi-Actor active-region union.
- [ ] Measure protocol throughput and dropped-delta recovery.
- [ ] Publish hardware, parameters, raw results, and uncertainty.

### P1 failure stories

- [ ] Corrupt a final event record and recover.
- [ ] Corrupt a middle event record and block honestly.
- [ ] Exhaust query budget.
- [ ] Exhaust VM fuel.
- [ ] Trigger a write conflict.
- [ ] Trigger a wake storm.
- [ ] Drop protocol deltas and resync.
- [ ] Fail an external effect and preserve the failed receipt.
- [ ] Restart during an open fold.

### Agent 5 done criteria

- [ ] The complete story works through the production protocol.
- [ ] Runtime evidence is independently readable.
- [ ] Performance claims include raw fresh measurements.
- [ ] The desktop reflects real events and never invents successful state.

---

## Coordinator checklist

- [x] Confirm all agents read `AGENTS.md` and this file.
- [x] Confirm ownership paths do not overlap.
- [x] Have Agent 1 publish shared contracts first.
- [x] Freeze Graph IR v0 before the vertical slice integration.
- [ ] Freeze protocol v0 before Mind Desktop integration.
- [ ] Review native code for hidden graph behavior.
- [x] Keep one canonical E2E correlation ID per run.
- [x] Refuse completion without independent readback.
- [ ] Reconcile every blocked item with cause, attempts, and required decision.

## Bootstrap completion gate

The repository reaches bootstrap v0 only when:

- [ ] a signed Genesis Graph boots a coherent Universe;
- [ ] graph CodeDefinitions validate and compile;
- [ ] a trigger launches the real Graph VM;
- [ ] a bounded local query physically opens the relevant situation;
- [ ] a write set commits atomically;
- [ ] an external test effect produces a real receipt;
- [ ] the result is independently read back;
- [ ] crash/replay restores the same committed revision;
- [ ] no business behavior is hidden in Rust, TypeScript, or Python;
- [ ] the complete E2E runs without Python or Cypher.

## First vertical slice evidence — 2026-07-30

Canonical coordinator run:

```text
correlation: e2e-10732-1785405571972966200
command: cargo run -p universe-e2e -- artifacts/verification/coordinator-final
manifest: artifacts/verification/coordinator-final/e2e-10732-1785405571972966200/manifest.json
runtime inventory: artifacts/verification/coordinator-final/e2e-10732-1785405571972966200/runtime-inventory.json
VM trace: artifacts/verification/coordinator-final/e2e-10732-1785405571972966200/vm-trace.jsonl
phase order: artifacts/verification/coordinator-final/e2e-10732-1785405571972966200/phases.json
authoritative event log: artifacts/verification/coordinator-final/store/events.jsonl
```

Observed and independently read back:

- Genesis hash and graph-owned Actor, Space, QueryPolicy, Narrative and Moment
  type loaded from the canonical fixture.
- `ReadField` opened for Actor `...01`; Space `...02` resonance measured as
  `899.9999771118164` using graph metric `resonance`.
- The initial bounded local read reported `budget_exhausted`, with 4 entities
  visited and 8 relations inspected; this was preserved rather than promoted to
  complete.
- Rapier materialized entities `...02` and `...03`.
- Graph IR revision 1 compiled and executed 17 traced instructions with fuel 17.
- The VM proposed a graph-derived `put_entity`; the transaction committed
  revision 0 to revision 1 at tick 1 with a checksummed event.
- A fresh protocol/store replay read Entity `...13` with graph-derived Moment
  symbol 7; a second local query independently found it and reported
  `frontier_exhausted`.
- Fold release returned both physical entities to `Dormant`.
- Runtime inventory contained exactly one activated mechanism:
  executor `universe-vm`; no transport was activated. Secondary Python and
  Cypher invocation counters were both zero.

Fresh validation:

```text
cargo fmt --all -- --check
cargo test --workspace
```

Both commands passed before the coordinator run.

Known first-slice limits and real blockers:

- Transactions are deliberately limited to exactly one command because the
  current store append API cannot yet prove atomic multi-command publication.
- The first local query exhausts its declared relation budget; this is an
  evidenced bounded result, not a complete traversal.
- Full loop/call-depth/mutation/tick budget support, trigger wiring, protocol
  resynchronization, cryptographic Genesis signing, 10M/10M scale proof and
  Desktop remain unimplemented and unchecked.
- `target/` and verification artifacts are generated local evidence; no branch,
  commit, stage, push or pull request was created.
