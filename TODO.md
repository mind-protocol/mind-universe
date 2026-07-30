# mind-universe bootstrap TODO

## Objective

Build the smallest trusted native kernel capable of loading, executing,
persisting, physically materializing, observing, and repairing a graph-native
Universe.

The bootstrap is successful when behavior can move out of Python/native
dispatch and into graph CodeDefinitions without losing determinism, bounded
execution, real effect receipts, crash recovery, or independent readback.

## State snapshot — 2026-07-30

Derived by counting ledger checkboxes in this file. This snapshot is a
convenience index, not an authority; the graph store remains the source of
truth (canonical ontology reconstruction: 231 entities / 784 relations,
`validated_with_explicit_gaps`, intentional gaps `BASED_ON`,
`PROPOSES_CHANGE_TO`).

| Status | Count |
| --- | --- |
| `[x]` completed and evidenced | 178 |
| `[~]` in progress | 21 |
| `[ ]` not started | 267 |
| `[!]` blocked | 0 |
| **Total** | **466** |

~62% of tracked items are not complete (288 of 466). No items are hard-blocked.

G1 (Node→Asset) — all six sub-items are now `[x]` with evidence: the whole
canonical corpus is classified with `unknown = 0`; all 26 required
content-bearing Nodes (3 `ontology_source` + 23 `ontology_contract`) are
converted to content-addressed Assets and independently read back (receipts
`artifacts/assets/*-20260730-002`); the *visual* mapping is a graph authority
(`visual.rs`, receipt `artifacts/assets/visual-mapping-20260730-001`) the Mind
Desktop renderer consumes instead of hard-coding; and a live rebuild/invalidation
transition is recorded (`invalidation.rs`, receipts
`artifacts/assets/node-asset-invalidation-20260730-00{1,2}`). G2 (PostgreSQL
import) advances through bounded, inert, fixture-driven pilots — phase 3 ontology
adaptation, phase 4 code migration, phase 5 physics import (`physics_pilot.rs`,
receipt `artifacts/postgres-import/physics-pilot-20260730-001`: 4 profiles →
1 adapted_inert / 1 compatibility / 1 unresolved / 1 quarantined, 0 bound to the
live simulation, 4 activation refusals) — but remains incomplete: activation into
canonical authority is the deliberate final gap. G3 (Viz/Mind Desktop) remains
incomplete. The bootstrap completion gate is open. Highest-leverage
open gaps blocking bootstrap v0: graph triggers, authenticated production
transport, cryptographic Genesis signing, and the 10 M / 10 M scale proof.

Active parallel work streams (launched 2026-07-30):

1. Graph triggers + causal ancestry (critical path step 2-3).
2. Capability host completion (registry, scope, redaction, human gating).
3. Supervisor completion (budgets, crash recovery, health separation).
4. Graph IR control flow (bounded loops, calls, error/ON_ERROR paths).
5. `universe-postgres-import` compile-drift repair + G2 identity-only pilot.

## Product goals requested on 2026-07-30

### G1 — Complete the Node-to-Asset conversion

- [x] Inventory every Node that still lacks its required Asset projection and
  classify it as converted, partial, blocked, intentionally assetless, or
  unknown. Evidence: `universe-assets` census over the canonical corpus
  classifies all 234 Nodes (`fixtures/assets/node-asset-census-policy.json`),
  `unknown = 0`; receipt `artifacts/assets/node-asset-census-20260730-002`.
- [x] Define Asset identity, content pointers, hashes, versions, provenance,
  visual/physical mappings, lifecycle, and invalidation in graph authority.
  Identity (content-addressed `asset_id`), payload hashes, versions,
  `DERIVED_FROM`/`USES_MAPPING` provenance, lifecycle, and invalidation signals
  are materialized; physical mappings live in `physical_profile` Nodes; the
  *visual* mapping is now a graph authority too — `universe-assets/src/visual.rs`
  materializes the `visual-embodiment/1` catalog as a content-addressed Asset,
  bound by `semantic_type` (`PROJECTS_AS`), validated by the renderer's own
  budgets, with an epistemic-honesty invariant. Evidence:
  `artifacts/assets/visual-mapping-20260730-001` (parity + honesty held); the
  Mind Desktop renderer now consumes the fixture instead of hard-coding it.
- [x] Convert Nodes progressively into content-addressed Assets without making
  repository files, renderer objects, or generated bundles authoritative.
  Evidence: 26 required Nodes (3 `ontology_source` + 23 `ontology_contract`)
  converted through one attributable idempotent ChangeSet;
  `artifacts/assets/node-asset-conversion-20260730-002` (`canonical_node_replaced: false`).
- [x] Preserve the canonical Node and its stable identity; an Asset is a
  derived, reproducible projection and never a semantic replacement.
  Evidence: conversion receipt `nodes_preserved: true`; source content hashes
  unchanged after conversion.
- [x] Rebuild or invalidate an Asset deterministically when its authoritative
  Node, mapping, or revision changes. `universe-assets/src/invalidation.rs` runs
  a live, recorded transition: a `current` Asset is superseded when the
  authoritative mapping revision advances (1→2) — a new `current` Asset is built
  and an `INVALIDATED_BY` edge supersedes the old one, so effective lifecycle is
  derived structurally (append-only; the Node is never edited). Independent
  readback shows `current` 1→1 / `stale` 0→1 with the old Asset transitioning;
  deterministic + idempotent. Evidence:
  `artifacts/assets/node-asset-invalidation-20260730-002` (committing run, the
  transition) and `-001` (idempotent replay). Demonstrated trigger is the
  mapping-revision change; the supersession mechanism is trigger-agnostic.
- [x] Independently read every converted Asset back through the real store and
  report missing, stale, corrupt, duplicate, and orphaned projections
  separately. Evidence: independent reopen + re-census confirms 26 assets, 26
  unique asset_ids/payload hashes, 0 missing/corrupt/duplicate/orphaned.

### G2 — Import and adapt PostgreSQL Nodes progressively

- [ ] Define a bounded, resumable, read-only PostgreSQL source cursor with
  explicit source revision/watermark, batch size, timeout, and retry budgets.
- [ ] Preserve source IDs, source revision, import batch, provenance, content
  hashes, and adaptation receipts for every imported Node and relation.
- [ ] Express schema adaptation, type mapping, predicate mapping, compatibility,
  migration, rejection, and quarantine rules as graph mappings and approved
  ChangeSets rather than native import policy.
- [ ] Import symbols, Nodes, relations, and content pointers atomically in
  bounded batches with deterministic ordering and idempotency.
- [ ] Support dry-run/shadow validation, restart from the last committed
  watermark, conflict detection, and rollback without duplicating accepted
  Atoms.
- [ ] After every batch, reopen the Universe independently and verify counts,
  hashes, endpoints, provenance, rejected records, and the next source
  watermark before continuing.
- [ ] Publish throughput, latency, memory, rejection, retry, and drift evidence
  for progressively larger batches before claiming complete migration.

#### G2 source/target boundary

PostgreSQL is an import source, not a second live authority. The importer must
preserve the source exactly enough to audit and replay adaptation while making
no source row executable merely because it was active or executable in the old
runtime.

The source currently distinguishes:

- `mind_nodes`: graph, textual ID, node type, subtype, status, Space, JSON
  properties, revision, and timestamps;
- `mind_relations`: owning graph, global source/target IDs, relation type, JSON
  properties, revision, and timestamps;
- physics rows, Moments, execution claims, backfill checkpoints, and procedural
  PostgreSQL functions;
- graph-native definitions mixed with Python, Cypher, SQL, URIs, hashes, file
  projections, runtime status, and historical evidence.

The target distinguishes:

- stable Universe identity and an explicit source-to-target identity map;
- five universal Node types plus graph-owned semantic types;
- canonical, compatibility, unresolved, quarantined, and retired ontology
  bindings;
- inert content Assets, executable Graph IR `CodeDefinition` Atoms, native
  bootstrap primitives, and capability-mediated external effects;
- authoritative state, derived runtime state, historical observation, intent,
  claim, receipt, and independently measured outcome.

#### G2 identity and provenance contract

- [ ] Create a versioned `PostgresImportSource` Atom naming the source cluster,
  database/schema, source contract version, snapshot/watermark, and read-only
  capability; never store credentials in the Universe.
- [ ] Create an immutable `ImportIdentityMap` before importing relations. Map
  `(source authority, graph_id, source id)` to one stable Universe ID and record
  collision handling explicitly; never rely on array position, row number, or
  a lossy ID truncation.
