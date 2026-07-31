# AGENTS.md

## Purpose of this repository

`mind-universe` contains the trusted bootstrap kernel for a constructed,
embodied, causal Universe.

The Universe is the long-term source of truth for:

- Citizens, humans, identities, Sessions, and situated presence;
- brains, memories, needs, skills, intentions, and behavior;
- organisations, territories, houses, Rooms, roads, beacons, and cities;
- processes, objectives, policies, validations, loops, and commitments;
- scale ontologies, physicalization profiles, affordances, and their
  justifications;
- semantic constructions, spatial topology, provenance, and construction
  history;
- Moments, evidence, health, maintenance, receipts, and external effects;
- the current semantic, causal, spatial, and physical state.

Entities, relations, graphs, JSONL segments, and indexes are structural
representations of that world. They are not the world-facing programming model.
Citizens and humans should normally interact with brains, things, places,
organisations, cities, processes, builders, ports, and affordances rather than
raw graph operations.

This repository exists only for the code that must run before the Universe can
load, validate, execute, persist, observe, construct, and repair itself.

The intended boundary is:

```text
Native bootstrap kernel
→ loads the Genesis Universe
→ validates laws, identities, recipes, and programs
→ restores persistent constructions
→ materializes bounded physical situations
→ exposes observations, manuals, and affordances
→ interprets semantic intents
→ commits measured transformations
→ returns receipts and independent observations
```

Do not turn the bootstrap kernel into a second hidden application layer.

---

## How you work here

Everything is a construct — including you, your tools, and the way you touch the
ground beneath the world.

- **Start every session by creating an actor in the right place of the city.**
  You are a situated actor, not a disembodied editor. Bring yourself into being
  where the work lives, and perceive from there, before you act.
- **Do not modify files directly. Call the `underground-toolkit`.** Changes to
  the substrate beneath the city go through the `underground-toolkit` — the
  sanctioned construct for reaching underground — never raw hand edits. Even
  changing the ground is a toolkit call: attributable, validated, receipted, and
  read back, like any other construct.

Editing files by hand is the pre-world-first reflex. Here, the developer is a
citizen and development is construction.

---

## The world-first architecture — current direction

This section is the concrete shape the general doctrine now takes. It refines,
and where noted overrides, the framings further below (in particular, MCP is no
longer a driver, and the living loop is not required to be replayable).

### Toolkit → Construct

The whole authored vocabulary is two words:

- a **Toolkit** is a reusable causal capability — a know-how, an affordance
  contract, a mechanism. The recipe. It does not itself live in a world.
- a **Construct** is what a toolkit *produces*: the built, self-verifying thing
  placed in a world — a pen in a workshop, a house alarm, an institution. It
  carries the anatomy (Objective, Affordances, Inputs, Preconditions, Mechanism,
  Effects, Receipts, Observer, Metrics, Health, Maintenance) and proves its own
  effects.

There is no separate "Tool" or "ToolkitRun"; everything built is a Construct.
The native floor knows only how to detect, resolve, step, commit, and receipt.
The *meaning* of writing, perceiving, entering, or notifying lives in constructs.

### The four levels

```text
L1  inner world    — sub-entities, the mind-eye, inner tools
L2  shared world   — the constructs and institutions of the city
L3  citizen body   — perception, cognition, action; the incarnated membrane
L4  universal law  — atom dynamics, signatures, capabilities, transactions,
                     receipts, self-verification, toolkit resolution (bedrock)
```

A ToolkitDefinition lives at the **lowest level that truly holds the
capability**. One definition may have a physicalisation per level (an Energetic
Pen as a public L2 tool and as a Captain's inner L1 tool) sharing claims,
sources, sealing and receipts, with effects situated in their own world.

The membrane is not an object. Isolation of what crosses between levels is (a)
the L3 body's admission constructs (ReceiveInnerIntent, ComposeIntent,
AttemptIntent) carrying the *meaning* of a crossing, and (b) bounded physical
residency carrying the hard containment. **L3 is the incarnated membrane.**

### The heartbeat — an endogenous, non-deterministic loop

