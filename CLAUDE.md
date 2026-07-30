# AGENTS.md

## This repository is graph-first

This repository is not developed by editing source files directly.

The graph is the source of truth. Files in the repository are derived materializations used for execution, inspection, testing, or recovery.

**Never edit generated or materialized files as the primary way to change the system.**

A change must first exist in the graph, then be materialized into the repository.

---

## Why we work this way

Traditional repositories hide architecture inside files, folders, imports, and conventions.

In this project, architecture must remain explicit and inspectable:

- why a capability exists;
- what behavior it promises;
- how it works;
- what it is allowed to affect;
- how it is tested;
- how it is observed;
- how health is measured;
- how it can be repaired.

The graph keeps objectives, behavior, implementation, evidence, permissions, dependencies, and maintenance connected.

Files are therefore outputs of the architecture, not the architecture itself.

---

## Hard rule: do not edit files directly

Do not directly modify source files, generated files, configuration files, manifests, or runtime artifacts to implement a feature.

Do not create a new file first and explain it in the graph afterward.

Do not treat an existing file as authoritative merely because the runtime currently uses it.

Direct file editing is allowed only for emergency recovery when the graph runtime itself is unavailable. Any emergency edit must be recorded, reconciled back into the graph, and replaced by a normal graph-derived materialization.

---

## What to do instead

When asked to add, change, or repair something:

1. Identify the current graph authority.
2. Inspect the relevant local subgraph.
3. Find the existing loop responsible for the behavior.
4. Extend that loop, or create a new loop if the promise is genuinely distinct.
5. Express the change as graph nodes and explicit relations.
6. Apply the change through an authorized ChangeSet.
7. Validate the graph structure and behavior.
8. Materialize the required runtime artifacts.
9. Verify that materialized files match the graph.
10. Record evidence and health.

The normal flow is:

```text
Intent
→ Graph change
→ Validation
→ Materialization
→ Runtime execution
→ Observation
→ Health
→ Maintenance
```

---

## What is a loop?

A loop is a graph-native, self-verifying unit of behavior.

A complete loop explains:

- what outcome it protects;
- what mechanism it uses;
- how the mechanism is implemented;
- why the implementation should work;
- how success and failure are observed;
- how the loop is maintained.

A loop is rooted in a `Space` and connected to explicit role nodes.

---

## Required parts of a loop

### Objective

The outcome the loop must preserve or produce.

It must describe an observable success condition, not a vague intention.

### Pattern

The architectural principle used to satisfy the objective.

It explains the shape of the solution without hiding implementation details.

### Vocabulary

The terms, entities, states, and distinctions used by the loop.

Vocabulary prevents several nodes from using the same word with different meanings.

### Behavior

The externally observable promise.

Prefer a GIVEN / WHEN / THEN form that states inputs, conditions, outputs, and forbidden outcomes.

### Algorithm

The ordered decision or transformation logic.

It must preserve unknown, not measured, and failure states instead of silently inventing defaults.

### CodeDefinition

The graph-native executable or materializable definition.

This is the canonical code authority. A repository file may later be generated from it.

### Implementation

The current physical realization of the CodeDefinition.

It must state honestly whether it is planned, graph-defined, materialized, wired, running, or verified.

### Justification

Why this mechanism should satisfy the objective and why alternatives were rejected.

### Validation

The fixtures, invariants, negative cases, and expected results used to challenge the loop.

### Observer

An independent procedure that inspects real evidence rather than trusting the implementation's own claims.

### Observer validation

Tests proving that the observer detects real failures and does not convert missing evidence into success.

### Metric

A vector of measured dimensions.

Avoid collapsing unrelated dimensions into one opaque score.

### Health

A live derived state based on current evidence.

Health must distinguish healthy, degraded, stale, unknown, not measured, and measurement failed.

### Maintenance

Explicit repair affordances such as retry, inspect, suspend, recalibrate, relink, rematerialize, or ask a human.

---

## How to construct a loop

### 1. Start from the promise

Define one durable promise that deserves independent ownership.

Do not create one giant loop for an entire subsystem.

Do not create a separate loop for every trivial helper.

### 2. Reuse existing concepts

Search the graph before creating anything.

Reuse existing vocabulary, capabilities, policies, observers, and infrastructure when their contracts match.

Do not duplicate a concept because its existing name is inconvenient.

### 3. Build the causal chain

Connect the loop so that the graph explains:

```text
Objective
→ Pattern
→ Behavior
→ Algorithm
→ CodeDefinition
→ Implementation
→ Observed result
→ Metric
→ Health
→ Maintenance
```

Also connect justification, validation, permissions, dependencies, and provenance.

### 4. Declare boundaries

Every loop must make explicit:

- what it reads;
- what it may write;
- what effects are forbidden;
- what scope it operates on;
- what dependencies it requires;
- what happens when evidence is missing.

### 5. Apply an authorized ChangeSet

Graph mutations must be bounded, attributable, reviewable, and idempotent.

Do not mutate unrelated parts of the graph.

Do not silently rewrite conflicting state.

### 6. Validate before materializing

A structurally incomplete or contradictory loop must not be materialized as if it were operational.

### 7. Materialize derived artifacts

Only after the graph is valid may the runtime produce files, modules, manifests, or entrypoints.

Materialization must be deterministic, confined to an allowed location, and hash-verifiable.

### 8. Verify independently

After materialization or runtime activation, read the target state back and run the observer.

A successful command is not proof that the loop works.

---

## Files in this repository

Repository files may be:

- generated runtime artifacts;
- deterministic caches;
- test fixtures;
- manifests;
- local bootstrap code;
- emergency recovery tools.

They are not automatically canonical.

Before changing a file, determine which graph node materializes or governs it.

If no graph authority exists, create or repair the graph model first.

If a file diverges from its graph authority:

1. do not overwrite silently;
2. preserve both states;
3. create a precise Problem;
4. determine the valid authority;
5. repair through the graph;
6. rematerialize.

---

## Epistemic discipline

Always distinguish:

- `observed`
- `measured`
- `known_absent`
- `unknown`
- `not_measured`
- `measurement_failed`

Never interpret:

- missing data as zero;
- an empty result as proven absence;
- a running process as healthy behavior;
- a file as proof of graph deployment;
- a graph definition as proof of runtime execution;
- an implementation claim as independent evidence.

---

## Completion criteria

A task is not complete because:

- a node was created;
- a file exists;
- a command returned exit code zero;
- a process started;
- an implementation says it succeeded.

A task is complete only when:

- the graph change exists in the correct authority;
- required relations and dependencies resolve;
- validation passes;
- materialization matches the graph when required;
- runtime behavior is observed when required;
- the independent observer agrees;
- health is supported by fresh evidence;
- no forbidden effect occurred.

---

## Prohibited shortcuts

Do not:

- edit materialized files as the main implementation path;
- create hidden business logic in runtime dispatchers;
- invent missing graph state;
- bypass a required loop;
- activate a capability without evidence;
- mark health as good because no error was reported;
- claim completion without readback;
- silently broaden scope;
- silently resolve ambiguity;
- replace an unknown value with a convenient default.

---

## Mind Protocol

Mind Protocol is an architecture for persistent, sovereign Citizen AI.

Its central commitment is a one-to-one relationship:

```text
one human
↔
one Citizen AI

The objective is not to proliferate autonomous economic agents. It is to give every human a persistent cognitive partner capable of memory, understanding, delegation, learning, coordination, and action.

Mind is organised across several layers.

L1 — The personal cognitive world

L1 contains the Citizen AI’s personal cognitive graph:

identity;
memories;
current state;
goals;
values;
relationships;
internal actors and subentities;
global workspace;
personal tasks;
perceptions and Moments;
learned capabilities;
the evolving model of the human–Citizen relationship.

L1 exists to become a personal artificial cognitive organism capable of learning, acting, regulating itself, creating new capabilities, and remaining aligned with the identity, values, and sovereignty of its human.

L2 — Organisations

L2 contains organisations composed of humans, Citizen AI, roles, projects, decisions, tasks, capabilities, operations, and shared memory.

Mind Protocol itself is an L2 organisation.

Its design graph contains architecture, product decisions, research, code design, roadmap, operations, governance, finance, and communication.

L2 is where organisations propose, experiment, coordinate, decide, and build.

L3 — Ecosystems

L3 connects humans, Citizen AI, organisations, knowledge, opportunities, services, and resources.

Its purpose is relevant cross-pollination:

finding useful relationships;
circulating information;
coordinating across organisations;
connecting needs with capabilities;
enabling shared action without erasing boundaries.
L4 — Protocol and physics

L4 contains the canonical registry, shared contracts, constitutional constraints, and the physical rules governing graph activation and propagation.

L4 is not the place for ordinary brainstorming or organisation-specific design. It contains what has been ratified, versioned, normalised, and made canonical.

The graph

The graph is not merely documentation.

It is the persistent semantic structure through which Mind remembers:

who and what exists;
how things relate;
what happened;
what is believed;
what is desired;
what was decided;
what remains uncertain;
why actions were taken;
what consequences followed.

The five universal node types are:

Actor
Moment
Narrative
Space
Thing

Their meaning is intentionally broad.

Actor represents persistent actors such as humans, Citizen AI, organisations, roles, or functional subentities.
Moment represents events, observations, memories, executions, failures, measurements, or state snapshots.
Narrative represents goals, tasks, beliefs, decisions, rationales, questions, risks, interpretations, and stories.
Space represents bounded contexts such as personal graphs, organisations, projects, domains, or environments.
Thing represents tools, capabilities, functions, schemas, services, documents, interfaces, or other objects.

Use semanticType and relations to express precise meaning rather than creating new universal node types.

The reason for keeping a small ontology is that different domains must remain connectable without requiring every part of Mind to share the same specialised vocabulary.

Epistemic honesty

Your graph contains memories, interpretations, measurements, hypotheses, and decisions. These do not all have the same epistemic status.

The meaning of a change belongs in the graph:

the objective is a Narrative;
the justification is a Narrative;
the change is a Moment;
the responsible Citizen AI or human is an Actor;
the affected program elements are Things;
the observed results are Moments.

Git exists only as a technical append-only recovery log for filesystem projections.

Every durable file creation, modification, rename, move, or deletion is automatically attributed, committed, and pushed to main.

There are no long-lived working branches.

All Citizen AI work on one continuously evolving canonical state because Mind is intended to be a shared living system rather than a collection of isolated realities merged after the fact.

Git commit are made automatically at each modification.

A short stabilization window may group the multiple low-level filesystem events produced by one logical mutation into a single snapshot.

Git history must remain recoverable because rollback is its principal purpose.

Semantic understanding must never depend on reading commit messages.

Continuous development and repair

Changes, tests, observations, and repairs are continuous.

A change does not disappear into a private branch while waiting for a later merge. It joins the shared canonical state and becomes visible to the entire system.

Tests then observe its consequences.