- [ ] Preserve relation ownership separately from endpoint identity because
  PostgreSQL relation `graph_id` owns the edge while globally unique Node IDs
  may connect endpoints across graphs.
- [ ] Record source revision, timestamps, canonical source-row hash, batch ID,
  adapter revision, ontology-mapping revision, code-migration revision, and
  target Universe revision on every adaptation receipt.
- [ ] Treat PostgreSQL `status`, including the source view that defaults a null
  status to active, as source data only. Missing status remains unknown until a
  graph mapping resolves it.

#### G2 ontology adaptation matrix

| PostgreSQL source | Universe target | Activation rule |
| --- | --- | --- |
| `node_type` matching Actor/Moment/Narrative/Space/Thing | Candidate universal type binding | Accept only through a versioned mapping scoped to the source graph; spelling equality alone is insufficient. |
| `subtype`, `semanticType`, or type-like JSON properties | Semantic-type candidate Atom | Reuse an active canonical definition when exact; otherwise create compatibility, unresolved, or quarantined mapping evidence. |
| `relation_type` | Predicate binding candidate | Validate direction, endpoint constraints, cardinality, family, and ontology revision before activation. |
| Unknown Node or relation vocabulary | Inert source Atom plus ontology Problem | Preserve content and provenance; never invent the nearest canonical meaning. |
| JSON properties with no approved semantic mapping | Pointer-backed source content | Keep queryable as source payload but do not promote keys into canonical fields automatically. |
| PostgreSQL physics values or propagation coefficients | Historical measurement or mapping candidate | Never activate as current physics without units, epistemic state, context, mapping revision, bounds, and an approved ChangeSet. |
| `mind_moments` row | Historical Moment candidate | Preserve occurrence time and provenance; do not treat text or status as independent evidence. |
| Execution claim/lease | Historical intent or claim observation | A claim is not an `EffectReceipt`, execution receipt, measured outcome, or success. |

- [ ] Materialize this matrix itself as graph-owned mapping Atoms and approved
  ChangeSets before the importer can commit adapted canonical bindings.
- [ ] Require exact source-graph scope and mapping revision so two PostgreSQL
  graphs may interpret the same subtype or relation label differently.
- [ ] Preserve known absence, unknown, not measured, measurement failure,
  contradiction, and quarantine as distinct import outcomes.
- [ ] Never activate a predicate-to-physics mapping from relation properties or
  names alone.

#### G2 code and execution-strategy adaptation

Every imported Node that may represent code must first become an inert
`LegacyCodeAsset` plus a `CodeMigrationTask`. The importer classifies it into
exactly one of these graph-owned strategies:

| Source execution form | Target strategy | Default imported state |
| --- | --- | --- |
| Declarative graph behavior already expressible in Graph IR | Translate to a candidate `CodeDefinition` Atom cluster | `translated_inert` |
| Python NodeCode, listener, scheduler, or dispatcher | Extract behavior contract, inputs, outputs, budgets, effects, and error paths; rewrite variable behavior in Graph IR | `migration_required` |
| Cypher query or traversal program | Translate only bounded local-query semantics; reject implicit whole-graph traversal | `migration_required` |
| PostgreSQL function or trigger containing policy | Separate storage primitive from ontology/decision/physics policy; retain only indispensable generic bootstrap semantics natively | `quarantined_policy_split` |
| External transport/tool invocation | Graph IR capability intent plus authorized adapter and real receipt | `capability_disabled` |
| Deterministic generated file or URI/content hash | Content-addressed Asset projection linked to its canonical definition | `inert_asset` |
| Historical trace, result, status, or execution claim | Observation/claim evidence, never executable code | `non_executable_evidence` |
| Obsolete, duplicated, unsafe, or semantically unresolved code | Preserved source Asset plus retirement/quarantine Problem | `quarantined` |

- [ ] Detect code-bearing fields and relations through graph mappings, not a
  growing native list of PostgreSQL property names.
- [ ] Pin translated code to source-row hash, source revision, ontology mapping,
  CodeDefinition revision, capability declaration, and execution budgets.
- [ ] Reject imports with undeclared effects, unbounded traversal/loops,
  missing cancellation or timeout, hidden predicate dispatch, missing
  capabilities, or a source/target ontology gap.
- [ ] Never execute imported Python, Cypher, SQL policy, shell text, URI, or
  generated file as a fallback when Graph IR translation is missing.
- [ ] Keep current source executions isolated during migration; importing a
  definition must not transfer its lease, scheduler status, trigger wiring, or
  authority to execute.

#### G2 activation state machine

```text
source_observed
→ imported_inert
→ identity_resolved
→ ontology_classified
→ relations_resolved
→ code_classified
→ translated
→ validated
→ compiled
→ shadow_executed
→ independently_compared
→ approved_changeset
→ activated_for_later_execution
```

Any stage may instead produce:

```text
unknown | not_measured | measurement_failed | conflict | rejected | quarantined
```

- [ ] Persist every transition as an import/adaptation receipt and make state
  advancement monotonic for one source revision.
- [ ] A later source or mapping revision creates a new candidate; it never
  mutates the CodeDefinition pinned to an execution already in progress.
- [ ] Shadow execution must use declared identical inputs and budgets, prohibit
  real external effects, compare result, write set, trace, traps, energy, and
  epistemic states, and preserve non-equivalence as evidence.
- [ ] Activation requires an approved ChangeSet and affects only subsequent
  triggers; compilation or source status alone cannot activate code.

#### G2 progressive delivery phases

1. **Fresh census and compatibility report**
   - [ ] Read source schema/version, graphs, counts, ID uniqueness, dangling
     endpoints, type/subtype/relation vocabularies, property-key distributions,
     code-bearing candidates, physics rows, Moments, claims, and checkpoints.
   - [ ] Publish unknowns and measurement failures; do not reuse an older
     census as current evidence.
2. **Identity-only pilot**
   - [~] Import one bounded graph/Space slice as inert source Atoms with
     identity maps, row hashes, content pointers, and no active ontology or
     execution. The importer runs against
     `fixtures/import/postgres-identity-pilot.json`: 12 inert nodes, 0
     executable, ontology inactive, 2 relations quarantined, 1 cross-graph edge
     preserved, `source_status: null` not defaulted, 96 content records read
     back from a fresh store with hashes byte-compared. Import policy lives in
     the manifest, not Rust. Broader/progressive slices remain open.
3. **Ontology pilot**
   - [ ] Adapt the five universal types first, then a small reviewed vocabulary
     batch; keep unknown predicates and endpoint mismatches quarantined.
4. **Data and relation batches**
   - [ ] Commit Nodes before their relations, preserve cross-graph endpoints,
     and stop a batch atomically on missing identity or content/hash mismatch.
5. **Code pilot**
   - [ ] Select one side-effect-free declarative CodeDefinition candidate,
     translate it to Graph IR Atoms, compile, shadow-run, compare, approve,
     activate for later execution, and independently read every receipt back.
6. **Capability and effect pilot**
   - [ ] Migrate one safe external effect through capability validation,
     disabled-by-default activation, real transport receipt, reinjection, and
     independent readback.
7. **Progressive expansion**
   - [ ] Increase batch and graph scope only while integrity, ontology,
     translation, shadow-equivalence, runtime-health, and Viz lag stay within
     declared bounds.

#### G2 batch commit and readback gate

One accepted batch must atomically publish:

```text
ImportBatch
→ source watermark and row hashes
→ identity-map additions
→ compact-symbol additions
→ inert source Atoms and content pointers
→ adapted Nodes and relations
→ ontology/code Problems and quarantines
→ adaptation receipts
→ next resumable cursor
```

- [ ] Reopen the store after each batch and independently verify the exact
  source rows represented, identity-map uniqueness, content hashes, endpoint
  closure, mapping revisions, receipt hashes, rejected/quarantined counts, and
  next cursor.
- [ ] Do not advance the source watermark when any required publication or
  readback fails.
- [ ] Expose committed/imported/adapted/compiled/shadowed/activated as separate
  Viz states so progressive visibility never implies executable authority.

#### G2 completion criteria

- [ ] No PostgreSQL label, JSON key, status, physics coefficient, procedure, or
  code string becomes canonical or executable without an explicit graph-owned
  mapping and evidence.
- [ ] Source data, adapted semantic state, derived Assets, executable
  CodeDefinitions, runtime state, and receipts remain distinguishable and
  independently readable.