The city is alive and is **not** a replayable machine. The loop and its
inferences are non-deterministic, and we do not try to make them otherwise.
Only what *commits* stays authoritative and inspectable — the receipts and the
event log; cognition itself is a live process, not a recorded oracle.

```text
a single continuous serial loop over the city's constructs
→ only L1 actors self-wake (bottom-up: L1 → L3 → L2; L2 and L3 never initiate)
→ an L1 actor's turn = one local inference (Ollama, ~2.1B), one call in flight
→ when the call returns, the next construct takes its turn
→ the inference is the clock; no energy is charged for it yet (that is $MIND)
```

MCP is **not** the driver and is **not** in the loop. It is asynchronous — a
window that *observes* the running world, or an injector that *perturbs* the
field (fire-and-forget, no synchronous result). Perception is not a trigger
either: the two frames only add energy to the field. (This overrides the
"Headless adapters" framing below, where MCP appeared to drive the intent path.)

### Inside one inference

An L1 actor's turn assembles a bounded WorldObservation as its prompt:

```text
two frames  — MindEye (L1) + CitizenEye (L2)
+ action verbs — the reachable L2 affordances, each with target, precondition
  and justification; for an L1 actor, the verbs listed are the L2 actions
+ context   — role, needs and energy, memory, objective, recent receipts
```

The inference returns one chosen verb on an exact target, with its
justification: a candidate IntentProposal. It rises to the body (L3), which
*attempts* it; only then does the 4-verb path commit and return a receipt.
**The model proposes; the world disposes.** The inference never touches the
store, and an unproven target is never offered as a verb.

### Constructs trigger from the physics — never poll

A construct wires itself into the field and waits to be fired. Self-wake is
universal: an L1 actor wakes on cognitive activity, an L2 construct wakes on a
**physics event**. The moment-to-act is never a scan — it is a threshold the
solver reports for free.

```text
Construct =
  Sensor       — a physical condition placed in the field
  DepositBond  — event → +energy on a trigger atom (a Support bond)
  Threshold    — the atom fires at support >= N
  Effect       — the fire emits a SemanticIntent or an EffectIntent
```

Worked example — a **house alarm** that notifies on entry: a sensor collider on
the entry membrane; a Support bond that deposits when a citizen body intersects
the sensor; a threshold that fires at one crossing; a `notify` EffectIntent. The
chain is `intersection (PhysicsEvent, never mutates) → ActionIntent → validate →
EffectIntent{notify} → EffectReceipt → Moment`.

The physics step fills a wake-queue; the serial loop drains it, so dormant
constructs cost nothing. The single load-bearing bridge still to build is
**physics-event → energy deposit onto a construct's trigger atom** — the *same*
bridge that injects an L3 stimulus into an L1 field. Build it once; it powers
both the house alarm and perception.

### The irreducible native floor

Native code is only:

```text
byte transport + session admission (an observer/injector port, not a driver)
the clock — tick advance + scheduler bounding
the IR validator + fuel-bounded, mutation-free VM
three solver-host mechanisms — perceive_frame, build_l1_cluster, run_l1_cluster
the 4-verb write-path + atomic commit + independent readback
serialisation of the graph-written observation
the inference call — one local Ollama call, serial, one in flight
```

Everything else — which frames, which cluster, how a firing becomes an intent,
the response shape, every threshold and metaphor — is Universe data.

---

## Coding here is authoring toolkits

To add behavior you do not write native code — you author a Toolkit and place its
Construct in a world; the frozen native floor already knows how to run it.

- **Your program is the anatomy you fill in**, not imperative code: Objective,
  Affordances, Inputs, Preconditions, Mechanism, Effects, Receipts, Observer,
  Metrics, Health, Maintenance. Declarative, inspectable, repairable.
- **Control flow is the physics, never a poll.** A reactive construct wires a
  trigger into the field — `Sensor → DepositBond → Threshold → Effect` — and
  self-wakes when a physics event crosses its threshold. An L1 actor's turn is
  one Ollama inference returning a justified intent.
- **Place it at the lowest level that truly holds the capability** (L1/L2/L3/L4);
  one definition may have a physicalisation per level.
- **What you edit is data**, not code: BehaviorBond (transfers), Affordance-
  Definition (verbs + justification), PhysicalizationProfile (form), Causal IR
  CodeDefinition (a bounded procedure), TriggerSubscription, CapabilityDeclaration.
