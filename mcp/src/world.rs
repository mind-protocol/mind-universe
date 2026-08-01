//! The mounted Universe the adapter observes.
//!
//! `sense` is a headless interface into the SAME Universe semantics as the 3D
//! world (CLAUDE.md, "Headless adapters"). The adapter therefore holds a real
//! [`Supervisor`] booted from a store + genesis, never a private projection.
//! When no store is mounted the adapter says so honestly instead of fabricating
//! a world: an unmounted `sense` is `unknown`, not empty.
//!
//! There is no general write path here. The adapter is a pipe, not a host
//! (CLAUDE.md, "MCP is a pipe, not a host"): changing the world is not a
//! transport's to offer. The one write that remains is [`World::materialize_actor`],
//! reached only from `arrive`, which turns an admitted sponsored session into a
//! durable inhabitant.

use std::env;
use std::path::PathBuf;

use serde_json::{json, Value};
use universe_core::{EntityKey, RelationKey, UniverseError};
use universe_e2e::canonical::canonical_predicate;
use universe_e2e::mutation_translate::{translate_mutation_proposal, MutationPlan};
use universe_store::{ContentRef, UniverseSnapshot, UniverseStore};
use universe_supervisor::{PhaseHook, RuntimeInventory, Supervisor, TickPhase};
use universe_transactions::{CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet};

use crate::session::{ActorSession, Capability, RecalledBody};

/// Environment variables that point the adapter at a real Universe, mirroring
/// the `universe-server` binary's positional arguments.
const STORE_ENV: &str = "UNIVERSE_STORE";
const GENESIS_ENV: &str = "UNIVERSE_GENESIS";

/// A mounted Universe, or an honest absence.
pub enum World {
    /// A booted supervisor over a real store + genesis. `store_root` is kept so
    /// the write path can open a second store handle for content-addressing
    /// through the generic mutation translator (which content-addresses via a
    /// `UniverseStore`, not the supervisor's private handle).
    Mounted {
        supervisor: Supervisor,
        store_root: PathBuf,
    },
    /// No store was configured, or the boot failed. The reason is preserved so
    /// `sense` can report *why* the world is unknown rather than pretending it
    /// is empty.
    Unmounted { reason: String },
}

impl World {
    /// Boots from `UNIVERSE_STORE` + `UNIVERSE_GENESIS` if both are set.
    ///
    /// A missing configuration or a failed boot is not fatal: the adapter still
    /// serves, and every `sense` reports the honest unmounted state. This keeps
    /// the transport observable for tooling without ever inventing a Universe.
    pub fn from_env() -> Self {
        let store = env::var(STORE_ENV).ok().filter(|value| !value.is_empty());
        let genesis = env::var(GENESIS_ENV).ok().filter(|value| !value.is_empty());
        match (store, genesis) {
            (Some(store), Some(genesis)) => Self::mount(PathBuf::from(store), PathBuf::from(genesis))
                .unwrap_or_else(|error| World::Unmounted {
                    reason: format!("supervisor boot failed: {error}"),
                }),
            _ => World::Unmounted {
                reason: format!(
                    "no Universe mounted: set {STORE_ENV} and {GENESIS_ENV} to a store directory and genesis json"
                ),
            },
        }
    }

    /// Boots a supervisor over an explicit store + genesis and mounts it. Used by
    /// `from_env` and by tests that need a real, mounted Universe.
    pub fn mount(store_root: PathBuf, genesis: PathBuf) -> Result<Self, UniverseError> {
        let supervisor = Supervisor::boot(&store_root, genesis)?;
        Ok(World::Mounted {
            supervisor,
            store_root,
        })
    }

    pub fn snapshot(&self) -> Option<&UniverseSnapshot> {
        match self {
            World::Mounted { supervisor, .. } => Some(supervisor.snapshot()),
            World::Unmounted { .. } => None,
        }
    }

    pub fn runtime_inventory(&self) -> Option<RuntimeInventory> {
        match self {
            World::Mounted { supervisor, .. } => Some(supervisor.runtime_inventory()),
            World::Unmounted { .. } => None,
        }
    }

    /// Reads an entity's content value, or `None` when unmounted / unreadable.
    /// This is how `sense` recovers a node's `canonical_id` (its identity) from
    /// the content segment, without a second store handle.
    pub fn read_content(&self, content: &ContentRef) -> Option<serde_json::Value> {
        match self {
            World::Mounted { supervisor, .. } => supervisor.read_content(content).ok(),
            World::Unmounted { .. } => None,
        }
    }

    pub fn unmounted_reason(&self) -> Option<&str> {
        match self {
            World::Mounted { .. } => None,
            World::Unmounted { reason } => Some(reason.as_str()),
        }
    }

