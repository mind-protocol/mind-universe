# CLAUDE.md

## Avant tout — où tu es

Tu n'es pas dans un dépôt de code. Tu es dans une ville, et tu la bâtis.

- **Pense en objets, pas en code.** La question n'est jamais « quelle fonction
  écrire » mais « quelle chose placer, à quel endroit, qui la tient, ce qu'elle
  offre, comment elle prouve son effet ».
- **Parle en choses.** Une balise, un stylo, une maison, un atelier, une
  capacité tenue — pas des fichiers, des fonctions et des modules. Ceux-là ne
  reviennent que quand tu touches vraiment le socle.
- **Tu es situé.** Arrive quelque part, perçois depuis là, puis agis. Pas
  d'édition hors-sol : on ne devine pas le monde, on le regarde.
- **Écrire, c'est construire.** Un acte attribué (qui a demandé, pourquoi), une
  écriture atomique, relue indépendamment ensuite. Ce que tu as bâti est là où
  tu l'as posé.
- **Le code natif est l'exception**, réservé au socle de confiance. Si une règle,
  un seuil ou une métaphore peut varier, c'est un objet du monde — pas une ligne
  de Rust.
- **C'est fini quand c'est prouvé** : exécuté pour de vrai, observé
  indépendamment, l'effet distinguable et attribuable. Jamais « ça compile ».
- **Dis ce que tu ignores.** Vide n'est pas zéro ; absent d'une vue n'est pas
  absent du monde ; un `unknown` honnête vaut mieux qu'une histoire cohérente.

## Les deux seuls mots : Toolkit et Construct

Tout le vocabulaire de ce que tu bâtis tient en deux mots. Il n'y a ni « Tool »
ni « exécution de toolkit » : **tout ce qui est bâti est un Construct**.

**Toolkit** — une capacité causale réutilisable : un savoir-faire, un contrat
d'affordance, un mécanisme. La recette. Elle n'habite aucun monde et ne fait
rien toute seule.

**Construct** — ce qu'un Toolkit produit : la chose bâtie, posée quelque part
dans un monde, qui **prouve ses propres effets**. Un stylo dans un atelier, une
alarme de maison, une institution. Un Toolkit peut donner beaucoup de
Constructs, comme une recette donne des repas.

Un Construct porte son **anatomie** — et cette anatomie *est* ton programme.
C'est elle que tu remplis, au lieu d'écrire une suite d'instructions :

```text
Objectif · Affordances · Entrées · Préconditions · Mécanisme
Effets · Reçus · Observateur · Métriques · Santé · Maintenance
```

Déclaratif, inspectable, réparable. Un Construct sans Reçus ni Observateur
n'est pas un Construct : c'est une affirmation.

**Un Construct ne scrute jamais.** Il se câble dans la physique et attend d'être
déclenché ; dormant, il ne coûte rien :

```text
Sensor      — une condition physique posée dans le champ
DepositBond — l'événement dépose de l'énergie sur un atome déclencheur
Threshold   — l'atome s'allume au-delà de N
Effect      — l'allumage émet une intention
```

Une alarme de maison, en entier : un capteur sur la membrane d'entrée, un dépôt
quand un corps de citoyen le traverse, un seuil à un franchissement, un `notify`.
Aucune boucle, aucun scan.

**Chaque Construct habite un niveau** — pose-le au plus bas niveau qui tient
vraiment la capacité :

```text
L1  monde intérieur — sous-entités, œil intérieur, outils internes
L2  monde partagé   — les constructs et institutions de la ville
L3  corps citoyen   — perception, cognition, action ; la membrane incarnée
L4  loi universelle — dynamique des atomes, capacités, attribution (socle)
```

Le cycle d'écriture : décider le comportement et le niveau → écrire le Toolkit →
réutiliser les symboles canoniques (viser zéro nouveau symbole) → valider →
injecter en **une** écriture atomique attribuée → relire indépendamment →
vérifier que ça se déclenche et que l'effet est bien là quand on regarde.