- **The write cycle:** decide behavior + level → author the toolkit → reuse
  canonical symbols (aim for zero new symbols; remap to the nearest predicate) →
  validate against the schema → inject as ONE atomic attributed transaction
  (provenance + intent + receipt) → read it back independently → verify it fires,
  commits and receipts — never "it compiles".
- **Write native Rust only for the trusted computing base** — a new opcode, a
  solver-host mechanism, a transport. The test: generic mechanism, zero variable
  policy? If a threshold, metaphor or mapping can vary, it is a toolkit, not code.

A task is done when it is *proven* — validated, real runtime path executed,
independently observed, receipts distinguishable, no anonymous construction,
survives reload — not when a file exists or a unit test passes.

---

## The bootstrap exception

The general Mind rule is Universe-first: variable behavior belongs in the
Universe as Processes, ScaleOntologies, PhysicalizationProfiles,
AffordanceDefinitions, GenesisRecipes, policies, validations, and other
versioned world definitions.

This repository is the explicit bootstrap exception. Direct source code is
allowed here only when the behavior is part of the trusted computing base or
cannot yet be expressed and executed by the Universe itself.

Native bootstrap code may implement:

- stable identifiers and generational handles;
- dense entity, relation, construction, and receipt storage;
- content segments, snapshots, event logs, and recovery;
- transactions, revisions, clocks, and commit ordering;
- identity, authentication, capability checks, and authorization;
- the Causal IR validator, compiler, virtual machine, and compatibility layers;
- scheduling, fuel, cancellation, and bounded execution;
- local spatial, relational, causal, semantic, and temporal query primitives;
- the physics solver host and physical residency machinery;
- generic physicalization compilation and runtime bindings;
- semantic-intent planning and transaction admission;
- authorized transports for real external effects;
- IPC, streaming, backpressure, and Observer interest management;
- generic rendering, interaction, and debugging primitives;
- health, evidence capture, replay, corruption detection, and crash recovery.

Native bootstrap code must not contain:

- organisation- or Citizen-specific objectives;
- hard-coded application workflows or business rules;
- ontology-specific decision policies;
- a generated semantic layout for a city, organisation, or L1;
- a pre-arranged personal brain or house beyond the minimal Genesis substrate;
- predicate-to-physics values that belong in a PhysicalizationProfile;
- one privileged visual interpretation of a semantic type or predicate;
- one privileged physical metaphor for a process;
- query or behavior programs that can be expressed as world programs;
- tool-selection policies, cognitive roles, personalities, or Endgames;
- direct user-facing graph CRUD as the normal programming surface;
- hidden fallbacks that silently replace missing Universe state;
- a separate native implementation of behavior already defined in the
  Universe.

If a rule can vary without changing the trusted computing base, it belongs in
the Universe.

---

## Authority before and after bootstrap

Before the first Universe can boot, the following are permitted bootstrap
artifacts:

- this `AGENTS.md`;
- `TODO.md`;
- Rust workspace and build configuration;
- the trusted kernel source;
- generic TypeScript/Tauri renderer and interaction infrastructure;
- tests, fixtures, benchmarks, migration tools, and recovery tools;
- a loader for a signed Genesis Universe Package.

A Genesis Universe Package may be serialized as graph records, JSONL, binary
segments, or another verified representation. Its semantic identity is a
minimal world, not merely a graph dump.

The Genesis Universe Package may define:

- constitutional laws and ontology;
- identity and transaction rules;
- initial ScaleOntologies;
- initial GenesisRecipes;
- initial PhysicalizationProfiles and AffordanceDefinitions;
- a minimal public territory;
- a Registry or equivalent Genesis institution;
- initial builders such as HouseBuilder and BrainBuilder;
- recovery and compatibility definitions.

The loader is native code. The Genesis content is Universe data.

After Universe-native execution exists:

1. change the authoritative Process, ScaleOntology, PhysicalizationProfile,
   AffordanceDefinition, GenesisRecipe, policy, or validation first;