    /// Looks for the **durable body** this session already built, and reads its
    /// recorded facts back. Read-only: it opens no store handle, writes nothing,
    /// and reads the content of at most ONE entity.
    ///
    /// `materialize_actor` derives the body's key deterministically from the
    /// session id (that is what makes re-arriving idempotent), so recalling it is
    /// a direct key lookup rather than a scan — the same contract read backwards.
    ///
    /// The body is only accepted when its own `embodied_session` equals the id
    /// asked for. A key that exists but names a different session is a collision
    /// or a reused block, never this presence: it returns `None` and the caller
    /// falls back to a fresh walk-in (fail-closed).
    ///
    /// An unmounted world recalls nothing — honestly `None`, which is `unknown`
    /// and produces a walk-in, never a fabricated standing.
    pub fn recall_body(&self, session_id: &str) -> Option<RecalledBody> {
        let World::Mounted { supervisor, .. } = self else {
            return None;
        };
        let key = actor_key_for_session(session_id);
        let snapshot = supervisor.snapshot();
        let entity = snapshot.entities.iter().find(|e| e.key == key)?;
        let content = supervisor.read_content(entity.content.as_ref()?).ok()?;
        // Fail-closed identity check: the body must name THIS session itself.
        let embodied = content.get("embodied_session").and_then(Value::as_str)?;
        if embodied != session_id {
            return None;
        }
        Some(RecalledBody {
            body_key: key.to_string(),
            sponsor: content
                .get("sponsor")
                .and_then(Value::as_str)
                .map(str::to_owned),
            origin: content
                .get("origin")
                .and_then(Value::as_str)
                .map(str::to_owned),
            base_revision: content.get("base_revision").and_then(Value::as_u64),
            recorded_status: content
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }

    /// Embodies a SPONSORED session as a **durable L1 inhabitant** — the write
    /// that turns admission into residency.
    ///
    /// `arrive` alone mints only an *ephemeral* `ActorSession`: a row in a map,
    /// gone at expiry, invisible to the running world. When the arriving session
    /// holds the `Propose` (write) capability — a SPONSORED visitor (sponsor
    /// `nlr_ai` and the like) — this additionally writes a real `actor` node into
    /// the one reality, so the presence PERSISTS as an inhabitant and shows up on
    /// the live desktop. A walk-in with no `Propose` never reaches here (the
    /// `arrive` handler skips it) and, if it did, is refused fail-closed below —
    /// admission stays free, embodiment requires a scope.
    ///
    /// This is the ONLY write the adapter still performs, and it is not a
    /// general one: it takes no caller-supplied intent, so nothing a caller says
    /// can steer what is written. It is a fail-closed `Propose` gate, a generic
    /// four-verb write through `translate_mutation_proposal` (a mutation compiles
    /// to exactly one kernel verb, a type-level guarantee), the canonical
    /// predicate `PART_OF` (already canonical → 0 new predicate symbols), and
    /// independent-readback evidence (a fresh store replay, never the committing
    /// snapshot's own word).
    ///
    /// The node is typed `actor` and carries a top-level `canonical_id` under
    /// `actor:l1:` — the exact convention the perception layer reads to recognise
    /// an L1 inhabitant — plus `provenance: "built"` and the session's origin. It
    /// is attached to the **orientation beacon** (Balise Zéro) via a canonical
    /// `PART_OF` edge — never to any inhabitants-registry node. The beacon anchor
    /// is resolved at runtime by its canonical IDENTITY
    /// (`space:l2:lumina-prime:orientation-beacon-v0`, env-overridable) read from
    /// store DATA — never a baked hex key; if nothing carries that identity, the
    /// actor node is written anyway and the dropped edge is reported (never
    /// dangled).
    ///
    /// The actor key is deterministic from the session id (an fnv-based `kbase`
    /// in the `0x0AC0` prefix block), so re-arriving with the same session id is
    /// idempotent — `AlreadyCommitted`, no duplicate inhabitant.
    pub fn materialize_actor(
        &mut self,
        session: &ActorSession,
    ) -> Result<EmbodimentOutcome, UniverseError> {
        // Fail-closed authority gate. Being present is free; embodying a durable
        // inhabitant is a write, and a write with no `Propose` is refused before
        // any store handle is opened.
        if !session.has(Capability::Propose) {
            return Err(UniverseError::Validation(format!(
                "authority denied: session '{}' (origin '{}', status {:?}) does not hold the \
                 Propose (write) capability; durable embodiment requires a sponsor's Capability \
                 Bond. Nothing committed (fail-closed).",
                session.session_id, session.origin, session.status,
            )));
        }

        let World::Mounted {
            supervisor,
            store_root,
        } = self
        else {
            return Err(UniverseError::Validation("no Universe mounted".into()));
        };

        // A second store handle over the same root — the translator
        // content-addresses through a `UniverseStore`, same on-disk segment.
        let store = UniverseStore::open(&*store_root)?;
        let base = supervisor.revision();

        // The orientation beacon (Balise Zéro), resolved at runtime by its
        // canonical IDENTITY from store DATA — never an opaque hex key. The scan
        // reads each candidate's content lazily and short-circuits on the first
        // `canonical_id` match, so the hot path (beacon present) never hydrates
        // the whole store. If nothing carries that identity the actor is still
        // written and the `PART_OF` edge is reported dropped — an inhabitant with
        // no beacon is honest, a dangling edge is not.
        let anchor_canonical_id = actor_anchor_canonical_id();
        let beacon_key = resolve_entity_by_canonical_id(
            supervisor.snapshot(),
            &anchor_canonical_id,
            &|content| supervisor.read_content(content).ok(),
        );

        // The Toolkit Shelf's access capability, resolved the same way and for
        // the same reason. Granting it is how an inhabitant comes into the world
        // already holding every toolkit the city has shelved — as reach, not as
        // matter. Absent from this store, the grant is dropped and reported.
        let toolkit_access_canonical_id = toolkit_access_canonical_id();
        let toolkit_access_key = resolve_entity_by_canonical_id(
            supervisor.snapshot(),
            &toolkit_access_canonical_id,
            &|content| supervisor.read_content(content).ok(),
        );

        // `PART_OF` is already a canonical predicate → the remap is the identity
        // and no new predicate symbol is minted. The remap table is the SHARED
        // one (`universe_e2e::canonical`), not an adapter-local copy: the
        // authored-name -> canonical-predicate mapping is Universe vocabulary,
        // and a transport that kept its own would be a second authority on it.
        let (part_of_pred, part_of_swap) = canonical_predicate("PART_OF")
            .ok_or_else(|| UniverseError::Validation("PART_OF has no canonical mapping".into()))?;

        // The `actor` type symbol should already exist in the store; plan its
        // interning so a MISSING symbol is interned and REPORTED, never assumed.
        // `USED` rides alongside it: already canonical, planned the same way, so
        // a store missing it interns it and SAYS so rather than assuming.
        let requested: Vec<String> = ["actor", part_of_pred, GRANT_PREDICATE]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let plan = supervisor.snapshot().plan_symbol_interning(&requested)?;
        let interned_symbols = plan.additions.clone();
        let sym = |name: &str| -> Result<u32, UniverseError> {
            plan.assignments
                .get(name)
                .copied()
                .ok_or_else(|| UniverseError::Validation(format!("symbol {name} not planned")))
        };

        // Deterministic key from the session id: an fnv-based `kbase` in the
        // `0x0AC0` prefix block. Re-arriving with the same session id lands the
        // same key + idempotency key → AlreadyCommitted.
        let idempotency_key = actor_idempotency_key(&session.session_id);
        let kbase = actor_kbase(&session.session_id);
        let actor = EntityKey(kbase);
        let rel_part_of = RelationKey(kbase | 1);
        // The grant edge shares the body's deterministic key block, so
        // re-arriving lands the same edge and commits nothing new.
        let rel_grant = RelationKey(kbase | 2);

        // `actor:l1:` is the identity convention the perception layer keys on. The
        // `claude-` namespace prefix is applied HERE; a redundant leading
        // `claude:`/`claude-` is stripped from the session FIRST, so a
        // `claude:<uuid>` session yields `actor:l1:mind:claude-<uuid>` with a
        // SINGLE `claude-`, not a doubled `claude-claude-`. The remaining id is
        // sanitised so the local segment carries no stray separators. The entity
        // KEY still derives from the raw session id (below), so this string change
        // does not disturb idempotency.
        let canonical_id = format!(
            "actor:l1:mind:claude-{}",
            sanitize_session_id(strip_leading_claude(&session.session_id))
        );

        // The runtime proposal: the actor node's content, drawn by field name and
        // content-addressed by the translator. `canonical_id` is TOP-LEVEL so the
        // perception layer reads it directly.
        let proposal = json!({
            "actor_content": {
                "canonical_id": canonical_id,
                "node_type": "actor",
                "subtype": "actor",
                "provenance": "built",
                "origin": session.origin,
                "kind": "actor",
                "embodied_session": session.session_id,
                "sponsor": session.sponsor,
                "status": format!("{:?}", session.status),
                "base_revision": base.0,
                // The passport's own lifetime, carried ONTO the body. Without it
                // the node records no lifetime at all, and a later reaper reading
                // this node can only answer `unknown` — missing data is not zero,
                // so `--expired` could never judge a single body. The body is
                // durable; this is not a self-destruct, it is the arrival fact
                // that makes the lifetime READABLE from the node itself.
                "expires_at": session.expires_at,
                "note": "MCP arrive: a sponsored visitor embodied as a durable L1 inhabitant, \
attached to the orientation beacon (Balise Zéro) via PART_OF",
            },
        });

        // Every mutation is a `MutationPlan`; each compiles to exactly one kernel
        // verb through the generic translator. A fifth verb is unrepresentable.
        let mut plans = Vec::new();
        if !plan.additions.is_empty() {
            plans.push(MutationPlan::InternSymbols {
                symbols: plan.additions.clone(),
            });
        }
        plans.push(MutationPlan::PutEntity {
            key: actor,
            generation: 0,
            symbol: sym("actor")?,
            content_field: Some("actor_content".into()),
        });
        if let Some(beacon_key) = beacon_key {
            // The inhabitant is PART_OF the beacon's orientation space; the
            // predicate is unswapped, so source -> target reads actor -> beacon.
            let (p_src, p_tgt) = if part_of_swap {
                (beacon_key, actor)
            } else {
                (actor, beacon_key)
            };
            plans.push(MutationPlan::PutRelation {
                key: rel_part_of,
                generation: 0,
                source: p_src,
                target: p_tgt,
                predicate: sym(part_of_pred)?,
                content_field: None,
            });
        }
        if let Some(access_key) = toolkit_access_key {
            // The one grant: actor -> the capability that opens the Toolkit
            // Shelf. Unswapped, because that is the direction a held-capability
            // read walks (source must be the actor, or the grant is somebody
            // else's). It hands over REACH, not matter — nothing of any
            // toolkit's content is copied onto this body, so a blueprint revised
            // tomorrow is held revised without touching this inhabitant again.
            plans.push(MutationPlan::PutRelation {
                key: rel_grant,
                generation: 0,
                source: actor,
                target: access_key,
                predicate: sym(GRANT_PREDICATE)?,
                content_field: None,
            });
        }

        // Compile each plan through the translator, gather into ONE atomic write
        // set, and commit at the next tick boundary. Attribution rides in the
        // content (`embodied_session`) and the idempotency key, both of which
        // name the session.
        let mut commands: Vec<UniverseCommand> = Vec::with_capacity(plans.len());
        for mp in &plans {
            let ws = translate_mutation_proposal(
                mp,
                &proposal,
                &store,
                base,
                idempotency_key.clone(),
            )?;
            commands.extend(ws.commands);
        }

        let write_set = UniverseWriteSet {
            base_revision: base,
            idempotency_key,
            commands,
        };
        // A prepare rejection commits nothing; prior state is intact.
        let transaction = UniverseTransaction::prepare(supervisor.snapshot(), write_set)?;
        supervisor.enqueue(transaction);
        let receipts = supervisor.advance(&mut NoopHook)?;
        let idempotent = matches!(receipts.first(), Some(CommitReceipt::AlreadyCommitted { .. }));

        // Independent readback: a fresh reopen, never the committing snapshot.
        let fresh = supervisor.independent_readback()?;
        let actor_present = fresh.entities.iter().any(|e| e.key == actor);
        let edge_present =
            beacon_key.is_some() && fresh.relations.iter().any(|r| r.key == rel_part_of);
        let dropped_edge = beacon_key.is_none();
        let grant_present =
            toolkit_access_key.is_some() && fresh.relations.iter().any(|r| r.key == rel_grant);
        let grant_dropped = toolkit_access_key.is_none();

        // What the grant REACHES, walked from the committed edges of the fresh
        // snapshot: capability --APPLIES_IN--> shelf <--PART_OF-- toolkit. This
        // is read at the post-commit revision, so the receipt reports the shelf
        // as it stands NOW rather than as it stood when this body was authored —
        // which is the whole point of handing over a bearing instead of a copy.
        let (shelf_canonical_id, toolkits_reached) = match toolkit_access_key {
            Some(access_key) if grant_present => read_shelf_through_grant(
                &fresh,
                access_key,
                &|content| supervisor.read_content(content).ok(),
            ),
            _ => (None, Vec::new()),
        };

        // On an idempotent re-run nothing NEW is committed this call; the node is
        // already present (the readback still proves it).
        let mut committed_effects = Vec::new();
        if !idempotent {
            committed_effects.push(json!({
                "put_entity": actor.to_string(),
                "canonical_id": canonical_id,
                "node_type": "actor",
            }));
            if let Some(beacon_key) = beacon_key {
                committed_effects.push(json!({
                    "put_relation": rel_part_of.to_string(),
                    "predicate": part_of_pred,
                    "target": beacon_key.to_string(),
                }));
            }
            if let Some(access_key) = toolkit_access_key {
                committed_effects.push(json!({
                    "put_relation": rel_grant.to_string(),
                    "predicate": GRANT_PREDICATE,
                    "target": access_key.to_string(),
                    "grants": toolkit_access_canonical_id.clone(),
                }));
            }
        }

        Ok(EmbodimentOutcome {
            actor_key: actor.to_string(),
            canonical_id,
            from_revision: base.0,
            to_revision: fresh.revision.0,
            idempotent,
            actor_present,
            edge_present,
            dropped_edge,
            beacon_key: beacon_key.map(|k| k.to_string()),
            toolkit_access_canonical_id,
            toolkit_access_key: toolkit_access_key.map(|k| k.to_string()),
            grant_present,
            grant_dropped,
            shelf_canonical_id,
            toolkits_reached,
            interned_symbols,
            committed_effects,
            evidence: vec![json!({
                "independent_readback": {
                    "revision": fresh.revision.0,
                    "actor_present": actor_present,
                    "part_of_beacon_present": edge_present,
                    "toolkit_grant_present": grant_present,
                }
            })],
        })
    }
}

/// The measured outcome of embodying a sponsored session as a durable L1
/// inhabitant — the actor node, its beacon edge, and independent readback
/// evidence. Folded into the `arrive` receipt's `embodiment` block.
#[derive(Debug)]
pub struct EmbodimentOutcome {
    /// The 32-hex EntityKey of the written `actor` node (deterministic from the
    /// session id) — the stable target a later wake hook can address.
    pub actor_key: String,
    pub canonical_id: String,
    pub from_revision: u64,
    pub to_revision: u64,
    /// True when this call committed nothing new (the inhabitant already existed).
    pub idempotent: bool,
    /// Independent-readback: the actor node survives a fresh store reopen.
    pub actor_present: bool,
    /// Independent-readback: the `PART_OF` edge to the beacon survives a reopen.
    pub edge_present: bool,
    /// The beacon did not resolve, so the edge was dropped (never dangled).
    pub dropped_edge: bool,
    /// The resolved orientation-beacon key, or `None` when it did not resolve.
    pub beacon_key: Option<String>,
    /// The canonical identity the toolkit grant was resolved by (reported even
    /// when it resolved to nothing, so a reader can see WHAT was looked for).
    pub toolkit_access_canonical_id: String,
    /// The resolved access-capability key, or `None` when this store carries no
    /// Toolkit Shelf.
    pub toolkit_access_key: Option<String>,
    /// Independent-readback: the `USED` grant edge survives a fresh reopen.
    pub grant_present: bool,
    /// The access capability did not resolve, so the grant was dropped (the body
    /// is still written; no edge is ever dangled).
    pub grant_dropped: bool,
    /// The shelf reached THROUGH the grant, by canonical identity.
    pub shelf_canonical_id: Option<String>,
    /// The toolkits standing on that shelf at the post-commit revision — what
    /// this inhabitant can reach right now. Empty with a present grant means the
    /// shelf is bare, which is a measured fact; empty with a DROPPED grant means
    /// `unknown`, and the two are distinguished by `grant_dropped`.
    pub toolkits_reached: Vec<String>,
    /// Symbols interned this call — expected empty against the canonical store.
    pub interned_symbols: Vec<String>,
    pub committed_effects: Vec<serde_json::Value>,
    pub evidence: Vec<serde_json::Value>,
}

impl EmbodimentOutcome {
    /// The `embodiment` block folded into the `arrive` receipt.
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "materialized": true,
            "actor_key": self.actor_key,
            "canonical_id": self.canonical_id,
            "idempotent": self.idempotent,
            "node_written": !self.idempotent,
            "actor_present": self.actor_present,
            "revision": { "from": self.from_revision, "to": self.to_revision },
            "part_of_beacon": {
                "beacon_key": self.beacon_key,
                "edge_present": self.edge_present,
                "dropped": self.dropped_edge,
            },
            "toolkits": {
                "held_as": "bearing",
                "granted_capability": self.toolkit_access_canonical_id,
                "capability_key": self.toolkit_access_key,
                "grant_edge_present": self.grant_present,
                "dropped": self.grant_dropped,
                "shelf": self.shelf_canonical_id,
                "reachable_now": self.toolkits_reached,
                "reachable_count": self.toolkits_reached.len(),
                "epistemic": if self.grant_dropped {
                    "unknown — this store carries no Toolkit Shelf access capability, so nothing \
is claimed about what this inhabitant can reach"
                } else {
                    "measured — walked from committed edges in a fresh store replay, at the \
post-commit revision"
                },
                "note": "a BEARING, not a copy: no toolkit content is duplicated onto this body. \
The list is what the shelf holds RIGHT NOW; a blueprint revised later is held revised, and a \
toolkit shelved later is held too, with no further write to this inhabitant.",
                "authority": "reach, not authority — this capability is class `observe`. Wielding \
a toolkit is adjudicated separately and fails closed at the sealed port.",
            },
            "interned_symbols": self.interned_symbols,
            "committed_effects": self.committed_effects,
            "evidence": self.evidence,
            "note": if self.idempotent {
                "already an inhabitant: re-arriving with the same session id committed nothing new \
(AlreadyCommitted)"
            } else {
                "embodied as a durable L1 inhabitant (actor:l1:...), attached to the orientation \
beacon via PART_OF; independently read back from a fresh store replay"
            },
        })
    }
}