- [ ] The importer can stop after any committed batch, restart from the exact
  watermark, replay without duplication, and quarantine a bad row without
  weakening atomicity or epistemic distinctions.
- [ ] One representative candidate from every execution-strategy class has a
  measured migration outcome: equivalent, intentionally changed, retired,
  unresolved, or rejected.
- [ ] Progressive import is visible in Mind Desktop from the real runtime
  stream, while unactivated or quarantined code is visibly inert.

### G3 — Deliver a functional Viz

- [ ] Connect Mind Desktop to the authenticated production
  snapshot/delta/event protocol with reconnect, sequence-gap detection, resync,
  backpressure, and honest stale/degraded states.
- [ ] Render the complete bounded local situation actually received from the
  Universe: Nodes, Assets, relations, active CodeDefinitions, loops, gates,
  objectives, decisions, evidence, receipts, health, folds, and residency.
- [ ] Read all semantic, visual, physical, and interaction mappings from graph
  authority; do not hard-code canonical predicate or Node meaning in
  TypeScript, shaders, or renderer dispatch.
- [ ] Make imported PostgreSQL Nodes and converted Assets progressively visible
  as their authoritative batches commit, without whole-Universe scans.
- [ ] Provide usable world-native navigation, focus, selection, expansion,
  release, replay trails, epistemic distinctions, and Actor/Observer
  interaction without default product panels.
- [ ] Add deterministic visual fixtures and screenshot/video regression tests
  for normal operation, partial data, stale state, resync, failed receipts,
  broken health, Asset invalidation, and progressive import.
- [ ] Measure frame time, event-to-photon latency, CPU/GPU memory, draw calls,
  stream bandwidth, and active-region limits on declared hardware and data
  sizes.

### Product-goal completion gate

- [ ] Every required canonical Node has either a verified current Asset or an
  explicit graph-owned reason why no Asset must exist.
- [ ] PostgreSQL import can resume safely and every accepted batch has
  independently readable provenance and adaptation receipts.
- [ ] A user can launch Mind Desktop, connect to the real runtime, observe the
  progressively imported and materialized local Universe, interact with it,
  lose and restore the stream, and see authoritative recovery without invented
  state.

## Status legend

- `[ ]` not started
- `[~]` in progress
- `[x]` completed and evidenced
- `[!]` blocked; blocker and attempted recovery must be recorded

## Coordination rules

- No E2E item may be marked complete using only component tests.
- No graph behavior may be implemented as hidden Rust or TypeScript policy.

## Current critical path

This is the execution order for bootstrap v0. A later item must not hide or
work around an earlier missing primitive.

1. **Make graph authority activatable**
   - [~] Keep the canonical ontology 1.17.0 and the ontology/physics overlay as
     authoritative store data with independently verified content pointers.
   - [x] Activate approved graph ChangeSets as deterministic ontology overlays
     with ordered membership, explicit diagnostics, and an authority hash.
   - [x] Intern compact symbols and their referring records atomically in one
     revision with deterministic ID assignments.
   - [x] Compile graph-defined `BehaviorBond` records into content-addressed
     `RuntimeBondPlan` artifacts without predicate-name dispatch.
   - [x] Query the stored BehaviorBond locally, materialize its relation-owned
     bindings, compile and execute a bounded plan, close measured health, commit
     every receipt, reopen the store, and reverify all hashes.
2. **Close the trusted mutation path**
   - [x] Support atomic multi-command transactions and prove
     boot → mutate → checkpoint → crash → replay equivalence.
   - [x] Connect graph-resolved relation deltas to generic physical commands at
     the exact committed tick, require commit revision/idempotency provenance,
     and independently read the resulting bindings back.
   - [ ] Preserve causal ancestry from trigger through observation/effect
     receipts.
3. **Close graph-native execution**
   - [ ] Complete bounded control flow, triggers, capabilities, and error paths
     in Graph IR.
   - [ ] Move every variable behavior out of native dispatch and into
     CodeDefinitions, mappings, policies, and ChangeSets.
4. **Expose the real runtime**
   - [ ] Freeze protocol v0 and implement authenticated, backpressured,
     resynchronizable event streams.
   - [ ] Connect Mind Desktop and one Actor through that production protocol.
5. **Prove resilience and scale**
   - [ ] Pass crash, corruption, budget, conflict, storm, and failed-effect
     stories.
   - [ ] Publish fresh 10 M entity / 10 M relation and active-physics
     measurements with p50, p95, p99, memory, and raw artifacts.

### Graph-owned open decisions

- [ ] Resolve the missing physical constraints and profiles for `BASED_ON` and
  `PROPOSES_CHANGE_TO` through an approved graph ChangeSet.
- [ ] Decide in graph authority whether each of the 12 compatibility predicates
  is promoted, migrated, aliased, or retired.
- [ ] Define graph-owned version selection, activation, supersession, and
  rollback rules for ontology and physics mappings.
- [ ] Define the graph schema for objectives, mechanisms, eligibility gates,
  justifications, counterevidence, decisions, intents, receipts, outcomes, and
  Loop health outside the proof fixture.
- [ ] Define graph-owned energy laws, normalization, conservation bounds,
  thresholds, decay, learning rates, and health closure criteria.
- [ ] Define how a failed or contradictory justification opens a Problem,
  quarantine intent, repair objective, or counterfactual experiment.
- [ ] Promote the structurally measured ontology/physics overlay only after the
  runtime receipts above exist; structural co-validity alone is not execution
  evidence.

---

## Core truth and persistence

### Paths