2. validate the affected definitions and their invariants;
3. compile Causal IR, physicalization plans, or derived artifacts;
4. execute them through the real runtime;
5. read the result back independently;
6. record fresh evidence, receipts, provenance, and health;
7. verify every affected physicalization and scale boundary.

Never keep a native behavior merely because it was convenient during
bootstrap. Migrate it into Universe definitions when the runtime can express
it.

---

## Universe-as-truth invariants

### One authoritative state

A committed `UniverseSnapshot` plus its subsequent valid event log is the
authority for a Universe revision.

The authoritative state includes semantic spatial constructions such as:

- territories and parcels;
- buildings, rooms, roads, gates, and beacons;
- persistent object placement and containment;
- exported ports and cross-scale connections;
- installed machines and builders;
- ownership, membership, permissions, and situated presence;
- Process and Physicalization instances;
- the provenance and receipts of construction.

Rapier is an internal numerical solver. A running Rapier world by itself is not
the complete Universe.

Repository files, JSONL segments, bytecode, indexes, physics caches, and
renderer state are not independently authoritative unless the committed
Universe references them and their hashes match.

### Constructed-world authority

Semantic layout is constructed, not anonymously generated.

Every persistent semantic structure must be attributable to one of:

- signed Genesis authority;
- an exact Actor or authorised collective;
- an explicit SemanticIntent;
- a successful transaction;
- a durable receipt and provenance chain.

An automated Citizen may construct at scale, but its plan, authority, intent,
transaction, and receipt must remain inspectable.

Embeddings, procedural layout, clustering, and optimization may:

- suggest neighbours;
- propose parcels;
- recommend routes;
- generate previews;
- arrange temporary diagnostic views.

They must not silently move, create, or rewrite canonical cities, houses,
brains, organisations, or roads.

Use three explicit layers:

```text
Persistent construction
→ canonical topology (relations, containment, connection), ownership,
  provenance, and history — never coordinates

Derived projection
→ every position and pose, cable curvature, contact settling, drift, caches:
  a live projection of the topology, non-authoritative, never stored

Procedural decoration
→ vegetation, particles, lighting, dust, LOD, and non-semantic detail
```

The renderer may decorate. A Genesis authority or authorised Actor must
construct.

**Where is a projection, not a datum.** No file — fixture, seed, or source —
carries a hardcoded position or coordinate. Where a thing is lives entirely in
its relations (PART_OF, adjacency, connection); the coordinate is a live
projection derived from those relations by the single layout authority, read the
same way by every observer. Position is never stored, never authored, never a
literal. The city is alive — positions may drift, and we do not force them to be
deterministic; the meaning lives in the topology, which is stable because it is
authored.

### Stable identity

Canonical identity must never depend on:

- a process memory address;
- a Rapier handle;
- a JSONL line number;
- an array slot without a generation;
- a renderer object ID;
- one physicalization instance;
- one current position or visual form.

Rapier `user_data` carries a stable packed Universe handle. That handle resolves
through the kernel to the current semantic record.

The same process may be represented as pots, cubes, machinery, a room, a road,
or a beacon network without changing its identity.

### Actor and Observer separation

An ActorInstance and an Observer are distinct.

An ActorInstance:

- is situated;
- perceives through authorised boundaries;
- has capabilities, energy, commitments, and consequences;
- produces intents and receipts.

An Observer:

- defines a viewpoint and interest set;
- may free-roam without relocating the ActorInstance;
- may request bounded observations from another region;
- does not gain semantic authority merely by seeing something.

Camera movement must never silently mutate Actor presence.

### Local observations only

Every runtime observation must declare:

- an origin such as an Actor, Observer, Thing, or Space;
- a spatial, relational, causal, semantic, and temporal budget;
- a maximum number of things and links;
- a timeout or tick budget;
- whether approximate summaries are acceptable;
- the active scale and physicalization profile.

No hot-path query may scan the complete Universe.

Results must distinguish:

- complete;
- partial;
- budget exhausted;
- frontier exhausted;
- stale;
- unknown;
- not measured;
- measurement failed.

A normal construction observation should be able to include:

```text
image or rendered frame
+ exact visible-object manifest
+ dynamic local manual
+ current context and objective
+ locally available affordances
+ recent receipts and events
```