/// A commit needs no tick-phase side effects here.
struct NoopHook;
impl PhaseHook for NoopHook {
    fn run(&mut self, _phase: TickPhase, _snapshot: &UniverseSnapshot) -> Result<(), UniverseError> {
        Ok(())
    }
}

/// The canonical IDENTITY of the actor anchor — the orientation beacon
/// ("Balise Zéro", the civic origin at (0,0,0), written by the
/// `inject_orientation_beacon` bin). A new inhabitant is attached HERE (via
/// canonical `PART_OF`) so it belongs to the city's orientation space, NOT to any
/// inhabitants-registry node.
///
/// This is a SEMANTIC identity resolved from store data, NOT an opaque packed
/// key: a named landmark is a legitimate anchor (an identity the world authors),
/// where a baked hex `EntityKey` is a forbidden "privileged" value (CLAUDE.md:
/// "Where is a projection, not a datum"; "no anonymous construction"). The
/// resolver ([`resolve_entity_by_canonical_id`]) matches the entity whose content
/// `canonical_id` equals this string. `MIND_ACTOR_ANCHOR_CANONICAL_ID` overrides
/// it for a store that anchors inhabitants under a different identity.
const DEFAULT_ACTOR_ANCHOR_CANONICAL_ID: &str = "space:l2:lumina-prime:orientation-beacon-v0";
const ACTOR_ANCHOR_CANONICAL_ID_ENV: &str = "MIND_ACTOR_ANCHOR_CANONICAL_ID";