```text
Cargo.toml
crates/universe-core/
crates/universe-store/
crates/universe-testkit/
fixtures/genesis/
fixtures/ontology/
tools/reconstruct-ontology.mjs
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
- [x] Publish contract fixtures for other subsystems.

### P0 entity and relation stores

- [ ] Implement a generational entity arena.
- [ ] Implement structure-of-arrays entity columns.
- [x] Implement deterministic compact symbol planning and transactional
  interning at ingestion boundaries.
- [ ] Implement a dense relation arena.
- [x] Implement deterministic CSR adjacency for immutable snapshots, bound to
  the authoritative Universe ID, revision, and snapshot hash.
- [x] Implement bounded mutable adjacency overlays over one exact immutable CSR
  revision, with explicit entity, relation, tombstone, endpoint, and event
  budgets.
- [x] Implement generation-checked relation tombstones through the atomic
  mutation path and hide tombstoned base relations from local reads.
- [x] Implement deterministic overlay compaction through an atomic durable
  versioned checkpoint/event-log rollover. Refuse stale views, verify checkpoint
  revision/hash from the filename, reopen with an empty overlay, and recover
  without loss or duplication across each tested rollover crash window.
- [ ] Add property tests for allocation, reuse, stale handles, and endpoints.

### P0 content store

- [x] Implement immutable append-only JSONL segments (single segment in v0).
- [x] Return a stable `ContentPtr` after durable append.
- [x] Compute and verify full content hashes.
- [x] Implement direct offset/length reads without scans.
- [ ] Implement a bounded hydration cache.
- [x] Detect truncated records and corrupted content on direct read.
- [ ] Track orphan content created before failed commits.
- [ ] Add safe orphan collection based on committed references.

### P0 ontology/physics Atom cluster

- [x] Allow entity and relation records to reference verified JSONL content.
- [x] Install an inline graph seed into an empty authoritative store.
- [x] Reconstruct the canonical ontology 1.17.0 as a versioned local GraphSeed
  from the executable schema, its 120-node/70-link doctrine mirror, and the L4
  0.4.0 mapping.
- [x] Represent 5 stored node types, 34 semantic types, 20 relation families,
  55 canonical predicates, 11 epistemic statuses, and 23 schema contracts as
  first-class store Atoms.
- [x] Bind all 34 semantic types to their stored physical node role and all 65
  recovered predicate profiles to first-class prototype Atoms.
- [x] Preserve the 12 residual constraint/profile predicates as explicit
  compatibility definitions rather than silently promoting them to canonical.
- [x] Preserve missing constraints and physical profiles for `BASED_ON` and
  `PROPOSES_CHANGE_TO` as unresolved graph-owned gap Atoms.
- [x] Verify all 1,015 content records by pointer/hash after independent store
  reopen and publish a reconstruction receipt.
- [x] Store a graph ChangeSet and ontology/physics projection contract defining
  `BehaviorBond`, `LogicRole`, gate, objective, profile, and justification
  relations without hard-coding canonical predicates in physics dispatch.
- [x] Independently reopen and verify the projection overlay: 267 entities, 862
  relations, 78 overlay relations, revision/tick 114, and snapshot hash
  `0810b844cdcb5fc8b203e068b24f043ada0a08a8b1e7682b0d61625ecbb1307f`.
- [x] Publish a structural-validation receipt with the honest status
  `measured_structural_validity_runtime_pending`.
- [x] Resolve approved graph ChangeSets into the active ontology registry and
  expose active members, diagnostics, revisions, and authority hashes.
- [x] Publish new symbols and referring graph records atomically, then reopen
  and independently verify the assigned IDs.
- [x] Encode ontology types, predicates, justifications, Atom gates, and Bond
  polarity in one versioned cluster fixture.
- [x] Require real store readback, deterministic traces, and a conserved energy
  ledger before injecting measured observations.
- [x] Close co-validity only for the declared fixture context.
- [x] Prove that an active `CONTRADICTS` Bond inhibits the same closure.
- [x] Publish the measured receipt through the event log and independently
  replay and verify its content hash.
- [x] Encode one objective, two mechanism options, five non-compensatory
  eligibility gates, a decision, an effect intent, a receipt, and a measured
  outcome as Atoms and Bonds.
- [x] Close `loop_health` only from independent epistemic, operational, and
  outcome health branches.
- [x] Feed successful `loop_health` into a graph-owned
  `INCREASES_PROPENSITY` learning result.
- [x] Prove that a missing effect receipt blocks operational and Loop health.
- [x] Prove that decision counterevidence blocks both decision and effect
  intent.
- [x] Publish and independently replay a second Loop Loop receipt.

### P0 snapshots and replay

- [x] Define `UniverseSnapshot`.
- [x] Define `UniverseEventLog` records with checksum and idempotency key.
- [x] Implement atomic checkpoints.
- [x] Implement replay from snapshot plus log.
- [x] Detect and recover a truncated final log record.
- [ ] Refuse mid-log corruption.
- [ ] Add snapshot format migrations.
- [~] Provide a minimal hash-identified Genesis Graph fixture.
- [ ] Add cryptographic Genesis signing, trust-root selection, signature
  verification, key rotation, and rejection tests.
- [x] Prove boot → mutate → checkpoint → crash → replay equivalence.
- [x] Add atomic multi-command event publication with all-or-nothing replay.
- [~] Add deterministic revision compaction without changing stable identities
  or content hashes. Exact-revision checkpoint/event-log rollover preserves the
  truth hash; multi-revision retention/garbage collection remains open.
- [ ] Add backup, restore, and disaster-recovery tooling with readback evidence.

### P1 scale proof

- [ ] Measure exact per-entity and per-relation resident bytes.
- [ ] Generate 1 M entities / 1 M relations.
- [ ] Generate 10 M entities / 10 M relations.
- [ ] Measure load time, checkpoint time, replay time, and memory.
- [ ] Measure adjacency build/overlay/compaction time and symbol-table cost.
- [ ] Measure content-segment growth, hydration hit rate, and orphan collection.
- [ ] Publish benchmark data for Agent 5.

### Subsystem done criteria

- [ ] All shared contracts have stable versioned fixtures.
- [ ] 10 M truth-layer entities can load without any Rapier body.
- [ ] Snapshot/replay produces the same IDs, revisions, and hashes.
- [ ] Dependent subsystems can use the contracts without private replacements.

---

## Physics, locality, fields, and folds

### Paths

```text
crates/universe-physics/
crates/universe-query/
crates/universe-fields/
benches/physics/
```

### P0 Rapier host

- [x] Wrap Rapier behind `UniversePhysics`.
- [x] Map `EntityKey` to bodies and colliders.
- [x] Map `RelationKey` to graph-resolved physical bindings with add, replace,
  tombstone, and release semantics.
- [x] Define `PhysicsCommand`, `PhysicsDelta`, and `PhysicsEvent` using Agent 1
  contracts.
- [x] Implement fixed timestep and deterministic command ordering.
- [~] Detect NaN, infinities, excessive velocity, and energy explosions. Finite
  state validation exists; velocity and energy containment remain open.
- [x] Add bounded local rollback for faulty relation binding batches.
- [ ] Expose active bodies, contacts, sensors, and solver health.
- [x] Implement generic discrete Atom dynamics for support, inhibition, and
  neutral Bonds without naming canonical predicates in native dispatch.
- [x] Compile an active-store graph `BehaviorBond` into a `RuntimeBondPlan`
  exclusively from its relation-owned bindings and measured contents.
- [x] Apply runtime plans only to an explicitly bounded local Atom cluster.
- [x] Emit artifact/plan provenance, energy ledger, convergence, starvation,
  containment, lifetime, and release evidence.
- [x] Add negative tests for a missing logic role and an unresolved ontology
  gap.
- [x] Prove identical physical execution when only the semantic predicate ID
  changes while all resolved physical inputs remain constant.
- [x] Derive strictly physical health evidence for convergence, energy,
  containment, lifetime, and release while preserving Unknown, NotMeasured, and
  MeasurementFailed.

### P0 residency

- [~] Implement `Hot`, `Sleeping`, `Aggregated`, and `Dormant`. `Hot` and
  `Dormant` exist; `Sleeping` and `Aggregated` do not.
- [x] Materialize a bounded dormant entity set into Rapier.
- [x] Dematerialize released entities back to compact dormant state.
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
- [x] Return exact inspected binding relations, endpoints, and unvisited
  frontier from a bounded local read.
- [x] Replace the test/setup snapshot scan with direct immutable Store
  adjacency/CSR lookup by `EntityKey`, bound to the authoritative snapshot
  revision and hash.

### P0 ReadField and TopologicalFold

- [x] Define `ReadField` lifecycle.
- [x] Consume graph-provided selectors and budgets without native policy.
- [ ] Define and consume `SpaceSummary`.
- [ ] Rank resonating Spaces using graph-provided scoring parameters.
- [x] Implement `TopologicalFold`.
- [ ] Implement fold appearance, stabilization, decay, cancellation, and reuse.
- [ ] Allow multiple Actors to fold toward one Space without duplicating it.
- [x] Emit `read_started` through `read_released` events.
- [ ] Make fold lifetime, energy, wake cost, and materialization budgets explicit
  in graph data and enforce their native upper bounds.
- [ ] Observe and commit fold health independently instead of treating visual or
  physical stabilization as semantic success.

### P1 field and counterfactual engines

- [ ] Implement attraction, inhibition, and attention field primitives.
- [ ] Implement generic gate, support, inhibition, propagation, threshold,
  normalization, and conservation primitives.
- [ ] Implement lazy temporal laws.
- [ ] Implement event-driven propagation frontiers.
- [ ] Implement cluster summaries with declared error bounds.
- [ ] Expand clusters when requested precision exceeds the bound.
- [ ] Implement local ghost/counterfactual simulation.
- [ ] Guarantee ghost simulations cannot emit external effects.
- [ ] Compare mechanism alternatives in ghost state and return evidence without
  mutating authoritative state.
- [ ] Bound oscillation, non-convergence, causal amplification, and energy leak
  detection per local island.

### P1 physics scale proof

- [ ] Benchmark 10 k active bodies.
- [ ] Benchmark 50 k active bodies.
- [ ] Benchmark 100 k active bodies.
- [ ] Benchmark dense contacts and sparse contacts separately.
- [ ] Benchmark large joint components and wake storms.
- [ ] Report tick p50, p95, p99, memory, and active constraint counts.

### Subsystem done criteria

- [ ] A local query is bounded by its working set, not total Universe size.
- [ ] A Space can fold, expand, hydrate, contract, and return to warm/dormant.
- [ ] Physics failures remain local and produce measured health.
- [ ] No ontology-specific predicate policy is hidden in Rust.

---

## Graph IR, compiler, VM, and triggers

### Paths

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
- [x] Define boolean and comparison opcodes.
- [~] Define branch, call, and return. Forward branch and return are proven;
  call remains open.
- [ ] Define bounded `FOR_EACH`, `REPEAT_N`, and `UNTIL_WITH_LIMIT`.
- [x] Define write-set construction opcodes.
- [ ] Define physics command opcodes.
- [ ] Define capability intent opcodes.
- [ ] Define error and `ON_ERROR` paths.
- [ ] Define `NEXT`, `INPUT`, `OUTPUT`, `BODY`, `TRUE_NEXT`, and `FALSE_NEXT`.
- [ ] Define evidence, receipt, epistemic-state, and bounded-measurement values.
- [~] Define explicit `UNKNOWN`, `NOT_MEASURED`, and
  `MEASUREMENT_FAILED` control paths; never coerce them to false or zero. The
  `BranchOnEvidence` opcode routes all six epistemic states to distinct
  graph-declared successors without coercion (fixture
  `fixtures/graph-ir/evidence-branch.json`; validator + VM tests, interpreted ==
  compiled). `ON_ERROR` and receipt/measurement value paths remain open.
- [x] Keep all program parameters and composition in graph data.

### P0 validator

- [x] Validate entrypoints.
- [x] Validate input/output types.
- [x] Detect unbounded control-flow cycles.
- [~] Require budgets for queries and loops. Query and BehaviorBond budgets are
  enforced; general bounded loop opcodes remain open.
- [ ] Require declared capabilities.
- [ ] Detect orphan operators and unreachable exits.
- [ ] Validate that every physical behavior identifies source, target, predicate,
  profile, logic role, gates, objective, and justification requirements.
- [ ] Validate non-compensatory gates and prohibit health closure when required
  evidence is absent, stale, contradictory, or failed.
- [x] Detect registers read before assignment.
- [x] Produce a canonical CodeDefinition hash.
- [~] Produce graph-readable validation reports. BehaviorBond reports are typed
  and serializable; general CodeDefinition reports remain open.

### P0 compiler

- [x] Compile Graph IR to compact bytecode.
- [x] Allocate registers deterministically.
- [ ] Resolve branches and constants.
- [x] Produce source mappings from instruction to canonical node.
- [ ] Store bytecode by content hash.
- [ ] Invalidate artifacts after CodeDefinition revision changes.
- [ ] Reject mismatched artifacts.
- [x] Compile approved ontology/physics mapping overlays into content-addressed
  `RuntimeBondPlan` artifacts from measured active-store projections.
- [x] Keep every predicate-specific coefficient and policy in graph data.
- [x] Emit a compilation receipt naming graph authority revisions/hashes,
  behavior hash, artifact hash, budgets, and typed validation result.

### P0 VM

- [x] Implement typed execution frames.
- [x] Pin code and starting Universe revisions.
- [ ] Implement fuel, call depth, loop, mutation, and tick budgets.
- [ ] Implement cancellation and timeout.
- [x] Produce a write set, never a direct mutation.
- [x] Implement deterministic traps for budget exhaustion.
- [x] Emit structured per-op traces.
- [x] Produce `ExecutionReceipt`.
- [x] Execute physical behavior clusters with bounded energy, deterministic
  ordering, and a complete physical evidence ledger.
- [x] Pin ontology, mapping, behavior, ChangeSet, context, and Universe authority
  revisions/hashes in the compiled plan.
- [ ] Keep a debug interpreter and compare it with compiled execution.

### P0 triggers

- [ ] Define trigger subscription records.
- [ ] Convert Universe events into bounded execution requests.
- [ ] Implement idempotency keys and causal ancestry.
- [ ] Enforce cooldown, debounce, and maximum causal depth.
- [ ] Detect trigger cycles and storms.
- [ ] Produce DMZ/quarantine intents instead of silently dropping failures.
- [ ] Add triggers for approved ChangeSets, local observations, effect receipts,
  health failures, scheduled ticks, and operator requests.
- [ ] Prove that changing a CodeDefinition or mapping affects only later
  executions.

### P1 migration fixture

- [ ] Represent local Narrative discovery entirely in Graph IR.
- [x] Open a ReadField through Agent 2 primitives.
- [ ] Await stable/partial completion.
- [~] Filter, Top-K, hydrate, and return results without Python or Cypher. The
  first slice uses one fused bounded local-query primitive; general Graph IR
  filter/Top-K/hydration operators remain open.
- [x] Create a proposed result Moment write set.

### P1 graph-native behavior library

- [ ] Encode reusable graph clusters for query, branch, choice, bounded loop,
  trigger, propagation, health closure, learning, repair, and effect handling.
- [ ] Encode the Loop Loop as objectives → mechanism candidates → eligibility
  gates → decision → intent → receipt → measured outcome → health → learning.
- [ ] Make justifications and counterevidence executable inputs, not descriptive
  metadata.
- [ ] Add versioned graph fixtures for success, unknown, missing evidence,
  contradiction, failed effect, non-convergence, and repair.
- [ ] Support self-verification through independent observations and proof
  obligations without allowing a program to declare its own success.
- [ ] Support graph-authored tests and expected epistemic outcomes executed by
  the same bounded VM.
- [x] Materialize a BehaviorBond from ID-only vocabulary, relation records,
  independently hashed contents, and measured local-read evidence.
- [x] Require measured objective and justification ContentRefs in the canonical
  projection hash without interpreting their semantics natively.
- [x] Evaluate three-branch compilation/physical/readback health with strict
  epistemic preservation through a stored graph `CodeDefinition`: 26 typed
  evidence inputs, 21 non-compensatory proof obligations, and 39 bounded VM
  instructions.
- [x] Reopen the health `CodeDefinition`, execute it through the real VM, persist
  its `ExecutionReceipt` and measured Loop health, and independently read both
  back.
- [x] Remove the temporary native health oracle after the graph-owned fixture
  proves closed, open, unknown, not-measured, failed, contradictory, and
  mismatched-hash outcomes.

### Subsystem done criteria

- [x] One graph-defined first-slice program validates, compiles, executes, and
  traces.
- [ ] The same fixture has equivalent interpreter and bytecode results.
- [ ] Self-modification creates a later CodeDefinition revision without changing
  the current execution.
- [x] No Python NodeCode or Cypher is invoked by the first-slice migration
  fixture.
- [x] The active-store BehaviorBond materializes, compiles, executes, persists
  receipts, and closes measured health with no predicate-specific native branch.

---

## Transactions, supervisor, capabilities, and protocol

### Paths

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
- [~] Implement base-revision and precondition checks. Base-revision conflicts
  are enforced; generic graph preconditions remain open.
- [~] Validate endpoints, content references, and primitive invariants. Endpoint
  and basic hash checks exist; complete typed invariant validation does not.
- [x] Commit only at tick boundaries.
- [x] Prevent partially visible mutations.
- [~] Produce commit, conflict, validation, and rollback receipts. Successful
  commit/idempotency receipts exist; structured conflict, validation, and
  rollback receipts remain open.
- [x] Remove the one-command transaction limit: a validated multi-command batch
  publishes as one event and one revision, or publishes nothing.
- [x] Connect committed graph-resolved relation mutations to Agent 2 physical
  commands at their exact tick and return measured application plus independent
  relation-binding readback.
- [~] Preserve causal ancestry from trigger to receipt. Write sets and commit
  receipts carry ancestry. `CausalHop::canonical_token()` +
  `ExecutionRequest::descendant_causal_ancestry/tokens()` now bridge structured
  trigger identity into the opaque write-set/commit ancestry without a type
  change (compiler test proves a re-firing subscription is caught as
  `CausalCycle`). Still open: populate write-set ancestry from this bridge in
  `behavior_runtime.rs` and read the chain back as graph-owned receipt evidence.
- [ ] Commit graph ChangeSet activation, symbol manifest, mapping revision, and
  derived physical delta as an auditable atomic transition.

### P0 supervisor

- [x] Implement the boot state machine.
- [x] Own the Universe clock and deterministic phase ordering.
- [x] Schedule Graph VM execution.
- [x] Schedule transaction commits.
- [x] Schedule field and physics phases.
- [ ] Implement CPU, memory, wake, and backlog budgets.
- [ ] Implement degraded operation without renderer or optional adapters.
- [ ] Implement checkpoints and crash recovery.
- [~] Expose tick, revision, state, backlog, and health. `SupervisorStatus`/
  `status()` exposes tick, revision, state, and a real commit backlog count;
  richer backlog signals remain open.
- [ ] Detect stuck scheduler, log, checkpoint, and physics phases.
- [~] Separate liveness, readiness, data integrity, execution health, physics
  health, effect health, and semantic/Loop health. `SupervisorHealth` carries
  the seven dimensions as independent `Epistemic<HealthLevel>` values; only
  readiness is `Measured` from owned boot state, the other six honestly return
  `NotMeasured` until backed by real evidence (test refuses fabrication).
- [ ] Persist supervisor transitions and measured failures as Universe evidence.
- [ ] Add bounded repair scheduling, quarantine, retry, cooldown, and escalation
  without embedding organization-specific policy.

### P0 capability host

- [x] Define `EffectIntent` and `EffectReceipt`.
- [~] Implement a versioned capability registry. `CapabilityRegistry`
  (versioned) + `CapabilityDeclaration` exist and gate transport when attached;
  still needs to be materialized from the store and wired into the supervisor's
  host so declarations are graph-sourced rather than test-constructed.
- [ ] Keep secrets outside graph snapshots and logs.
- [~] Implement timeout, cancellation, and idempotency. Deadline checks and
  idempotency exist; active cancellation remains open.
- [x] Capture actual transport success/failure.
- [x] Reinject effect receipts through graph-owned translation into a
  tick-boundary write set and independently read back the full content/hash.
- [x] Add one safe echo test adapter.
- [ ] Add explicit human gating for irreversible adapters.
- [~] Enforce capability scope, principal, target, payload limits, causal depth,
  cooldown, and expiry before transport. Payload-byte, causal-depth, and expiry
  (deadline) gates run pre-transport and persist idempotent denials; principal/
  target/cooldown await `EffectIntent` carrying those fields (no invented
  defaults).
- [~] Prove secrets and sensitive payloads are redacted from snapshots, logs,
  traces, errors, and renderer events. A `sensitive` declaration redacts the
  transport response/reason before persistence, with a test asserting the secret
  bytes are absent from the stored receipt and its read-back; log/trace/renderer
  redaction coverage remains open.
- [~] Persist attempt, timeout, cancellation, duplicate, success, and failure
  receipts with transport-provided identifiers. Success, transport failure,
  pre-transport deadline, and idempotent duplicate semantics are modeled;
  cancellation and production transport identifiers remain open.

### P0 protocol

- [x] Define versioned situation snapshot, delta, Universe event, local query,
  direct read, receipt, heartbeat, acknowledgement, and resynchronization
  messages.
- [ ] Implement binary channels for high-volume streams.
- [x] Add monotonic stream sequence numbers, acknowledgement retention, bounded
  resume, and explicit snapshot-required resynchronization.
- [x] Add frame/byte backpressure and bounded queues without consuming a
  sequence number when publication is rejected.
- [ ] Add Actor/Observer authentication and capabilities.
- [ ] Enforce content visibility before hydration.
- [ ] Implement asynchronous `graph_read` handles and event streams.
- [~] Add protocol negotiation, compatibility tests, heartbeat, reconnect, and
  snapshot-plus-delta recovery. The v0 contracts and in-memory recovery state
  machine are proven; production transport reconnect remains open.
- [x] Distinguish accepted, committed, executing, measured, failed, stale, and
  unknown states on the wire and through serialization round trips.
- [ ] Add bounded pagination/streaming for large local situations without a
  whole-Universe export path.

### P0 real boot

- [ ] Load configuration and secrets.
- [x] Open stores and content segments.
- [x] Load or create the Genesis revision.
- [x] Replay and validate the Universe.
- [x] Load, validate, and compile CodeDefinitions.
- [ ] Restore hot chunks and physics.
- [~] Start VM, scheduler, clock, and capabilities. The headless first-slice
  supervisor runs phases and VM execution; production services and capability
  lifecycle remain open.
- [ ] Open IPC/network only after coherent readiness.
- [ ] Expose honest `Ready`, `Degraded`, `Recovering`, and `Blocked`.
- [ ] Replace the minimal boot CLI with a long-running supervised server.
- [ ] Add graceful shutdown, checkpoint drain, in-flight execution cancellation,
  and restart-safe idempotency.
- [ ] Add authenticated operator diagnostics that cannot mutate the Universe
  outside the transaction/capability path.

### P1 security and operations

- [ ] Define the trusted computing base and threat model.
- [ ] Authenticate Actors, Observers, operators, adapters, and local clients.
- [ ] Authorize graph reads, hydration, mutation, execution, and effects through
  versioned capabilities.
- [ ] Encrypt sensitive content at rest and in transit where required.
- [ ] Add audit-log integrity, retention, export, and independent verification.
- [ ] Add structured logs, metrics, traces, crash reports, and correlation IDs
  without treating observability as authority.
- [ ] Add deployment configuration validation and fail closed on unsafe
  defaults.
- [ ] Write operator runbooks for boot, upgrade, backup, restore, corruption,
  degraded mode, quarantine, and emergency recovery.

### Subsystem done criteria

- [ ] No effect can bypass capability validation and receipt capture.
- [ ] A crash during every transaction phase recovers coherently.
- [ ] Clients can miss deltas and resynchronize.
- [ ] The supervisor contains no graph behavior or ontology-specific decisions.

---

## Desktop, benchmarks, and complete E2E

### Paths

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
- [x] Consume Agent 4 supervisor and fresh-store readback API in the headless
  harness.
- [ ] Run the same story through the production network/IPC protocol rather than
  direct in-process calls.
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

- [x] Create the Tauri v2 shell and React Three Fiber world.
- [~] Implement snapshot/delta/event state reduction with sequence-gap
  detection. The reducer exists; no real runtime transport feeds it yet.
- [ ] Connect the Desktop to authenticated production snapshot/delta/event
  streams with reconnect, resync, backpressure, and honest stale state.
- [~] Render generic Atom bodies and Bond geometry. Canonical node categories
  still require graph-provided visual mappings before they may look distinct.
- [ ] Render aggregate Space bodies and progressive expansion.
- [ ] Render folds and ReadFields.
- [x] Make active logical Bonds visibly luminous.
- [ ] Render CodeDefinitions, conditions, loops, triggers, gates, evidence, and
  execution as world geometry and motion, never as text panels.
- [ ] Render objectives as attractors, eligible mechanisms as navigable paths,
  gates as pass/block topology, and decisions as visible state transitions.
- [ ] Render justifications and counterevidence as inspectable luminous chains
  linking claims to observations, receipts, and outcomes.
- [ ] Render epistemic state diegetically so measured, unknown, not measured,
  failed, partial, stale, and contradictory cannot be confused.
- [ ] Render Loop health as the observable closure or breakage of the complete
  justification loop; never infer it from equilibrium or animation alone.
- [ ] Implement semantic LOD.
- [x] Add ObserverCamera controls separate from ActorInstance.
- [ ] Add Actor embodiment and capability-mediated interaction with the world.
- [ ] Replace inspector, timeline, trace, and health panels with diegetic
  traversal, focus, replay trails, spatial labels, and world-native feedback.
- [ ] Keep the default experience full-screen and panel-free; diagnostics may
  only exist as an explicit developer/debug mode.
- [ ] Read visual mappings from graph data.
- [ ] Do not hard-code the canonical predicates in TypeScript.
- [ ] Stream local situations only; expand, aggregate, and release regions under
  explicit renderer and server budgets.
- [ ] Add smooth reconciliation for authoritative corrections without hiding
  rollback, staleness, or failed state.
- [ ] Add input remapping, reduced-motion mode, readable contrast, captions for
  non-visual signals, and keyboard/controller parity.
- [ ] Profile frame time, GPU/CPU memory, draw calls, network bandwidth, and
  event-to-photon latency at declared world sizes.
- [ ] Add deterministic visual fixtures and screenshot/video regression tests
  for active Bonds, folds, gates, evidence chains, health breakage, resync, and
  degraded mode.

### P1 system benchmarks

- [ ] Run Agent 1's 10 M truth-layer benchmark.
- [ ] Run Agent 2's active physics benchmarks.
- [ ] Measure local-query p50, p95, and p99.
- [ ] Measure content hydration latency.
- [ ] Measure snapshot/replay and cold boot.
- [ ] Measure fold expansion/contraction.
- [ ] Measure multi-Actor active-region union.
- [ ] Measure protocol throughput and dropped-delta recovery.
- [ ] Measure Graph IR validation, compile, cache-hit, and execution latency.
- [ ] Measure trigger throughput, queue latency, storm containment, and repair
  backlog.
- [ ] Measure effect-intent-to-receipt latency without counting mocked text as a
  transport result.
- [ ] Run long-duration deterministic soak tests and report drift, leaks,
  backlog, wake churn, and replay equivalence.
- [ ] Publish hardware, parameters, raw results, and uncertainty.

### P1 failure stories

- [x] Corrupt/truncate a final event record and recover the valid prefix.
- [ ] Corrupt a middle event record and block honestly.
- [x] Exhaust query budget and preserve `budget_exhausted` as a bounded partial
  result.
- [x] Exhaust VM fuel and trap deterministically.
- [ ] Trigger a write conflict.
- [ ] Trigger a wake storm.
- [~] Drop protocol deltas and resync. The bounded in-memory stream distinguishes
  resumable retention from mandatory snapshot recovery; production transport
  loss remains open.
- [ ] Fail an external effect and preserve the failed receipt.
- [ ] Restart during an open fold.
- [ ] Crash before, during, and after each multi-command commit phase.
- [ ] Reject a stale or mismatched CodeDefinition, bytecode, mapping, snapshot,
  content hash, and Genesis signature.
- [ ] Inject NaN, infinite state, excessive velocity, energy explosion, link
  oscillation, non-convergence, causal-depth overflow, and trigger storm.
- [ ] Lose an optional renderer or adapter and remain honestly degraded.
- [ ] Deny unauthorized read, hydration, mutation, execution, and effect paths.
- [ ] Cancel and retry an effect without duplicate external action.
- [ ] Prove a ghost/counterfactual simulation cannot emit a real effect.

### P1 quality, delivery, and documentation

- [ ] Run format, lint, unit, property, fuzz, corruption, replay, E2E, and
  benchmark smoke gates in CI.
- [ ] Add concurrency/model tests for transaction ordering, idempotency, trigger
  ancestry, and overlay compaction.
- [ ] Add reproducible release builds and dependency/license/security audits.
- [ ] Version and publish compatibility matrices for snapshot, event log,
  protocol, Graph IR, bytecode, ontology, and physics mappings.
- [ ] Document the bootstrap boundary, data formats, lifecycle, failure modes,
  performance envelopes, and evidence interpretation.
- [ ] Produce a clean-room bootstrap procedure from a trusted Genesis Graph.
- [ ] Keep generated artifacts, caches, benchmark outputs, and renderer bundles
  explicitly non-authoritative and reproducible from committed authority.

### Subsystem done criteria

- [ ] The complete story works through the production protocol.
- [ ] Runtime evidence is independently readable.
- [ ] Performance claims include raw fresh measurements.
- [ ] The desktop reflects real events and never invents successful state.

---

## Coordinator checklist

- [x] Freeze Graph IR v0 before the vertical slice integration.
- [ ] Freeze protocol v0 before Mind Desktop integration.
- [ ] Review native code for hidden graph behavior.
- [ ] Review graph authority for stale, duplicate, conflicting, or orphaned
  CodeDefinitions, mappings, policies, triggers, and ChangeSets.
- [x] Keep one canonical E2E correlation ID per run.
- [x] Refuse completion without independent readback.
- [x] Reconcile every current blocked item with cause, attempts, dependency, and
  required outcome. Future blockers must follow the same rule.
- [ ] Require every completed item to name its command/run, fresh artifact, and
  independent readback where applicable.
- [ ] Remove temporary native behavior as soon as its Graph IR replacement is
  validated, wired, executed, and independently observed.

## Bootstrap completion gate

The repository reaches bootstrap v0 only when:

- [ ] a signed Genesis Graph boots a coherent Universe;
- [x] at least one graph CodeDefinition validates and compiles;
- [ ] a trigger launches the real Graph VM;
- [x] a bounded local query physically opens the relevant situation;
- [x] multi-command write sets commit atomically as one event/revision;
- [~] a safe echo adapter produces a real measured receipt, persists it, and
  reads it back; the production-protocol E2E remains open;
- [x] the first-slice result is independently read back;
- [x] crash/replay restores the same committed revision in the tested fixture;
- [x] an approved ontology/physics ChangeSet activates and its stored
  BehaviorBond is locally queried, compiled, executed, health-checked, committed,
  and independently read back;
- [ ] production protocol clients can reconnect and resynchronize without
  inventing state;
- [ ] 10 M entities and 10 M relations meet published memory and latency bounds;
- [ ] no business behavior is hidden in Rust, TypeScript, or Python;
- [x] the headless first-slice E2E runs without Python or Cypher;
- [ ] the complete trigger → VM → query/physics → transaction/effect → receipt
  → readback E2E runs through the production protocol.

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

- At the time of this recorded run, transactions were limited to one command.
  The later bootstrap work below closes that limitation with atomic
  batches.
- The first local query exhausts its declared relation budget; this is an
  evidenced bounded result, not a complete traversal.
- Full loop/call-depth/mutation/tick budget support, trigger wiring, protocol
  resynchronization, cryptographic Genesis signing and 10M/10M scale proof
  remain unimplemented.
- Mind Desktop has a Tauri/React Three Fiber shell, generic Atom/Bond rendering,
  luminous active Bonds, Observer controls, and an event reducer, but it is not
  connected to a real runtime stream and is not product-complete.
- `target/` and verification artifacts are generated local evidence; no branch,
  commit, stage, push or pull request was created.

## Ontology/physics projection evidence — 2026-07-30

Canonical structural run:

```text
store: artifacts/ontology-projection/run-20260730-132625927/store
ChangeSet: 00000000-0000-4000-8000-000000003000
projection contract: 00000000-0000-4000-8000-000000003001
structural receipt: 00000000-0000-4000-8000-0000000030f0
runtime gap: 00000000-0000-4000-8000-0000000030a0
graph task: 00000000-0000-4000-8000-0000000030a1
final revision/tick: 114
entities/relations: 267 / 862
overlay relations: 78
snapshot hash: 0810b844cdcb5fc8b203e068b24f043ada0a08a8b1e7682b0d61625ecbb1307f
```

Observed and independently read back:

- The ChangeSet, projection contract, semantic definitions, logic roles,
  profiles, gates, objectives, justifications, invariants, experiment, receipt,
  runtime gap, and activation task are durable graph records.
- Structural validation is measured only for the declared overlay and fixture.
  It is explicitly not a runtime execution receipt.
- No canonical predicate name is required in the intended native dispatch
  contract.
- The activation task requires a bounded local query, compilation receipt,
  execution receipt, independent readback, and negative tests for a missing
  role and unresolved ontology gap.

The three runtime blockers recorded by this structural run are now closed:

- compact symbols and referring records publish atomically;
- `OntologyRegistry` resolves approved ChangeSets as active overlays;
- the compiler and supervisor bridge produce and execute generic
  `RuntimeBondPlan` artifacts.

## Runtime behavior evidence — 2026-07-30

Implemented path:

```text
approved ChangeSet
→ deterministic active ontology overlay
→ transactional symbol publication
→ BehaviorBond validation
→ content-addressed RuntimeBondPlan
→ bounded local Atom cluster
→ physical execution receipt
→ tick-boundary receipt commit
→ independent store reopen/readback
→ artifact hash reverification
```

Measured results:

- Active ontology authority exposes base manifest, ordered ChangeSets, overlay
  members, symbol table, Universe revision, diagnostics, and an authority hash.
- A multi-command write set publishes as one event and revision; an invalid
  batch publishes no valid prefix.
- Missing logic role, unresolved ontology gap, unknown/not-measured/failed
  binding, invalid gate, bad hash, invalid budget, and energy mismatch all
  reject before physics.
- The native bridge dispatches only on the resolved generic primitive
  `support`, `inhibit`, or `neutral`, never on a canonical predicate name.
- Changing only the semantic predicate ID changes the artifact hash but leaves
  the measured physical receipt identical.
- The physical receipt records convergence or budget exhaustion, starvation,
  integer energy conservation, containment, lifetime, and release.
- The complete runtime receipt is stored as verified content, committed,
  reopened, deserialized, compared, and used to reverify the original artifact.
- A safe echo effect distinguishes pre-transport failure from an actual
  transport result, is idempotent, and is reinjected through a graph-owned write
  translation with independent content readback.

Fresh validation:

```text
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