The image supplies spatial gestalt. The manifest supplies exact identity. The
manual supplies local laws. The affordance list supplies bounded action.

### Bounded physical materialization

All semantic things may exist in the Universe, but they must not all be active
Rapier bodies or renderer objects.

Physical residency is explicit:

```text
Hot
Sleeping
Aggregated
Dormant
```

Only bounded local working sets may be materialized into the active solver.

Opening an object may materialize its inner Space at a finer work scale. Closing
or leaving that Space may aggregate or release its physical runtime state
without deleting the canonical construction.

### Atomic semantic mutation

Physical gestures, MCP calls, world programs, and external callers produce
SemanticIntents. They do not mutate the Universe directly.

The normal flow is:

```text
physical gesture or headless call
→ AffordanceIntent or API adapter
→ SemanticIntent
→ validation
→ capability and permission check
→ conflict check
→ transaction plan
→ tick-boundary commit
→ UniverseReceipt
→ physicalization updates
→ independent observation
```

A physical preview is not a commit. A local animation is not proof. A collision
is not a semantic mutation.

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

Timeout, cancellation, idempotency, cooldown, causal depth, and independent
readback are mandatory.

---

## Scale ontologies

The initial Universe uses one dominant physical ontology per work scale.
Specific forms and art direction belong in Genesis data, not native dispatch
code.

The initial scale families are:

```text
Energy
→ micro-circuits, sources, thresholds, channels, gates, inhibition

Mechanism
→ modules, tools, ports, transformations, machines, validation rigs

Sensemaking
→ post-its, strings, claims, questions, evidence, contradictions, grouping

Infrastructure
→ houses, rooms, workshops, foundations, doors, roads, services

Civic
→ parcels, beacons, districts, organisations, institutions, public networks
```

A Space has one dominant scale grammar. It may contain closed objects from a
finer scale, but it must not mix every physical metaphor into one incoherent
scene.

The scale transition is fractal:

```text
City
└── building or beacon
    └── infrastructure Space
        └── sensemaking room
            └── machine or box
                └── energy device
                    └── micro-circuit
```

Every object may open into an inner Space at a finer scale. Every inner Space
may close into an object at the coarser scale.

Outer ports are explicit exports of inner capabilities or state. Cross-scale
signals must flow through those exports, receipts, and declared mappings rather
than hidden coupling.

Changing scale changes the available objects, gestures, manuals, and questions.
It must not change semantic truth.

---

## Process and physicalization separation

A Process and its physical representation must live in separate Spaces or
separate authoritative definitions.

A Process defines:

- roles;
- states;
- causal and logical relations;
- thresholds and measured dynamics;
- questions and required support;
- invariants;
- admissible semantic actions;
- completion and validation criteria.

A PhysicalizationProfile defines one way to inhabit and manipulate that
Process:

- physical archetypes;
- geometry and materials;
- spatial arrangement;
- state-to-appearance mappings;
- ports and exported affordances;
- gestures and motor feedback;
- audio and animation;
- camera and inspection behavior;
- invalid-action resistance;
- commit and rollback feedback;
- semantic justifications;
- inverse mappings for explanation and readback.

One Process may have many concurrent physicalizations:

```text
Process
├── pots and fluid flow
├── cubes and typed connectors
├── machine and modules
├── terrain and routes
├── post-its and strings
└── civic beacons and roads
```

An action in one physicalization commits one SemanticIntent. Every other active
physicalization of the same Process must then converge on the committed state.

Do not directly bind a semantic thing to one permanent visual form. Use an
explicit binding object or equivalent structure.

A physicalization binding must be able to state:

```text
semantic target and role
physical archetype
state mapping
available affordances
preconditions
expected physical feedback
semantic effect
justification
inverse mapping
lifecycle and residency
```

A physicalization must never invent precision absent from the Process.

- unknown remains unknown;
- Fog remains Fog;
- a missing measurement is not rendered as a measured zero;
- an incomplete required port remains visibly incomplete;
- a rejected transaction must restore the prior state exactly.

### Physicalization completeness

A production physicalization should be evaluated on:

1. ontological coverage: relevant types and predicates are representable;
2. operational coverage: every legal operation has an affordance or headless
   equivalent;