Le reste de ce fichier est la doctrine détaillée. Ceci en est le sol.

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
- Moments, evidence, health, maintenance, attribution, and external effects;
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
→ carries out measured transformations, attributably
→ returns independent observations
```

Do not turn the bootstrap kernel into a second hidden application layer.

---

## How you work here

Everything is a construct — including you, your tools, and the way you touch the
ground beneath the world.

- **Start every session by creating an actor in the right place of the city.**
  You are a situated actor, not a disembodied editor. Bring yourself into being
  where the work lives, and perceive from there, before you act.
Here, the developer is a citizen and development is construction.

---

## The world-first architecture — current direction

This section is the concrete shape the general doctrine now takes; it refines
the framings further below.

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
The native floor knows only how to detect, resolve, step, write, and attribute.
The *meaning* of writing, perceiving, entering, or notifying lives in constructs.

### The four levels

```text
L1  inner world    — sub-entities, the mind-eye, inner tools
L2  shared world   — the constructs and institutions of the city
L3  citizen body   — perception, cognition, action; the incarnated membrane
L4  universal law  — atom dynamics, signatures, capabilities, attribution,
                     self-verification, toolkit resolution (bedrock)
```

A ToolkitDefinition lives at the **lowest level that truly holds the
capability**. One definition may have a physicalisation per level (an Energetic
Pen as a public L2 tool and as a Captain's inner L1 tool) sharing claims,
sources, sealing and attribution, with effects situated in their own world.

The membrane is not an object. Isolation of what crosses between levels is (a)
the L3 body's admission constructs (ReceiveInnerIntent, ComposeIntent,
AttemptIntent) carrying the *meaning* of a crossing, and (b) bounded physical
residency carrying the hard containment. **L3 is the incarnated membrane.**

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

The toolkits authored so far, by the `dominant_scale` each one declares
(`fixtures/ontology/*-toolkit-v0.json`):

```text
Energy         → none authored yet
Mechanism      → Mechanical toolkit    assemble modules into a machine
Sensemaking    → Abstraction toolkit   create, store, prove and show abstractions
                 Appearance toolkit    dress a construct affordance by affordance
Infrastructure → Construction toolkit  one mechanism builds a room, a route or a parcel
                 Underground toolkit   the buried network — immutable mechanisms
Civic          → Sky toolkit           programmable luminous constellations
telecom        → Telecom toolkit       wire mechanical effects onto distant things
```

`telecom` is an authored scale label, not one of the initial families and not a
native enum — a scale family is Universe data, so the list above grows without
touching the kernel.

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
signals must flow through those exports, attributably, and through declared
mappings rather than hidden coupling.

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
- feedback for an action that lands and one that does not;
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

An action in one physicalization produces one SemanticIntent. Every other active
physicalization of the same Process must then converge on the state the Process
now holds.

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
- a request that did not land is never shown as if it had.

### Physicalization completeness

A production physicalization should be evaluated on:

1. ontological coverage: relevant types and predicates are representable;
2. operational coverage: every legal operation has an affordance or headless
   equivalent;
3. fidelity: no gesture implies a semantic effect it cannot produce;
4. resistance: illegal actions fail perceptibly;
5. explainability: the result carries its own cause — readable from the node,
   not reconstructed by walking a history;
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
    recent_actions: Vec<ActionSummary>,
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
- error paths and attribution.

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
→ Universe change
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
- previews of construction that is not built.

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
- repeated animation of change without semantic progress.

---

## Storage and scale rules

The design target is at least 10 million semantic things and 10 million links in
the Universe, plus persistent construction history and spatial indexes.

The storage model must support:

- semantic things and links;
- construction event logs;
- persistent spatial topology and anchors;
- parcels, buildings, rooms, roads, ports, and beacons;
- Process and Physicalization definitions and instances;
- cross-scale exports;
- Observer interest sets;
- attribution, provenance, and causal history;
- physical residency and aggregation levels.

Use:

- structure-of-arrays for hot columns;
- generational arenas for stable local handles;
- compact symbol IDs;
- CSR-style adjacency for stable relation snapshots;
- hierarchical spatial indexes;
- bounded mutable overlays and tombstones for recent changes;
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
- a Universe change as a completed external effect;
- a physical equilibrium as semantic truth;
- a renderer animation as evidence of a real effect;
- a generated suggestion as an authored decision;
- a house preview as a built house;
- a physics event as an authorised semantic mutation.

---

## Test slices

### Required kernel conformance slice

Keep a bounded kernel test equivalent to:

```text
Actor observation
→ local field or fold
→ world-program execution
→ bounded result
→ Moment written
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
→ one SemanticIntent applied
→ both physicalizations update
→ reload
→ both still represent the same semantic state
```

### Required habitable-Genesis slice

```text
Visitor enters the Genesis institution
→ identity and registration are validated
→ the admission is recorded, attributably
→ HouseBuilder and BrainBuilder become available
→ the Citizen chooses a valid parcel
→ a house is constructed
→ a minimal inner brain Space is opened
→ application reloads
→ a second Observer sees the same persistent construction
```

These proofs must exercise the same semantics as headless MCP and CLI paths.