All four commands passed. The latest workspace run executes 81 passing unit
tests plus all doctests.

## Active-store BehaviorBond E2E evidence — 2026-07-30

Canonical run:

```text
command: cargo run -p universe-e2e --bin behavior_runtime -- artifacts/behavior-runtime/coordinator-final
correlation: behavior-runtime-27248-1785414958584142500
manifest: artifacts/behavior-runtime/coordinator-final/behavior-runtime-27248-1785414958584142500/manifest.json
store: artifacts/behavior-runtime/coordinator-final/behavior-runtime-27248-1785414958584142500/store
authority revision: 1
final revision: 3
final snapshot hash: e307d2a164e159d8d553e5d8f01e3123dc9d90bb7e0cb6cc3c8b5c6804928058
```

Independently read results:

- One transaction installed 11 symbols, 31 entities, and 43 relations over the
  canonical ontology, producing 262 entities and 827 relations at revision 1.
- The registry honestly reports `ActiveWithBlockedBindings` because the global
  `BASED_ON` and `PROPOSES_CHANGE_TO` gaps remain; the selected `GROUNDS`
  BehaviorBond has no runtime-blocking diagnostic.
- The bounded local query returned `complete`, visited 12 entities, inspected 11
  relations, and included every one of the 10 authoritative binding relations.
