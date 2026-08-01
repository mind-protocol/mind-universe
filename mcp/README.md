# mind-mcp

A **window onto one Universe** over MCP. Two tools: `arrive` (announce yourself)
and `sense` (perceive a bounded situation). There is **no transform verb** —
changing the world is not a transport's to offer.

The adapter holds no logic. It frames, encodes, and routes; the observation it
serialises — image included — is composed by `universe_supervisor::perception`,
which is also what the endogenous L1 loop reads. A rule that could change what a
caller learns about the Universe would be Universe logic, and would belong in a
crate or a construct, not here (CLAUDE.md, "MCP is a pipe, not a host").

```text
UniverseSnapshot ──→ universe_supervisor::perception ──→ Observation
                     (POV, text, objects, affordances,      │
                      processes, changes, image frame)       │
                                                            ▼
                                          mind-mcp: frame · encode · route
                                                            │
                                                            ▼
                                                       MCP client
```

## Transport

JSON-RPC 2.0 over **stdio**, one message per line. Implemented methods:

- `initialize` → protocol handshake, advertises the `tools` capability.
- `notifications/initialized` (any notification) → acknowledged by silence.
- `ping` → `{}`.
- `tools/list` → the two tools below.
- `tools/call` → dispatch to `arrive` / `sense`. Any other name is a JSON-RPC
  error (`-32602`, `unknown tool: <name>`).

Diagnostics go to **stderr**; only JSON-RPC frames go to stdout.

Every result carries a `content` block plus machine `structuredContent`. The
human-readable text is a pretty-printed mirror of the structure, **capped at
5000 chars** (uniform across tools — the assembly layer carries no per-tool
logic); `structuredContent` is left whole.

## Sessions — the admission law

Every call carries an **`ActorSession`**. Entering Lumina Prime is not the right
to transform it: the city Gate mints an *ephemeral, traceable* envelope
(`TransientActor`) with minimal safe capabilities. Unauthenticated is never
untraceable — every visit has a provenance, every power a source.

`arrive` mints the session and returns an admission receipt (`SessionPassport`,
`CapabilityEnvelope`, `ExpirationCondition`). An unknown `actor_id` on `sense` is
auto-admitted as a traceable walk-in visitor — a presence never exists without
provenance. Sessions live in an in-process registry and expire (default TTL 2 h);
an expired id is re-minted as a walk-in.