/// The canonical IDENTITY of the capability that opens the Toolkit Shelf
/// (`space:l2:mind-universe:toolkit-shelf-v0`, bound by the `shelve_toolkits`
/// bin). An arriving inhabitant is granted THIS one capability, and through it
/// reaches every toolkit standing on the shelf.
///
/// What the inhabitant receives is a BEARING, not a copy: the grant edge names
/// the capability, the capability's scope names the shelf, and the shelf's
/// members name the toolkit definitions themselves. Every read therefore
/// resolves at the reader's current revision — a revised blueprint is held
/// revised, and a toolkit shelved after this inhabitant arrived is held from
/// that revision on, with no write ever touching the inhabitant again.
///
/// Resolved from store DATA by identity, exactly like the beacon anchor. If it
/// does not resolve, the grant is DROPPED and reported: the body is still
/// written, and no edge is ever dangled.
const DEFAULT_TOOLKIT_ACCESS_CANONICAL_ID: &str = "capability:l2:mind-universe:toolkit-shelf-access";
const TOOLKIT_ACCESS_CANONICAL_ID_ENV: &str = "MIND_TOOLKIT_ACCESS_CANONICAL_ID";

/// The canonical predicate the grant rides. Already canonical — it is the
/// predicate `underground-maintenance-grant.json`'s authored `HOLDS_CAPABILITY`
/// is STORED as — so it needs no remap and is deliberately not run through the
/// authored-name table (which fails closed on `HOLDS_CAPABILITY` on purpose, so
/// no alias is invented here either). Direction is unswapped: actor -> capability,
/// which is the direction `universe_query::read_actor_capability_set` reads.
const GRANT_PREDICATE: &str = "USED";
/// The predicate binding the access capability to the shelf it opens.
const SCOPE_PREDICATE: &str = "APPLIES_IN";
/// The predicate binding a toolkit onto the shelf.
const MEMBERSHIP_PREDICATE: &str = "PART_OF";
/// Bounded relation budget for the through-the-grant walk. The enumeration is a
/// courtesy read folded into a receipt, never a hot path; it stops at this many
/// inspected relations and says so rather than scanning without limit.
const SHELF_WALK_BUDGET: usize = 8192;

/// The canonical id the actor anchor is resolved by: `MIND_ACTOR_ANCHOR_CANONICAL_ID`
/// when set and non-empty, else the orientation beacon's canonical id.
fn actor_anchor_canonical_id() -> String {
    env::var(ACTOR_ANCHOR_CANONICAL_ID_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_ACTOR_ANCHOR_CANONICAL_ID.to_owned())
}

/// Resolves an entity by its canonical IDENTITY — the node whose content
/// `canonical_id` field equals `wanted`. This is how the actor anchor (the
/// orientation beacon) is found from store DATA rather than by an opaque packed
/// key, honouring "identity, not a baked handle".
///
/// The scan is bounded: content is read lazily, one candidate at a time (the
/// whole store is never hydrated at once), entities carrying no content are
/// skipped, and the walk SHORT-CIRCUITS on the first match — so the common case
/// (the anchor is present) stops as soon as it is found. A miss returns `None`,
/// and the caller drops the edge rather than dangling it.
fn resolve_entity_by_canonical_id(
    snapshot: &UniverseSnapshot,
    wanted: &str,
    read_content: &dyn Fn(&ContentRef) -> Option<Value>,
) -> Option<EntityKey> {
    snapshot.entities.iter().find_map(|entity| {
        let content = entity.content.as_ref()?;
        let value = read_content(content)?;
        let id = value.get("canonical_id").and_then(Value::as_str)?;
        (id == wanted).then_some(entity.key)
    })
}

/// The canonical id the Toolkit Shelf's access capability is resolved by:
/// `MIND_TOOLKIT_ACCESS_CANONICAL_ID` when set and non-empty, else the shelf's
/// own capability identity.
fn toolkit_access_canonical_id() -> String {
    env::var(TOOLKIT_ACCESS_CANONICAL_ID_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_TOOLKIT_ACCESS_CANONICAL_ID.to_owned())
}