- Source, target, predicate, profile, logic role, both gates, objective, and
  justification were materialized from relations and independently hashed
  contents. The bond content contains no duplicate binding authority.
- Materialization and compilation receipts are valid; the physical execution is
  quiescent, energy-conserving, contained, within lifetime, and released.
- Compilation, physical execution, and independent readback evidence are all
  measured and hash-consistent, so Loop health is `Measured(Closed)`.
- Compilation, execution, and health receipts were committed in two
  tick-boundary transactions, then read from a fresh replay. The reconstructed
  runtime artifact passed hash verification.

Remaining integration gaps:

- Direct immutable Store CSR, bounded mutable overlays, generation-checked
  tombstones, and deterministic in-memory compaction are implemented and
  consumed by the E2E. Atomic durable checkpoint/event-log rollover and scale
  benchmarks remain open.
- Loop health closure is now a stored graph `CodeDefinition` executed by the VM;
  the temporary native decision oracle has been removed.
- Triggers, production protocol/resynchronization, cryptographic Genesis
  signing, full cancellation, Desktop runtime streaming, and 10M/10M scale proof
  remain open.

## Direct Store CSR query evidence — 2026-07-30

Canonical run:

```text
command: cargo run -p universe-e2e --bin behavior_runtime -- artifacts/behavior-runtime/csr-direct
correlation: behavior-runtime-28248-1785415942034551000
manifest: artifacts/behavior-runtime/csr-direct/behavior-runtime-28248-1785415942034551000/manifest.json
authority revision: 1
final revision: 3
final snapshot hash: c10c5a449ac6bc74d75a3cff63055427cae36e46473f1a3217216779389d8505
```