| standing | capabilities | may |
| --- | --- | --- |
| `unauthenticated_visitor` | observe, speak | perceive, dialogue, propose *only with a sponsor* |
| `sponsored_visitor` | + propose (in scope) | be embodied as a durable L1 inhabitant (`arrive`'s one write) |
| `authenticated_actor` | per capabilities | (contract; needs an auth path) |
| `citizen` | sovereign | (contract; needs L1 identity) |

`fire`, `own`, `delegate`, `emergency_broadcast`, `enter_private_l1`,
`read_personal_memory` are **never** granted to a visitor — requested ones come
back in `denied` with a reason. A visitor may *signal* an emergency, never
proclaim it.

## Mounting a Universe

The adapter boots a real `Supervisor` from the environment, mirroring the
`universe-server` binary:

| variable | meaning |
| --- | --- |
| `UNIVERSE_STORE` | store directory to replay |
| `UNIVERSE_GENESIS` | genesis json (used if the store is empty) |
| `MIND_ACTOR_ANCHOR_CANONICAL_ID` | overrides the identity a new inhabitant is attached to (default: the orientation beacon, below) |

If either of the first two is unset or the boot fails, the adapter still serves,
but every `sense` reports `uncertainty: unknown` with the reason, and embodiment
reports `materialized: false` — it never fabricates a world.

```bash
UNIVERSE_STORE=./path/to/store UNIVERSE_GENESIS=./genesis.json cargo run --manifest-path mcp/Cargo.toml
```

## `arrive` — announce yourself

Input (all optional): `session_id` (a stable id used on later `sense` calls;
generated if omitted), `origin` (declared, traceable even when unauthenticated),
`sponsor`, `requested_capabilities`, `requested_scope`.

Output: the admission receipt — `admitted`, `arrival_position` (the Porte
d'Arrivée, `[0, -500, 0]`), `passport`, `capability_envelope`, `expiration`,
`denied` (each refusal with its reason), `note`.

### The one write: a sponsored session becomes an inhabitant

Admission alone is a row in a map, gone at expiry and invisible to the running
world. When the arriving session **holds `Propose`** — i.e. a sponsor was named,
making it a `sponsored_visitor` — `arrive` additionally writes a durable `actor`
node into the one reality (`World::materialize_actor`), so the presence persists
as an L1 inhabitant and appears on the live desktop. The outcome is folded into
the receipt as an `embodiment` block.

This is the **only** write the adapter performs, and it is not a general one: it
takes **no caller-supplied intent**, so nothing a caller says can steer what is
written. Concretely:

- **Fail-closed authority gate.** No `Propose`, no embodiment — refused before
  any store handle is opened, with a reason naming the session and the missing
  capability. A walk-in is admitted only; nothing is written.
- **Generic four-verb write.** Each step is a `MutationPlan` compiled through
  `universe_e2e::mutation_translate::translate_mutation_proposal`, so a mutation
  compiles to exactly one kernel verb — a fifth is unrepresentable — gathered
  into ONE atomic write set and committed at the next tick boundary.
- **Identity, not a baked key.** The node is typed `actor` and carries a
  top-level `canonical_id` of the form `actor:l1:mind:claude-<sanitised session
  id>` (a leading `claude:`/`claude-` marker is stripped first, so the prefix is
  never doubled) — the exact convention perception reads to recognise an L1
  inhabitant — plus `provenance: "built"`, the session's origin, sponsor, and
  status.
- **Attached to the orientation beacon.** A canonical `PART_OF` edge to Balise
  Zéro, resolved at runtime by its canonical identity
  (`space:l2:lumina-prime:orientation-beacon-v0`, env-overridable) read from
  store **data** — never a baked hex key. `PART_OF` is already canonical, so no
  new predicate symbol is minted; any genuinely missing symbol is interned and
  **reported** in `interned_symbols`, never silently assumed.
- **Honest absence.** If nothing carries the anchor identity, the actor node is
  written anyway and the edge is reported **dropped** — an inhabitant with no
  beacon is honest, a dangling edge is not.
- **Idempotent.** The entity key is deterministic (FNV-1a over
  `mcp:arrive:embody:<session_id>`, in the `0x0AC0` prefix block), so re-arriving
  with the same session id yields `AlreadyCommitted`: no duplicate inhabitant,
  `committed_effects: []`.
- **Independent readback.** Evidence comes from a **fresh store replay**
  (`Supervisor::independent_readback`), never the committing snapshot's own word.

```json
"embodiment": {
  "materialized": true,
  "actor_key": "0ac0…",
  "canonical_id": "actor:l1:mind:claude-8dbd9ddf-…",
  "idempotent": false,
  "node_written": true,
  "actor_present": true,
  "revision": { "from": 268, "to": 269 },
  "part_of_beacon": { "beacon_key": "…", "edge_present": true, "dropped": false },
  "interned_symbols": [],
  "committed_effects": [
    {"put_entity": "0ac0…", "canonical_id": "actor:l1:mind:claude-…", "node_type": "actor"},
    {"put_relation": "0ac0…01", "predicate": "PART_OF", "target": "…"}
  ],
  "evidence": [{"independent_readback": {"revision": 269, "actor_present": true, "part_of_beacon_present": true}}]
}
```

A failed embodiment is reported, not thrown: `{"materialized": false, "error":
"…"}` — an unmounted Universe, a refused authority, or a rejected prepare all
come back as an honest block, and a prepare rejection commits nothing.

## `sense` — perceive without changing anything

Inputs, all optional: `actor_id` (the perceiving session), `where` (a place,
Space, Actor or object to observe from — an EntityKey hex or a symbol name;
defaults to the actor), `focus`, `scale`, `since`. The session passport rides in
`situation.session`.

Output: `{ situation, pov, text, objects, processes, changes, affordances,
uncertainty }` — plus the first-person frame as a real MCP **`image`** block.
`uncertainty` is `inferred` whenever a Universe is mounted and `unknown` when it
is not; `measured` is reserved for a genuinely measured signal and positions are
never one.

### Through whose eyes

`actor_id` is not required, and how it resolved is always reported in
`situation.actor_resolution` / `situation.embodied_actor` — a caller is never
misled into thinking it chose the inhabitant.

| `actor_id` | `actor_resolution` | vantage |
| --- | --- | --- |
| a 32-hex EntityKey or a type symbol that matches | `named` | situated at that node's inferred position |
| an id matching an actor node's stored `canonical_id` (e.g. `claude:<uuid>`) | `named_canonical` | situated at that actor's own placement |
| given, but nothing matches | `named` | `external_observer`, a vantage derived from the placed set |
| omitted | `random_l1` | situated in an arbitrary L1 inhabitant — you see from *within* the city |
| omitted, and the world holds no L1 actor | `no_l1_actor_external` | `external_observer` |

An actor with no inferred position of its own also falls back to
`external_observer`. No vantage is ever a hardcoded coordinate.

### The frame is a sphere, and it is bounded

Perception gathers a bounded candidate **cluster** by walking the graph outward
from the origin (breadth-first, so a truncated cluster is still the nearest
ring), solves the layout over that cluster, then keeps the **sphere** of it
around the actor's position — a spatial budget, not an adjacency depth. Budgets:
≤ 64 objects, ≤ 128 relations, ≤ 384 cluster candidates. `situation.sphere`
reports `center`, `radius_m`, `radius_source` (`requested` or
`self_calibrated_to_budget` — absent an explicit radius, the sphere calibrates to
the shell that just holds the object budget), `cluster_candidates`,
`within_sphere` and `materialised`; `situation.completion` is `complete` or
`budget_exhausted`. No `sense` scans the whole Universe.

### Positions are inferred from the physics

A node's place in an observation is the **output of the graph-native layout
solver** (`universe-assets::layout`), settled from link forces and containment —
perception reads no stored coordinate, and reports each object as
`position_source: "inferred_from_physics"` or, when the solver did not place it,
`"unplaced"` (present, never given an invented coordinate). It is a derivation,
never a measurement — hence `uncertainty: inferred`. The adapter never writes a
coordinate. (Fidelity gap: force profiles / similarity are not sampled here, so
this layout agrees with the renderer's in shape but not in force detail.)

### What comes back

- **`pov`** — `{ actor, generated, eye, eye_source, look_at, yaw, pitch,
  projection }` over the sphere.
- **`text`** — the first-person text of sense, in French: a situation line at the
  world's *derived* place, then the nodes around you
  grouped by their authored name with their affordance verbs as bullets. The
  situation line names the **origin** and reads it three ways: `Tu es … (rev N)`
  when the origin is the perceiver itself (the default call, where `where` is
  omitted and the origin falls back to the embodied actor), `Tu es près de …`
  when a `where` puts the origin somewhere else, and `Tu observes …` from an
  external-observer vantage. Names,
  verbs and place are all derived from store data — never a hand-maintained
  dictionary — and the render is byte-budgeted (2500), breadth before depth.
- **`objects`** — the bounded set, each with `semantic_type`, its `identity`
  (`canonical_id`), a readable `name`, position, distance and bearing.
- **`affordances`** — the **real** affordance-parts of the perceived objects,
  read from the graph, never the adapter's tool names. Each carries `verb`,
  `target`, a `genre` (`visible` — rendered only; `active` — its precondition
  gate has fired; `inert` — present but not yet invocable, which *is* a need), an
  `invocable` flag that is `true | false | "unknown"` (honest: the gate's fired
  state is not always readable from a committed snapshot, so `true` is never
  fabricated), a `gate` kind, and `precondition` / `effect` / `justification`.
- **`processes`** — from the runtime inventory.
- **`changes`** — the measured `revision` and `tick`, plus the `since` echo. An
  itemised receipt log for `since` is a **declared gap**, stated in the payload.
- **the image** — a first-person JPEG of the physics-sphere, composed by the
  observation itself (`Observation::with_frame`), captioned with the universe,
  revision and tick it was drawn at and the fact that the projection is inferred.
  The adapter **moves** it out of `structuredContent` into the MCP `image` block
  rather than carrying ~40 KB of base64 twice; a `null` stays in place, so "no
  frame" remains distinguishable from "no such field". There is no frame when
  there is no vantage or nothing placed to see — honest absence, never a picture
  of nothing.

`sense` **cannot** mutate the Universe, move an Actor, create or delete a
construction, change a permission, or trigger a business process.

## There is no write verb

`tools/call` with `act` — or any other name — returns a JSON-RPC error. This is
deliberate and tested: a caller asking the pipe to change the world is told
plainly that it cannot, rather than being handed a transaction.

What proves an effect in this world is **reading the thing itself**: state is
atomised, so the evidence a write landed is the node, now — not a journal entry
about it, and not a label in a response. Reads here are served from a committed
snapshot and an observation states the revision it was taken at, so an effect
missing from a view means *not in this view*, never *rejected*.

Real transformation belongs to the world's own loop — constructs that wake on a
physics event, and L1 actors whose turn is one inference proposing a justified
intent that the body attempts. The pipe watches that happen; it does not drive it.

## Tests

```bash
cargo test
```

Run it **from inside `mcp/`**: this is a *detached* workspace (a standalone
bootstrap binary reaching the kernel through path dependencies), so a
workspace-wide cargo run from the repo root does not cover it.

Covers, in this crate: the two-tool listing and that there is no write verb; the
`initialize` handshake; notifications answered by silence and unknown methods as
`method not found`; `arrive` minting a traceable passport; `sense` accepting no
actor and reporting `unknown` when unmounted; the frame moved, never copied, out
of `structuredContent`; the admission law (sponsor unlocks `propose`, dangerous
capabilities denied with a reason, walk-ins traceable, density by standing); and
embodiment end-to-end against a real mounted Genesis — a sponsored session
written and independently read back, idempotent on re-arrival, the anchor
resolved by canonical id at an arbitrary key, the edge dropped and reported when
no beacon exists, and a walk-in refused fail-closed with the revision unmoved.

The observation itself is tested where it is composed — in
`universe-supervisor::perception`, not here.
