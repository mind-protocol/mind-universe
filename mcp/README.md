# mind-mcp

A **minimal, two-verb MCP adapter** into the Universe: `sense` (perceive) and
`act` (transform). It is a *headless interface into the same Universe semantics*
as the embodied 3D world — not a second ontology, not a second planner
(CLAUDE.md, "Headless adapters"; ideas.md §4 "Tools = Things").

```text
world gesture  ─┐
                ├─→ one SemanticIntent → one planner/validator → one transaction
MCP sense/act  ─┘                     → one receipt → one observable update
```

## Transport

JSON-RPC 2.0 over **stdio**, one message per line. Implemented methods:

- `initialize` → protocol handshake, advertises the `tools` capability.
- `notifications/initialized` → acknowledged by silence.
- `ping` → `{}`.
- `tools/list` → the three tools below.
- `tools/call` → dispatch to `arrive` / `sense` / `act`.

Diagnostics go to **stderr**; only JSON-RPC frames go to stdout.

## Sessions — the admission law

Every call carries an **`ActorSession`**. Entering Lumina Prime is not the right
to transform it: the city Gate mints an *ephemeral, traceable* envelope
(`TransientActor`) with minimal safe capabilities. Unauthenticated is never
untraceable — every visit has a provenance, every power a source.

`arrive` mints the session and returns an admission receipt (`SessionPassport`,
`CapabilityEnvelope`, `ExpirationCondition`). An unknown `actor_id` on
`sense`/`act` is auto-admitted as a traceable walk-in visitor — a presence never
exists without provenance.

| standing | capabilities | may |
| --- | --- | --- |
| `unauthenticated_visitor` | observe, speak | perceive, dialogue, propose *only with a sponsor* |
| `sponsored_visitor` | + propose (in scope) | assemble real-but-inert mechanisms within the sponsor's perimeter |
| `authenticated_actor` | per capabilities | (contract; needs an auth path) |
| `citizen` | sovereign | (contract; needs L1 identity) |

`fire`, `own`, `delegate`, `emergency_broadcast`, `enter_private_l1`,
`read_personal_memory` are **never** granted to a visitor — requested ones come
back in `denied` with a reason. A visitor may *signal* an emergency, never
proclaim it.

## Mounting a Universe

The adapter boots a real `Supervisor` from the environment, mirroring the
`universe-server` binary:

| variable           | meaning                              |
| ------------------ | ------------------------------------ |
| `UNIVERSE_STORE`   | store directory to replay            |
| `UNIVERSE_GENESIS` | genesis json (used if the store is empty) |

If either is unset or the boot fails, the adapter still serves, but every
`sense` reports `uncertainty: unknown` and every `act` reports the honest gap —
it never fabricates a world.

```bash
UNIVERSE_STORE=./path/to/store UNIVERSE_GENESIS=./genesis.json \
  cargo run --manifest-path mcp/Cargo.toml
```

## `sense` — perceive without mutating, *as a session*

Every call carries an **`actor_id`** — the session id. If it matches a graph
entity it is a *situated* actor; otherwise it is a **visitor**, and its POV is
placed at the **Porte d'Arrivée** `[0, -500, 0]` facing **Balise Zéro** `[0,0,0]`
— the civic arrival frame. Other inputs are optional: `where` (observe from a
place; defaults to the actor), `focus`, `scale`, `since`. The session passport
rides in `situation.session`.

Output: `{ situation, pov, text, objects, processes, changes, affordances,
uncertainty }` where `uncertainty ∈ { measured, inferred, unknown }`:

- **`pov`** — the actor's `{ eye, look_at, yaw, pitch }` over the neighbourhood,
  plus `generated` and `projection`.
- **`text`** — the first-person *text of sense*: the actor and the nearest
  visible **spheres**, by bearing and distance (the full bounded set stays in
  `objects`, each with its `semantic_type`).

### Positions are inferred from the physics — none are stored

There are **no coordinates in the store**. A node's place is the OUTPUT of the
graph-native layout solver (`universe-assets::layout`), which settles positions
from link forces + containment. `sense` runs that solver over the **bounded
neighbourhood only** (never the whole Universe) and reports the result as
`position_source: "inferred_from_physics"`. It is a derivation, never a
measurement — `uncertainty` is `inferred`, never `measured`; the adapter never
writes a coordinate. A situated actor's eye is its inferred position; anyone else
observes from an `external_observer` vantage derived from the placed set. Nodes
the solver did not place are `unplaced` — present, never given an invented
coordinate. Observation is **bounded** (one hop, ≤64 objects / ≤128 relations,
nearest 12 in the text). (Fidelity gap: force profiles / similarity are not yet
sampled here, so the layout agrees with the renderer's in shape but not force
detail.)