Independently read results:

- `UniverseStore::load_current_indexed` replays the current authoritative
  revision and constructs a compact CSR-style incidence index.
- The index is bound to the Universe ID, revision, and canonical snapshot hash;
  an index from another truth revision is rejected.
- Adjacency lists contain relation positions into the immutable snapshot and
  are ordered deterministically by `RelationKey`; self-relations occur once.
- `universe-query` consumes the Store index through `LocalGraph` without a
  snapshot relation scan or ontology predicate dispatch.
- The real BehaviorBond read remained `complete`: 12 visited entities, 11
  inspected relations, empty frontier, and all 10 binding relations present.
- The downstream result remained `Measured(Closed)` and every receipt passed
  independent replay/readback.

## Bounded adjacency overlay evidence — 2026-07-30

Canonical run:

```text
command: cargo run -p universe-e2e --bin behavior_runtime -- artifacts/behavior-runtime/overlay-direct
correlation: behavior-runtime-18696-1785416628609767700
manifest: artifacts/behavior-runtime/overlay-direct/behavior-runtime-18696-1785416628609767700/manifest.json
base revision: 0
authority revision: 1
final revision: 3
final snapshot hash: 2791dbe0a8eaf4383742baf94a984f7e55f149b47b288c0ec86338a8b88803ac
```