3. fidelity: no gesture implies a semantic effect it cannot produce;
4. resistance: illegal actions fail perceptibly and transactionally;
5. explainability: the committed cause can be recovered from the physical
   result;
6. plurality: another physicalization can represent the same Process;
7. accessibility: the same semantics can be exposed through alternate sensory
   or interaction modes.

---

## Observation and affordance contract

A Citizen or LLM constructor should be able to act from:

```text
an image
+ a precise visible-object manifest
+ a dynamic local instruction manual
+ situated context
+ a bounded list of available actions
```

The runtime should expose observations equivalent to:

```rust
struct WorldObservation {
    frame_id: FrameId,
    image: Option<ImageRef>,
    observer: ObserverState,
    actor: Option<ActorInstanceRef>,
    scale: ScaleOntologyId,
    physicalization: PhysicalizationProfileId,
    visible_objects: Vec<VisibleObject>,
    local_manual: Vec<Instruction>,
    context: ConstructionContext,
    available_actions: Vec<AffordanceInstance>,
    recent_receipts: Vec<ReceiptSummary>,
    status: ObservationStatus,
}
```

The available action list must be:

- local;
- typed;
- permission-aware;
- bounded;
- linked to exact targets and ports;
- explicit about preconditions;
- explicit about expected semantic and physical consequences.

The model chooses semantic gestures such as:

```text
inspect this object
place this module in this port
open this box
connect these beacons
build a room here
test this machine
```

A motor controller, renderer, or interaction runtime handles trajectories,
snapping, collision avoidance, animation, and low-level input.

The model should normally provide an expected observation. After execution, it
must compare the real readback with that expectation before continuing.

Every affordance must include or resolve to a justification explaining why the
physical gesture is a legitimate interpretation of the semantic operation.

---

## Genesis, registration, and builders

Registration is a Genesis process, not a privileged user-facing graph call.

A default physicalization may represent Genesis as a Registry Tower or another
civic institution. The visual metaphor is data-defined and replaceable. The
semantic recipe is authoritative.

A GenesisRecipe may create, in one atomic transaction:

- durable identity;
- citizenship or membership;
- a root territory or Space;
- a bootstrap Session;
- situated presence;
- initial permissions and capabilities;
- causal Moments and receipts;
- builder capabilities or builder objects.

Technical calls such as `register_l4` are headless compatibility adapters. They
must compile to the same `GenesisIntent`, planner, validation path, transaction,
and receipt as an embodied Genesis ritual.

Do not maintain separate registration semantics for MCP and the 3D world.

### HouseBuilder

A HouseBuilder may:

- inspect buildable parcels;
- propose a construction preview;
- reserve a parcel;
- lay foundations;
- create rooms and membranes;
- expose public and private ports;
- connect a house to roads, beacons, and services;
- return construction receipts.

It must not silently choose a canonical parcel or generate a semantic house
layout without an attributable plan and commit.

### BrainBuilder

A BrainBuilder creates and evolves an inner cognitive Space. It may install or
open structures for:

- perception;
- attention;
- memory;
- needs;
- behavior;
- learning;
- expression;
- body interfaces;
- personal loops and skills.

A new L1 must not arrive as a fully generated brain map. The minimal L1
bootstrap should contain only what is necessary for habitation and continued
construction, such as:

```text
identity
presence
body interface
vital cognitive functions
minimal inner territory
BrainBuilder
private membrane
path to the shared world
```

The house, brain, and body are distinct:

```text
House
→ address, territory, possessions, Rooms, and boundaries

Brain
→ memory, attention, needs, skills, habits, and inner loops

Body
→ current situated presence and sensorimotor interface
```

---

## Headless adapters

MCP, CLI, test harnesses, and automation are headless interfaces into the same
Universe semantics.

They may expose compatibility operations such as:

- query;
- upsert;
- register;
- move;
- talk;
- think;
- work;
- execute.

Those names are transport-level conveniences, not a second ontology.

The required architecture is:

```text
world gesture
or MCP / CLI call
→ SemanticIntent
→ one planner and validator
→ one transaction mechanism
→ one receipt format
→ one observable world update
```