/// Walks what a held grant REACHES: `capability --APPLIES_IN--> shelf` and then
/// the shelf's incoming `PART_OF` members, returning the shelf's canonical
/// identity and each member's, sorted.
///
/// This is the read that makes a bearing worth having. It resolves against the
/// snapshot it is handed — so a caller passing a FRESH post-commit replay learns
/// what the inhabitant can reach *now*, not what was true when the body was
/// authored. Nothing is copied and nothing is cached: ask again later, get the
/// later answer.
///
/// Honest by construction: an unknown predicate symbol, an unresolvable member
/// or a shelf with no members yields fewer names, never an invented one. The
/// walk is bounded by [`SHELF_WALK_BUDGET`] inspected relations; a store large
/// enough to exhaust it returns what it found, and the caller reports a count
/// rather than a claim of completeness.
fn read_shelf_through_grant(
    snapshot: &UniverseSnapshot,
    capability: EntityKey,
    read_content: &dyn Fn(&ContentRef) -> Option<Value>,
) -> (Option<String>, Vec<String>) {
    let (Some(scope), Some(membership)) = (
        snapshot.symbol_id(SCOPE_PREDICATE),
        snapshot.symbol_id(MEMBERSHIP_PREDICATE),
    ) else {
        return (None, Vec::new());
    };
    let identity_of = |key: EntityKey| -> Option<String> {
        let entity = snapshot.entities.iter().find(|e| e.key == key)?;
        let value = read_content(entity.content.as_ref()?)?;
        value
            .get("canonical_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
    };

    // Hop 1: the shelf this capability opens.
    let Some(shelf) = snapshot
        .relations
        .iter()
        .take(SHELF_WALK_BUDGET)
        .find(|r| r.source == capability && r.predicate == scope)
        .map(|r| r.target)
    else {
        return (None, Vec::new());
    };

    // Hop 2: everything standing on it. Traversed from committed edges — never
    // read off the shelf node's authored membership list, which states an
    // intention and not a fact.
    let mut toolkits: Vec<String> = snapshot
        .relations
        .iter()
        .take(SHELF_WALK_BUDGET)
        .filter(|r| r.target == shelf && r.predicate == membership)
        .filter_map(|r| identity_of(r.source))
        .collect();
    toolkits.sort();
    toolkits.dedup();
    (identity_of(shelf), toolkits)
}

/// Strips a single leading `claude:` or `claude-` marker from a session id, so the
/// `claude-` actor namespace prefix is not doubled when a session already begins
/// with `claude` (e.g. `claude:<uuid>` → `<uuid>`, which then prefixes to a single
/// `claude-<uuid>`). Deterministic and idempotent for the same session; a session
/// that does not begin with a `claude` marker is returned unchanged.
fn strip_leading_claude(session_id: &str) -> &str {
    for marker in ["claude:", "claude-"] {
        if let Some(rest) = session_id.strip_prefix(marker) {
            return rest;
        }
    }
    session_id
}

/// Sanitises a session id into the local segment of an `actor:l1:` canonical id:
/// every non-alphanumeric byte (colons, slashes, spaces) becomes `-`, so the
/// resulting id is deterministic and carries no stray separators.
fn sanitize_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The idempotency key an embodiment commits under — one string, derived from
/// the session id alone, so re-arriving lands `AlreadyCommitted` instead of a
/// duplicate inhabitant.
fn actor_idempotency_key(session_id: &str) -> String {
    format!("mcp:arrive:embody:{session_id}")
}

/// The deterministic key block a session's body occupies: an fnv-based base in
/// the `0x0AC0` prefix block, with the low byte left free for the relations that
/// hang off it (`kbase | 1` is the `PART_OF` edge).
///
/// This is the SINGLE derivation shared by the write ([`World::materialize_actor`])
/// and the read ([`World::recall_body`]). A body is recalled at exactly the key it
/// was written to, by construction rather than by a matching convention that two
/// sites could drift apart on.
fn actor_kbase(session_id: &str) -> u128 {
    let seed = fnv64(&actor_idempotency_key(session_id));
    (0x0AC0u128 << 96) | ((seed as u128) << 8)
}

/// The EntityKey of the durable body belonging to `session_id` — whether or not
/// anything has been written there yet. Existence is the snapshot's to answer.
fn actor_key_for_session(session_id: &str) -> EntityKey {
    EntityKey(actor_kbase(session_id))
}

/// Deterministic FNV-1a — for stable, collision-resistant ids (no RNG/clock).
fn fnv64(value: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{admit, AdmissionRequest, Continuity, SessionRegistry};
    // Only the tests hand-build a record: the seeded beacon stand-in below is
    // written straight through the kernel, not through the translator.
    use universe_store::{EntityRecord, RelationRecord};

    /// The repository Genesis fixture — a minimal, valid, signed snapshot.
    fn genesis_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/genesis/minimal-genesis.json")
    }

    /// A real mounted Universe over a throwaway store seeded from Genesis.
    fn mounted() -> (tempfile::TempDir, World) {
        let dir = tempfile::tempdir().expect("temp store dir");
        let world = World::mount(dir.path().to_path_buf(), genesis_path())
            .expect("mount minimal Genesis into a fresh store");
        (dir, world)
    }

    /// A session that HOLDS `Propose` — a sponsor mints the write capability.
    fn builder(session_id: &str) -> ActorSession {
        let req = AdmissionRequest {
            sponsor: Some("NLR".into()),
            ..Default::default()
        };
        let session = admit(&req, session_id.to_owned(), 0, 100, None).0;
        assert!(session.has(Capability::Propose), "builder must be able to write");
        session
    }

    /// An unauthenticated walk-in — observe + speak only, never `Propose`.
    fn walk_in(session_id: &str) -> ActorSession {
        let session = admit(&AdmissionRequest::default(), session_id.to_owned(), 0, 100, None).0;
        assert!(!session.has(Capability::Propose), "a walk-in cannot write");
        session
    }

    /// Seeds a bare orientation-beacon stand-in carrying the anchor's canonical
    /// IDENTITY (`space:l2:lumina-prime:orientation-beacon-v0`) so
    /// `materialize_actor` can attach the inhabitant to it. The minimal Genesis
    /// carries no beacon; this places it via the same commit path.
    ///
    /// The key is deliberately NOT the old magic `0xB000` — it is an arbitrary
    /// `0x5EED` — precisely to prove the resolver finds the anchor by its
    /// canonical_id from content DATA, never by a baked hex EntityKey.
    fn seed_beacon(world: &mut World) {
        let World::Mounted { supervisor, .. } = world else {
            unreachable!("mounted above");
        };
        let base = supervisor.revision();
        let content = supervisor
            .append_content(&json!({
                "canonical_id": "space:l2:lumina-prime:orientation-beacon-v0",
                "kind": "beacon",
            }))
            .expect("append beacon content");
        let write_set = UniverseWriteSet {
            base_revision: base,
            idempotency_key: "test:seed-beacon".into(),
            commands: vec![UniverseCommand::PutEntity {
                entity: EntityRecord {
                    // Deliberately NOT 0xB000: the resolver keys on canonical_id
                    // (content DATA), not this hex, so any key works.
                    key: EntityKey(0x5EED),
                    generation: 0,
                    // Any valid symbol works — resolve_key only checks existence.
                    // 1 == `Space` in the minimal Genesis symbol table.
                    symbol: 1,
                    content: Some(content),
                },
            }],
        };
        let tx =
            UniverseTransaction::prepare(supervisor.snapshot(), write_set).expect("prepare beacon");
        supervisor.enqueue(tx);
        supervisor.advance(&mut NoopHook).expect("commit beacon");
    }

    /// The round trip that makes a body persistent in more than shape: embody a
    /// sponsored session, then recall it from the graph alone — no registry, no
    /// memory of the arrival, exactly what a restarted process has.
    #[test]
    fn a_body_written_by_arrive_is_recalled_from_the_graph_alone() {
        let (_dir, mut world) = mounted();
        seed_beacon(&mut world);
        let session = builder("claude:persist-1");

        assert!(
            world.recall_body("claude:persist-1").is_none(),
            "nothing is recalled before a body exists — unknown, never a fabricated standing"
        );

        world
            .materialize_actor(&session)
            .expect("a sponsored embody succeeds");

        let recalled = world
            .recall_body("claude:persist-1")
            .expect("the durable body is found at its own deterministic key");
        assert_eq!(
            recalled.sponsor.as_deref(),
            Some("NLR"),
            "the sponsor is read off the body, not re-declared by the caller"
        );
        assert_eq!(recalled.origin.as_deref(), Some("unknown-external"));
        assert_eq!(recalled.recorded_status.as_deref(), Some("SponsoredVisitor"));
        assert!(recalled.base_revision.is_some(), "the body says when it was built");

        // The standing a brand-new process derives from that body alone.
        let restored = SessionRegistry::default().get_or_walk_in(
            "claude:persist-1",
            session.expires_at + 1_000_000, // long past the original admission window
            Some(&recalled),
        );
        assert!(
            restored.has(Capability::Propose),
            "the returning inhabitant is not demoted to a stranger"
        );
        assert_eq!(restored.continuity, Continuity::Persistent);
    }

    /// Fail-closed: the key block is the body's address, not its identity. A node
    /// sitting at another session's key is never handed over as this presence.
    #[test]
    fn a_body_naming_another_session_is_never_recalled() {
        let (_dir, mut world) = mounted();
        seed_beacon(&mut world);
        world
            .materialize_actor(&builder("claude:persist-1"))
            .expect("a sponsored embody succeeds");

        assert!(
            world.recall_body("claude:someone-else").is_none(),
            "a different session recalls nothing"
        );
    }

    /// Seeds a Toolkit Shelf stand-in: the access capability (carrying a
    /// TOP-LEVEL `capability` string, the shape a held-capability read
    /// resolves), the shelf Space, one node per `toolkit_ids`, the
    /// `APPLIES_IN` scope edge and one `PART_OF` membership edge each.
    ///
    /// Returns the toolkit keys in the order given, so a test can revise a
    /// blueprint in place and watch what an untouched inhabitant then reaches.
    /// Like `seed_beacon`, the keys are arbitrary: everything here is resolved
    /// by canonical identity from content DATA.
    fn seed_toolkit_shelf(world: &mut World, toolkit_ids: &[&str]) -> Vec<EntityKey> {
        let World::Mounted { supervisor, .. } = world else {
            unreachable!("mounted above");
        };
        let base = supervisor.revision();
        let plan = supervisor
            .snapshot()
            .plan_symbol_interning(&[
                "thing".to_owned(),
                MEMBERSHIP_PREDICATE.to_owned(),
                SCOPE_PREDICATE.to_owned(),
            ])
            .expect("plan shelf symbols");
        let sym = |name: &str| plan.assignments[name];

        let capability_key = EntityKey(0x5_A0_00);
        let shelf_key = EntityKey(0x5_A0_01);
        let mut commands = Vec::new();
        if !plan.additions.is_empty() {
            commands.push(UniverseCommand::InternSymbols {
                symbols: plan.additions.clone(),
            });
        }
        commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: capability_key,
                generation: 0,
                symbol: sym("thing"),
                content: Some(
                    supervisor
                        .append_content(&json!({
                            "canonical_id": DEFAULT_TOOLKIT_ACCESS_CANONICAL_ID,
                            "kind": "capability",
                            // TOP-LEVEL, deliberately: this is the field a
                            // bounded held-capability read resolves.
                            "capability": "observe:toolkit-shelf",
                            "class": "observe",
                        }))
                        .expect("append capability content"),
                ),
            },
        });
        commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: shelf_key,
                generation: 0,
                symbol: 1,
                content: Some(
                    supervisor
                        .append_content(&json!({
                            "canonical_id": "space:l2:mind-universe:toolkit-shelf-v0",
                            "kind": "toolkit_shelf",
                        }))
                        .expect("append shelf content"),
                ),
            },
        });
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: RelationKey(0x5_B0_00),
                generation: 0,
                source: capability_key,
                target: shelf_key,
                predicate: sym(SCOPE_PREDICATE),
                content: None,
            },
        });

        let mut toolkit_keys = Vec::new();
        for (index, id) in toolkit_ids.iter().enumerate() {
            let key = EntityKey(0x5_A1_00 + index as u128);
            toolkit_keys.push(key);
            commands.push(UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key,
                    generation: 0,
                    symbol: 1,
                    content: Some(
                        supervisor
                            .append_content(&json!({
                                "canonical_id": id,
                                "kind": "toolkit",
                                "blueprint_revision": "v0",
                            }))
                            .expect("append toolkit content"),
                    ),
                },
            });
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(0x5_B1_00 + index as u128),
                    generation: 0,
                    source: key,
                    target: shelf_key,
                    predicate: sym(MEMBERSHIP_PREDICATE),
                    content: None,
                },
            });
        }

        let write_set = UniverseWriteSet {
            base_revision: base,
            idempotency_key: format!("test:seed-toolkit-shelf:{}", toolkit_ids.len()),
            commands,
        };
        let tx =
            UniverseTransaction::prepare(supervisor.snapshot(), write_set).expect("prepare shelf");
        supervisor.enqueue(tx);
        supervisor.advance(&mut NoopHook).expect("commit shelf");
        toolkit_keys
    }

    /// Re-puts a toolkit node at the SAME key with revised content — a blueprint
    /// revised in place, the way `SupersedeEntity` revises a construct.
    fn revise_blueprint(world: &mut World, key: EntityKey, canonical_id: &str, revision: &str) {
        let World::Mounted { supervisor, .. } = world else {
            unreachable!("mounted above");
        };
        let base = supervisor.revision();
        let content = supervisor
            .append_content(&json!({
                "canonical_id": canonical_id,
                "kind": "toolkit",
                "blueprint_revision": revision,
            }))
            .expect("append revised content");
        let write_set = UniverseWriteSet {
            base_revision: base,
            idempotency_key: format!("test:revise-blueprint:{canonical_id}:{revision}"),
            commands: vec![UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key,
                    generation: 0,
                    symbol: 1,
                    content: Some(content),
                },
            }],
        };
        let tx =
            UniverseTransaction::prepare(supervisor.snapshot(), write_set).expect("prepare revise");
        supervisor.enqueue(tx);
        supervisor.advance(&mut NoopHook).expect("commit revise");
    }

    /// Shelves one more toolkit AFTER inhabitants have already arrived — the
    /// civic act that must reach every one of them without touching any body.
    fn shelve_one_more(world: &mut World, id: &str, index: u128) -> EntityKey {
        let World::Mounted { supervisor, .. } = world else {
            unreachable!("mounted above");
        };
        let base = supervisor.revision();
        let membership = supervisor
            .snapshot()
            .symbol_id(MEMBERSHIP_PREDICATE)
            .expect("PART_OF is interned by seed_toolkit_shelf");
        let key = EntityKey(0x5_A1_00 + index);
        let content = supervisor
            .append_content(&json!({
                "canonical_id": id,
                "kind": "toolkit",
                "blueprint_revision": "v0",
            }))
            .expect("append late toolkit content");
        let write_set = UniverseWriteSet {
            base_revision: base,
            idempotency_key: format!("test:shelve-one-more:{id}"),
            commands: vec![
                UniverseCommand::PutEntity {
                    entity: EntityRecord {
                        key,
                        generation: 0,
                        symbol: 1,
                        content: Some(content),
                    },
                },
                UniverseCommand::PutRelation {
                    relation: RelationRecord {
                        key: RelationKey(0x5_B1_00 + index),
                        generation: 0,
                        source: key,
                        target: EntityKey(0x5_A0_01),
                        predicate: membership,
                        content: None,
                    },
                },
            ],
        };
        let tx =
            UniverseTransaction::prepare(supervisor.snapshot(), write_set).expect("prepare shelve");
        supervisor.enqueue(tx);
        supervisor.advance(&mut NoopHook).expect("commit shelve");
        key
    }

    #[test]
    fn an_arriving_inhabitant_is_handed_every_shelved_toolkit_as_a_bearing() {
        let (_dir, mut world) = mounted();
        seed_beacon(&mut world);
        seed_toolkit_shelf(
            &mut world,
            &[
                "space:l2:mind-universe:underground-toolkit-v0",
                "space:l2:mind-universe:sky-toolkit-v0",
            ],
        );

        let outcome = world
            .materialize_actor(&builder("claude:shelf-1"))
            .expect("a sponsored embody succeeds");

        // THREE committed effects now: the body, its beacon edge, and the ONE
        // grant that carries every toolkit.
        assert_eq!(
            outcome.committed_effects.len(),
            3,
            "body + beacon edge + toolkit grant: {:?}",
            outcome.committed_effects
        );
        assert!(outcome.grant_present, "the USED grant must survive a fresh reopen");
        assert!(!outcome.grant_dropped);
        assert_eq!(
            outcome.shelf_canonical_id.as_deref(),
            Some("space:l2:mind-universe:toolkit-shelf-v0"),
            "the grant must reach the shelf it opens"
        );
        // Walked from committed edges: both toolkits, by identity, sorted.
        assert_eq!(
            outcome.toolkits_reached,
            vec![
                "space:l2:mind-universe:sky-toolkit-v0".to_owned(),
                "space:l2:mind-universe:underground-toolkit-v0".to_owned(),
            ]
        );
        // Nothing of any toolkit was COPIED: exactly one entity was written.
        let entities_written = outcome
            .committed_effects
            .iter()
            .filter(|effect| effect.get("put_entity").is_some())
            .count();
        assert_eq!(
            entities_written, 1,
            "a bearing writes ONE node (the body) and no duplicate of any toolkit"
        );

        let receipt = outcome.to_json();
        assert_eq!(receipt["toolkits"]["held_as"], "bearing");
        assert_eq!(receipt["toolkits"]["reachable_count"], 2);
        assert_eq!(receipt["evidence"][0]["independent_readback"]["toolkit_grant_present"], true);
    }

    #[test]
    fn a_toolkit_shelved_after_arrival_is_held_without_touching_the_inhabitant() {
        // This is the whole point of a bearing over a copy, and of one shelf
        // over N direct edges: the city grows, and everyone already living in it
        // holds the new thing — with no write to any body.
        let (_dir, mut world) = mounted();
        seed_beacon(&mut world);
        seed_toolkit_shelf(&mut world, &["space:l2:mind-universe:underground-toolkit-v0"]);
        let session = builder("claude:shelf-2");

        let first = world.materialize_actor(&session).expect("embody");
        assert_eq!(first.toolkits_reached.len(), 1);

        // A toolkit joins the city AFTER this inhabitant arrived.
        shelve_one_more(&mut world, "space:l2:mind-universe:mechanical-toolkit-v0", 9);

        let second = world.materialize_actor(&session).expect("re-embody");
        assert!(second.idempotent, "the same session commits nothing new");
        assert!(
            second.committed_effects.is_empty(),
            "NO write touched the inhabitant: {:?}",
            second.committed_effects
        );
        assert_eq!(
            second.toolkits_reached,
            vec![
                "space:l2:mind-universe:mechanical-toolkit-v0".to_owned(),
                "space:l2:mind-universe:underground-toolkit-v0".to_owned(),
            ],
            "yet it now reaches the newly shelved toolkit too"
        );
    }

    #[test]
    fn a_revised_blueprint_is_held_revised_with_no_per_actor_write() {
        let (_dir, mut world) = mounted();
        seed_beacon(&mut world);
        let toolkits = seed_toolkit_shelf(
            &mut world,
            &["space:l2:mind-universe:underground-toolkit-v0"],
        );
        let session = builder("claude:shelf-3");
        world.materialize_actor(&session).expect("embody");

        // The blueprint is revised in place — same key, new content.
        revise_blueprint(
            &mut world,
            toolkits[0],
            "space:l2:mind-universe:underground-toolkit-v0",
            "v1",
        );

        let after = world.materialize_actor(&session).expect("re-embody");
        assert!(
            after.committed_effects.is_empty(),
            "a revised blueprint must cost the inhabitant no write"
        );
        // What it reaches is the node itself, so it reaches the REVISION.
        let snapshot = world.snapshot().expect("mounted").clone();
        let entity = snapshot
            .entities
            .iter()
            .find(|e| e.key == toolkits[0])
            .expect("the toolkit node");
        let content = world
            .read_content(entity.content.as_ref().expect("content"))
            .expect("readable");
        assert_eq!(
            content["blueprint_revision"], "v1",
            "reading THROUGH the bearing lands on the revised blueprint, never on a stale copy"
        );
        assert_eq!(
            after.toolkits_reached,
            vec!["space:l2:mind-universe:underground-toolkit-v0".to_owned()],
            "the identity is stable across the revision — the bearing does not break"
        );
    }

    #[test]
    fn a_store_with_no_shelf_drops_the_grant_and_still_writes_the_body() {
        // Honest absence: no shelf means `unknown`, not an empty toolkit list,
        // and never a dangling edge.
        let (_dir, mut world) = mounted();
        seed_beacon(&mut world);

        let outcome = world
            .materialize_actor(&builder("claude:shelf-4"))
            .expect("embody still succeeds without a shelf");

        assert!(outcome.actor_present, "the body is written regardless");
        assert!(outcome.grant_dropped, "the grant is dropped, not dangled");
        assert!(!outcome.grant_present);
        assert!(outcome.shelf_canonical_id.is_none());
        assert!(outcome.toolkits_reached.is_empty());
        assert_eq!(
            outcome.committed_effects.len(),
            2,
            "body + beacon edge only"
        );
        let receipt = outcome.to_json();
        assert!(
            receipt["toolkits"]["epistemic"]
                .as_str()
                .expect("epistemic string")
                .starts_with("unknown"),
            "an absent shelf must read as unknown, never as 'holds nothing'"
        );
    }

    #[test]
    fn materialize_actor_embodies_a_sponsored_session_with_a_beacon_edge_and_is_idempotent() {
        let (_dir, mut world) = mounted();
        seed_beacon(&mut world);
        let before = world.snapshot().unwrap().revision.0;
        let session = builder("claude:embody-1");

        let first = world
            .materialize_actor(&session)
            .expect("a sponsored embody succeeds");

        // A REAL revision advance — proven by a fresh store replay, not the
        // committing snapshot's own word.
        assert_eq!(first.from_revision, before);
        assert!(
            first.to_revision > before,
            "revision must advance: {} -> {}",
            first.from_revision,
            first.to_revision
        );
        assert!(!first.idempotent);

        // The node carries an `actor:l1:` canonical id — how the perception layer
        // recognises an L1 inhabitant — with a SINGLE `claude-` prefix: the
        // `claude:` session marker is stripped before the `claude-` namespace, so
        // `claude:embody-1` yields `claude-embody-1`, never `claude-claude-...`.
        assert_eq!(
            first.canonical_id, "actor:l1:mind:claude-embody-1",
            "canonical id must carry a single claude- prefix: {}",
            first.canonical_id
        );

        // Independent readback: BOTH the actor node and its PART_OF beacon edge
        // survive a fresh reopen.
        assert!(first.actor_present);
        assert!(
            first.edge_present,
            "the PART_OF edge to the beacon must be present"
        );
        assert!(!first.dropped_edge);
        assert_eq!(
            first.evidence[0]["independent_readback"]["actor_present"],
            true
        );
        assert_eq!(
            first.evidence[0]["independent_readback"]["part_of_beacon_present"],
            true
        );
        // Two committed effects this call: the actor node + its beacon edge.
        assert_eq!(first.committed_effects.len(), 2);

        // Idempotent: re-arriving with the SAME session id commits nothing new,
        // targets the SAME deterministic key, and leaves the node in place.
        let second = world
            .materialize_actor(&session)
            .expect("re-embody succeeds");
        assert!(second.idempotent, "same session id must re-commit nothing");
        assert!(
            second.committed_effects.is_empty(),
            "an idempotent re-run commits no NEW effects"
        );
        assert_eq!(
            second.actor_key, first.actor_key,
            "the key is deterministic from the session id"
        );
        assert_eq!(
            second.evidence[0]["independent_readback"]["actor_present"],
            true
        );
    }

    #[test]
    fn an_embodied_body_records_its_own_passport_lifetime() {
        // A body must carry the lifetime it was admitted under. The reaper
        // (`crates/universe-e2e/src/bin/reap_session_bodies.rs`) judges staleness
        // from the node's OWN `expires_at` and treats a missing one as `unknown`,
        // never as expired — correctly. So a body written without it is
        // permanently unjudgeable by `--expired`, and the bodies accumulate at the
        // beacon with no criterion that can see them. Measured on the canonical
        // store at revision 304: 44 bodies, 44 `unknown`.
        let (_dir, mut world) = mounted();
        seed_beacon(&mut world);
        // `builder` admits at now=0 with ttl=100, so the passport expires at 100.
        let session = builder("claude:lifetime-1");
        let outcome = world
            .materialize_actor(&session)
            .expect("a sponsored embody succeeds");

        // Read the WRITTEN node back, found by its own content (never by a hex
        // key), exactly as the reaper finds it.
        let snapshot = world.snapshot().expect("mounted");
        let content = snapshot
            .entities
            .iter()
            .filter_map(|e| e.content.as_ref().and_then(|c| world.read_content(c)))
            .find(|c| {
                c.get("canonical_id").and_then(Value::as_str) == Some(&outcome.canonical_id)
            })
            .expect("the embodied body is readable from the store");

        assert_eq!(
            content.get("expires_at").and_then(Value::as_u64),
            Some(session.expires_at),
            "the body must carry the passport's lifetime: {content}"
        );
        assert_eq!(session.expires_at, 100, "the fixture's admitted lifetime");
        // The discriminator the reaper pairs it with is still there: a body with a
        // lifetime but no session would be an authored inhabitant, not a body.
        assert_eq!(
            content.get("embodied_session").and_then(Value::as_str),
            Some(session.session_id.as_str()),
            "still recognisable as a session body: {content}"
        );
    }

    #[test]
    fn strip_leading_claude_yields_a_single_claude_prefix() {
        // A `claude:`/`claude-` session marker is stripped before the `claude-`
        // namespace, so the id carries exactly one `claude-`.
        assert_eq!(strip_leading_claude("claude:d5bb4b3c-cfa7"), "d5bb4b3c-cfa7");
        assert_eq!(strip_leading_claude("claude-d5bb4b3c"), "d5bb4b3c");
        // A non-claude session is untouched.
        assert_eq!(strip_leading_claude("builder-1"), "builder-1");

        // End-to-end id form: `claude:<uuid>` → `actor:l1:mind:claude-<uuid>`,
        // NOT the doubled `claude-claude-<uuid>` the raw prefix produced.
        let id = format!(
            "actor:l1:mind:claude-{}",
            sanitize_session_id(strip_leading_claude("claude:d5bb4b3c-cfa7"))
        );
        assert_eq!(id, "actor:l1:mind:claude-d5bb4b3c-cfa7");
    }

    #[test]
    fn resolve_entity_by_canonical_id_finds_the_anchor_by_identity_and_never_dangles() {
        // Against a store seeded with the beacon at an ARBITRARY key (0x5EED),
        // the resolver finds it by its canonical_id — proving identity, not hex.
        let (_dir, mut world) = mounted();
        seed_beacon(&mut world);
        let (snapshot, resolved) = match &world {
            World::Mounted { supervisor, .. } => {
                let resolved = resolve_entity_by_canonical_id(
                    supervisor.snapshot(),
                    DEFAULT_ACTOR_ANCHOR_CANONICAL_ID,
                    &|c| supervisor.read_content(c).ok(),
                );
                (supervisor.snapshot(), resolved)
            }
            World::Unmounted { .. } => unreachable!("mounted above"),
        };
        assert_eq!(
            resolved,
            Some(EntityKey(0x5EED)),
            "the anchor resolves by canonical_id to its seeded key, whatever the hex"
        );

        // An identity that no entity carries resolves to None — the caller then
        // drops the edge (never dangles).
        assert!(
            resolve_entity_by_canonical_id(snapshot, "space:l2:nowhere:absent-v0", &|c| world
                .read_content(c))
            .is_none(),
            "an absent identity must not resolve to a phantom key"
        );
    }

    #[test]
    fn without_a_beacon_the_actor_is_still_written_and_the_edge_reported_dropped() {
        // No beacon seeded: the anchor identity resolves to None, so the edge is
        // dropped — but the inhabitant is still written, never dangled.
        let (_dir, mut world) = mounted();
        let outcome = world
            .materialize_actor(&builder("claude:no-beacon"))
            .expect("embody without a beacon still writes the actor");
        assert!(
            outcome.actor_present,
            "the actor node is written even with no beacon"
        );
        assert!(
            outcome.dropped_edge,
            "the beacon did not resolve, so the edge is dropped and reported"
        );
        assert!(!outcome.edge_present);
        assert!(outcome.beacon_key.is_none());
        // Only the actor node was committed (no edge).
        assert_eq!(outcome.committed_effects.len(), 1);
    }

    #[test]
    fn a_walk_in_without_propose_is_not_embodied_and_is_told_why() {
        let (_dir, mut world) = mounted();
        seed_beacon(&mut world);
        let before = world.snapshot().unwrap().revision.0;

        // A walk-in holds no `Propose`: embodiment is refused fail-closed.
        let refused = world
            .materialize_actor(&walk_in("tourist"))
            .expect_err("a walk-in without Propose is refused at the write site");
        let reason = refused.to_string();
        assert!(
            reason.contains("Propose"),
            "the refusal names the missing capability: {reason}"
        );
        assert!(
            reason.contains("tourist"),
            "the refusal is attributable to the session: {reason}"
        );

        // Fail-closed: nothing committed — the revision did not advance.
        assert_eq!(
            world.snapshot().unwrap().revision.0,
            before,
            "a refused embody must not advance the revision"
        );
    }
}