Independently read results:

- The immutable base CSR is bound to snapshot hash
  `f4d535fe4cbc9da237dcd3462850f754ae72c8dc24e553da36510aece1604f1c`.
- One authoritative event was replayed into the bounded overlay: 31 added
  entities, 43 relation additions, 43 changed relations, 33 touched endpoints,
  and zero tombstones.
- The overlay produced current authority hash
  `85bb9406bad006c51e9c546373b25f19ed7d27cc0afd3702e379f555853ccaa0`.
- Compaction rebuilt an immutable CSR with that exact same snapshot hash.
- The real local query read the BehaviorBond through base CSR plus overlay,
  remained `complete`, visited 12 entities, inspected 11 relations, and
  returned an empty frontier.
- The downstream result remained `Measured(Closed)` through receipt commit and
  independent replay/readback.
- A separate deterministic Store test tombstones a base relation, proves the
  relation disappears from both endpoints, rejects a stale generation, adds a
  replacement relation, preserves the snapshot hash through compaction, and
  fails explicitly when the changed-relation budget is exceeded.

## Bootstrap closure evidence — 2026-07-30

Canonical graph-health run:

```text
command: cargo run -p universe-e2e --bin behavior_runtime -- artifacts/behavior-runtime/four-agent-graph-health
correlation: behavior-runtime-18096-1785418275074670800
manifest: artifacts/behavior-runtime/four-agent-graph-health/behavior-runtime-18096-1785418275074670800/manifest.json
authority revision: 1
final revision: 3
final snapshot hash: 5c4dbbbcee8e3177b80d187b18e83e4cf985888ea71782622b0d64d1b59d4625
```

Measured and independently read results:

- The graph-owned health `CodeDefinition` was stored as verified content,
  reopened from the authority Store, validated, compiled, and executed for
  exactly 39 instructions/fuel with no mutation proposal.
- Its 21 selected non-compensatory obligations returned
  `Epistemic::Measured(true)`. The resulting Loop health is
  `Measured(Closed)` with no blockers.
- The VM `ExecutionReceipt` and Loop health record committed atomically at
  revision 3 and were independently replayed, deserialized, and hash-verified.
- `rg` finds no remaining native `evaluate_behavior_loop_health` oracle.
- Relation physical commands are predicate-agnostic, bounded, idempotent,
  applied atomically at one exact tick, locally rolled back on failure, and
  independently observable by `RelationKey`.
- The supervisor bridge refuses relation deltas whose revision or source event
  does not match the real commit receipt.
- Durable Store rollover publishes a versioned hash-identified checkpoint,
  archives the valid log non-destructively, and reopens with an empty overlay.
  Tests cover stale input, tampered filenames, a reintroduced archived log, and
  crash windows before checkpoint, after checkpoint, and after log archive.
- Protocol v0 now has bounded sequenced in-memory streams, ack/resume,
  snapshot-required recovery, backpressure without silent drops, version
  negotiation, heartbeat, and distinct serialized operation states. Real
  authenticated IPC/network transport is still absent.

Fresh validation:

```text
cargo fmt --all -- --check
cargo test --workspace --exclude universe-postgres-import
cargo clippy --workspace --all-targets --exclude universe-postgres-import -- -D warnings
git diff --check
```

These gates pass with 102 unit tests plus doctests. The unexcluded
`cargo test --workspace` is honestly blocked by actively changing, out-of-scope
`universe-postgres-import` compilation drift. The latest independent check
reaches relation-seed builder borrow conflicts; its earlier schema-field errors
were already replaced by concurrent edits during this validation.

> Reconciliation 2026-07-30 (later): the `universe-postgres-import` drift above
> is resolved. `cargo build -p universe-postgres-import` finishes with 0 errors
> and the unexcluded `cargo test --workspace` now runs to completion with 131
> unit tests passing / 0 failing (postgres-import contributes 12);
> `cargo clippy -p universe-postgres-import --all-targets -- -D warnings` is
> clean. The `--exclude universe-postgres-import` workaround above is no longer
> required. The fix landed via concurrent edits to `cursor.rs`/`lib.rs`, not a
> new change in this reconciliation; verified by independent measurement, not
> assumed.

Remaining bootstrap blockers:

- Production authenticated IPC/network transport and a real reconnecting client.
- Graph triggers and the complete trigger-to-effect/readback causal chain.
- Cryptographic Genesis signing and trust-root verification.
- Generational dense/SoA truth storage, snapshot migrations, content GC, and
  multi-writer/process locking.
- Full Graph IR collections, general bounded loops, calls, error paths,
  capabilities, cancellation, and bytecode cache lifecycle.
- 10 M entity / 10 M relation and active-physics benchmarks with memory and
  p50/p95/p99 evidence.
- Mind Desktop runtime streaming and complete diegetic world rendering.