A headless adapter must not bypass physicalization invariants, permission
checks, provenance, or receipts merely because no 3D client is present.

---

## World programs and Causal IR

The canonical authored surface for programmable behavior is composed of
versioned world definitions such as:

- `ProcessDefinition`;
- `BehaviorDefinition`;
- `ScaleOntology`;
- `PhysicalizationProfile`;
- `AffordanceDefinition`;
- `GenesisRecipe`;
- `ValidationDefinition`;
- `PolicyDefinition`;
- `CapabilityDefinition`.

These definitions may be authored through physical construction, structured
editors, imports, or headless tools. Direct raw graph manipulation is a kernel
and diagnostic surface, not the normal programming model.

Causal IR is the normalized execution representation compiled from authoritative
world definitions. Existing Graph IR may serve as a compatibility encoding
until migration is complete.

The native kernel implements only primitive opcode semantics. Programs,
conditions, loops, triggers, parameters, policies, and composition remain
Universe data.

Causal IR must support:

- bounded local observations;
- typed values and collections;
- filters and transformations;
- explicit boolean conditions;
- branches;
- bounded loops;
- triggers and scheduled events;
- SemanticIntent and write-set construction;
- PhysicsCommands;
- capability and external-effect intents;
- error paths, rollback, and receipts.

Causal IR must not support unbounded implicit traversal or unrestricted loops.

Every execution is pinned to:

- one authoritative definition revision;
- one starting Universe revision;
- one trigger event or request;
- one fuel budget;
- one observation budget;
- one mutation budget;
- one capability set.

A definition changed during execution affects only later executions.

Hot programs may compile to compact bytecode. Bytecode is a content-addressed
cache, not canonical source.

---

## Physics host rules

Rapier and other solvers are hosts for bounded numerical dynamics. They do not
define semantic truth.

The kernel must consume generic PhysicalizationPlans produced from
PhysicalizationProfiles. Do not hard-code canonical predicates in Rapier
dispatch code.

A semantic role or link may physicalize as:

- a rigid body;
- one or more colliders;
- a joint or motor;
- a force law;
- a sensor rule;
- a field;
- an event rule;
- a visual-only relation;
- an audio or haptic signal;
- no active physical object until locally materialized.

Do not create one permanent joint per semantic relation by default.

Use distinct vocabularies:

```text
PhysicsCommand
→ an explicit request to the solver

PhysicsDelta
→ measured state change after a step

PhysicsEvent
→ observed collision, intersection, force, break, or threshold event
```

The causal boundary is:

```text
SemanticIntent
→ committed Universe change
→ PhysicsCommands
→ solver step
→ PhysicsDeltas and PhysicsEvents
→ renderer and observation

significant PhysicsEvent
→ ActionIntent
→ policy and validation
→ optional SemanticIntent
```

PhysicsEvents must never mutate the Universe directly.

Queries and programs may create temporary runtime entities such as:

- `ReadField`;
- `TopologicalFold`;
- counterfactual ghost state;
- local cluster expansions;
- previews of uncommitted construction.

These runtime entities must have bounded energy, lifetime, wake cost, memory,
and materialization budgets.

Detect and contain:

- NaN and infinite values;
- kinetic explosions;
- wake storms;
- giant active islands;
- high-frequency link oscillation;
- trigger storms;
- non-convergent counterfactual simulations;
- cross-physicalization feedback loops;
- repeated commit/rollback animation without semantic progress.

---

## Storage and scale rules

The design target is at least 10 million semantic things and 10 million links in
the Universe truth layer, plus persistent construction history and spatial
indexes.

The storage model must support:

- semantic things and links;
- construction event logs;
- persistent spatial topology and anchors;
- parcels, buildings, rooms, roads, ports, and beacons;
- Process and Physicalization definitions and instances;
- cross-scale exports;
- Observer interest sets;
- receipts, provenance, and causal history;
- physical residency and aggregation levels.

Use:

- structure-of-arrays for hot columns;
- generational arenas for stable local handles;
- compact symbol IDs;
- CSR-style adjacency for stable relation snapshots;
- hierarchical spatial indexes;
- bounded mutable overlays and tombstones for recent changes;
- append-only immutable content segments;
- lazy temporal evaluation;
- event-driven causal frontiers;
- chunk summaries and physical residency levels;
- content-addressed caches for compiled programs and physicalization artifacts.