`sense` **cannot** mutate the Universe, move an Actor, create/delete a
construction, change a permission, or trigger a business process.

## `act` — request a real transformation

Input: `actor_id` (the acting session — the transformation is admitted only
against its capabilities), `intent` (required, may be natural language),
`target`, `constraints`, `proof`.

**`act` returns what `sense` returns.** There is only one reality, so the honest
result of acting is the world *after*, perceived. `act` commits, the tick
advances, and it returns the actor's POV of the new revision — the exact same
`Observation` shape as `sense` — with the committed delta in `changes`.

### The write path is wired — a real, inert, committed proposal

`act` **commits for real**. It records the intent as a `proposal` entity
(`provenance: "built"`, carrying the intent + its builder session) plus an
authored `construction_moment` (a Built fact with no construction Moment is a
forgery), links them, commits the set as **one atomic transaction at the next
tick boundary**, and reads it back from a fresh store replay. The revision
advances; the nodes persist across processes.

```json
"changes": {
  "committed_effects": [ {"put_entity":"…","kind":"proposal"}, {"put_entity":"…","kind":"construction_moment"}, {"put_relation":"…","predicate":"CONSTRUCTED_BY"} ],
  "evidence": [ { "independent_readback": { "revision": 1, "proposal_present": true, "moment_present": true, "constructed_by_present": true } } ],
  "revision": { "from": 0, "to": 1 },
  "idempotent": false
}
```

Re-running the same `(session, intent)` is **idempotent**: `AlreadyCommitted`,
the tick does not advance, `committed_effects: []` — but the readback still shows
the node present. You read the world back to see what happened; you never trust a
label.

The proposal is **inert**: it records that the intent was proposed, it does not
yet assemble and fire the live mechanism — `remaining_gap` says so. Realising a
proposal as a running mechanism (and NL → a richer SemanticIntent than a
recorded proposal) is the next step; the shape is final.

### Injecting an authored fixture

Pass a `fixture` path and `act` injects the whole subgraph (root + members +
relations) as **one atomic transaction** instead of a plain proposal —
generalising the `inject_*` bins:

```json
{"name":"act","arguments":{
  "actor_id":"nlr-guest",
  "intent":"install the Lumina Prime Energy Pen",
  "fixture":"fixtures/ontology/lumina-prime-energy-pen-v0.json"
}}
```

Mount the **canonical** store (which carries the predicates: PART_OF, IMPLEMENTS,
GROUNDS, …) so injection interns **0 new symbols**:
`UNIVERSE_STORE=artifacts/ontology-registry/current/store`. Authored predicates
are remapped to canonical (e.g. `IMPLEMENTED_IN → IMPLEMENTS`, swapped); a
relation whose endpoint is not in the fixture is **dropped and reported**, never
dangled (the Pen's `PART_OF → city-v0` drops — the city isn't built); any missing
symbol is **interned and reported**, never silently minted. `changes.injection`
carries `{ fixture_id, nodes_injected, relations_kept, relations_dropped,
interned_symbols }`, with the independent readback in `evidence`. Re-injecting is
idempotent. The subgraph is `graph_status: WRITTEN` — wiring / runtime / health
stay not_wired / not_running / not_measured (a written loop is not a running one).

Pass an optional `position: [x, y, z]` (metres, near Balise Zéro `[0,0,0]` = the
city centre) to also **site** the fixture root: `act` writes an authored
`physical_profile → position_mm:X,Y,Z` placement + a construction Moment
(provenance `built`), in the same atomic transaction. Some fixtures (the Pen) are
deliberately placement-free — siting one is an **authoring decision**, recorded as
a Built fact, never a forged measurement. A later `sense` reads it back as an
`authored` object at that position.

## Tests

```bash
cargo test --manifest-path mcp/Cargo.toml
```

Covers: bounded neighbourhood observation, exact vs. inferred origin resolution,
unmounted honesty, the two-tool listing, and `act` never fabricating a commit.