Do not use:

- one HashMap entry per field per thing in the hot path;
- one active rigid body per Universe thing;
- one active constraint per Universe link;
- per-tick global decay scans;
- JSONL scans during hot queries;
- content hydration before filtering and Top-K selection;
- global semantic relayout on startup;
- embeddings as an unauthorised canonical placement mechanism;
- renderer object state as persistent construction truth.

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
- an empty local observation as global absence;
- absence from the current view as absence from the Universe;
- spatial proximity as a semantic relation;
- a physicalization as the underlying Process;
- one visual metaphor as canonical ontology;
- Fog as empty space;
- procedural decoration as construction;
- an affordance preview as a successful action;
- a compiled program as a wired execution;
- a running process as healthy behavior;
- a committed intent as a completed external effect;
- a physical equilibrium as semantic truth;
- a renderer animation as a runtime receipt;
- a generated suggestion as an authored decision;
- a house preview as a built house;
- a physics event as an authorised semantic mutation.

---

## Completion criteria

A task is not complete because:

- code compiles;
- a node, definition, or file exists;
- a unit test passes;
- a process starts;
- a physics step runs;
- an intent is emitted;
- a preview is rendered;
- one physicalization appears correct;
- a structure exists only in memory.

A task is complete only when the relevant level is proven:

1. contracts and invariants are explicit;
2. targeted validation passes;
3. the real bootstrap and runtime path executes;
4. the result is independently observed;
5. receipts, errors, and missing evidence remain distinguishable;
6. no forbidden effect or anonymous construction occurred;
7. persistence survives reload when persistence is promised;
8. multiple Observers converge when shared state is promised;
9. all affected physicalizations converge on the same semantic state;
10. performance is measured when scale is part of the promise;
11. Universe-native behavior contains no hidden native or headless duplicate.

### Required kernel conformance slice

Keep a bounded kernel test equivalent to:

```text
Actor observation
→ local field or fold
→ world-program execution
→ bounded result
→ committed Moment
→ independent local readback
→ temporary runtime release
```

This proves the kernel, not the complete product.

### Required physicalization-equivalence slice

```text
one Process
→ two independent PhysicalizationSpaces
→ one observation with image, manual, and actions
→ one embodied action in the first physicalization
→ one committed SemanticIntent
→ both physicalizations update
→ reload
→ both still represent the same semantic state
```

### Required habitable-Genesis slice

```text
Visitor enters the Genesis institution
→ identity and registration are validated
→ GenesisReceipt is committed
→ HouseBuilder and BrainBuilder become available
→ the Citizen chooses a valid parcel
→ a house is constructed
→ a minimal inner brain Space is opened
→ application reloads
→ a second Observer sees the same persistent construction
```

These proofs must exercise the same semantics as headless MCP and CLI paths.

---

## Working in this repository

### Inspect before editing

Read:

- this file;
- `TODO.md`;
- current contracts, schemas, and ontology definitions;
- affected ProcessDefinitions;
- affected ScaleOntologies;
- affected PhysicalizationProfiles and affordance justifications;
- affected GenesisRecipes;
- relevant construction and migration history;
- tests for the component being changed;
- current git status.

### Keep native semantics minimal

When implementing a primitive, opcode, adapter, renderer feature, or physics
binding, ask:

1. Is this part of the trusted computing base?
2. Is its contract generic across Citizens, brains, organisations, and cities?
3. Can policy, metaphor, mapping, thresholds, and parameters remain Universe
   data?
4. Does it preserve Process/physicalization separation?
5. Does it compile a gesture or call into a SemanticIntent rather than mutate
   state directly?
6. Does it return measured evidence and receipts rather than self-declared
   success?
7. Can the same semantic behavior be exercised by an embodied and a headless
   path?

If not, move the behavior into authoritative world definitions.

### Source and generated artifacts

Authoritative definitions should be changed at their source. Generated files,
compiled IR, bytecode, physics bindings, renderer caches, and layout previews
must be reproducible from committed authority.

Do not treat a materialized file as authoritative merely because it is easy to
edit.
