//! The mounted Universe the adapter observes.
//!
//! `sense` and `act` are headless interfaces into the SAME Universe semantics
//! as the 3D world (CLAUDE.md, "Headless adapters"). The adapter therefore
//! holds a real [`Supervisor`] booted from a store + genesis, never a private
//! projection. When no store is mounted the adapter says so honestly instead of
//! fabricating a world: an unmounted `sense` is `unknown`, not empty.

use std::env;
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};
use universe_core::{EntityKey, RelationKey, UniverseError};
use universe_e2e::mutation_translate::{translate_mutation_proposal, MutationPlan};
use universe_store::{ContentRef, EntityRecord, RelationRecord, UniverseSnapshot, UniverseStore};
use universe_supervisor::{PhaseHook, RuntimeInventory, Supervisor, TickPhase};
use universe_transactions::{CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet};

use crate::session::{ActorSession, Capability};

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
    /// serves, and every `sense`/`act` reports the honest unmounted state. This
    /// keeps the transport observable for tooling without ever inventing a
    /// Universe.
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

    /// Commits an `act` as a **real graph mutation** in the one reality, then
    /// reads it back independently.
    ///
    /// This is the wired write path, and every command it emits is compiled
    /// through the generic write-side translator
    /// (`universe_e2e::mutation_translate::translate_mutation_proposal`): a
    /// mutation becomes exactly ONE of the four kernel verbs, a type-level
    /// guarantee, never a hand-rolled `UniverseCommand`. It assembles a
    /// `construct` entity (carrying the intent, its builder session, and
    /// `provenance: "built"`), a `construction_moment` (the authored evidence —
    /// a Built fact with no construction Moment is a forgery), and their
    /// relations, and commits them as ONE atomic transaction at the next tick
    /// boundary. The supervisor's snapshot advances, so the following `sense`
    /// sees the new revision.
    ///
    /// The commit is real but WRITTEN, not RUNNING: a `construct` node persists
    /// in the graph and the revision advances, but no live mechanism is wired or
    /// fired yet. Independent readback (a fresh store replay) is the evidence —
    /// never the committing snapshot's own word. A `prepare` rejection (conflict
    /// or validation) returns `Err` and commits nothing: prior state is intact.
    ///
    /// **Authority is enforced here, at the write site, fail-closed.** A durable
    /// transformation requires the acting session to hold the `Propose` (write)
    /// capability (minted only for a sponsored visitor or higher — see
    /// `session.rs`). A session that does not hold it is refused *before any store
    /// handle is opened or any command is built*: `Err` is returned, nothing is
    /// committed, and the reason names the session and the missing power. An
    /// unauthenticated walk-in can observe and speak, but it can never write.
    pub fn commit_proposal(
        &mut self,
        intent: &str,
        target: Option<&str>,
        session: &ActorSession,
    ) -> Result<ProposalOutcome, UniverseError> {
        // Fail-closed authority gate. A write with no `Propose` capability is
        // refused at the write site, attributably, before anything is touched —
        // never a silent drop, never a fabricated success (CLAUDE.md: authority
        // ENFORCED at the write site; "Acting requires a scope").
        if !session.has(Capability::Propose) {
            return Err(UniverseError::Validation(format!(
                "authority denied: session '{}' (origin '{}', status {:?}) does not hold the \
                 Propose (write) capability; a durable transformation requires a sponsor's \
                 Capability Bond. Nothing committed (fail-closed).",
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

        // A second store handle over the same root: the translator
        // content-addresses through a `UniverseStore`, and the supervisor does
        // not hand out its private handle. Same on-disk content segment, so the
        // `ContentRef`s it produces are readable at commit and readback.
        let store = UniverseStore::open(&*store_root)?;

        let base = supervisor.revision();
        let target_key = target.and_then(|t| resolve_key(supervisor.snapshot(), t));

        // Canonical predicate remap for the two write-path edges, via the SAME
        // table the injection path uses (`canonical_predicate`). The authored
        // names `CONSTRUCTED_BY` / `PROPOSES_ON` are NOT in the canonical
        // ontology; interning them raw would mint new non-canonical symbols. The
        // remap sends them to canonical `GROUNDS` (the construction_moment
        // grounds the construct — the JUSTIFIED_BY pattern, swapped) and
        // `PROPOSES_CHANGE_TO` (the construct proposes a change to its target),
        // so the default act path interns 0 new non-canonical predicate symbols.
        let (constructed_pred, constructed_swap) = canonical_predicate("CONSTRUCTED_BY")
            .ok_or_else(|| UniverseError::Validation("CONSTRUCTED_BY has no canonical mapping".into()))?;
        let (proposes_pred, proposes_swap) = canonical_predicate("PROPOSES_ON")
            .ok_or_else(|| UniverseError::Validation("PROPOSES_ON has no canonical mapping".into()))?;

        let requested: Vec<String> =
            ["construct", "construction_moment", constructed_pred, proposes_pred]
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
        let plan = supervisor.snapshot().plan_symbol_interning(&requested)?;
        let sym = |name: &str| -> Result<u32, UniverseError> {
            plan.assignments
                .get(name)
                .copied()
                .ok_or_else(|| UniverseError::Validation(format!("symbol {name} not planned")))
        };

        // Deterministic id from the builder + intent (NOT the revision), so
        // re-running the same act yields AlreadyCommitted (idempotent), not a
        // duplicate construct — the ids below are stable across revisions.
        let idempotency_key = format!(
            "mcp:act:{}:{:016x}",
            session.session_id,
            fnv64(&format!("{}|{}", session.session_id, intent))
        );
        let seed = fnv64(&idempotency_key);
        let kbase = (0x0AC7u128 << 96) | ((seed as u128) << 8);
        let construct = EntityKey(kbase);
        let moment = EntityKey(kbase | 1);
        let rel_constructed = RelationKey(kbase | 2);
        let rel_proposes = RelationKey(kbase | 3);

        // The runtime proposal: the content each PutEntity plan draws from by
        // field name. The translator content-addresses these — absent content is
        // never coerced to a default.
        let proposal = json!({
            "construct_content": {
                "kind": "construct",
                "intent": intent,
                "builder_session": session.session_id,
                "origin": session.origin,
                "target": target,
                "base_revision": base.0,
                "provenance": "built",
            },
            "moment_content": {
                "kind": "construction_moment",
                "authored_by": session.session_id,
                "origin": session.origin,
                "base_revision": base.0,
                "note": "MCP act: intent committed as built matter (a construct), written not fired",
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
            key: construct,
            generation: 0,
            symbol: sym("construct")?,
            content_field: Some("construct_content".into()),
        });
        plans.push(MutationPlan::PutEntity {
            key: moment,
            generation: 0,
            symbol: sym("construction_moment")?,
            content_field: Some("moment_content".into()),
        });
        // `GROUNDS` reads moment -> construct, so honour the remap's swap flag.
        let (c_src, c_tgt) = if constructed_swap { (moment, construct) } else { (construct, moment) };
        plans.push(MutationPlan::PutRelation {
            key: rel_constructed,
            generation: 0,
            source: c_src,
            target: c_tgt,
            predicate: sym(constructed_pred)?,
            content_field: None,
        });
        if let Some(target_key) = target_key {
            let (p_src, p_tgt) = if proposes_swap { (target_key, construct) } else { (construct, target_key) };
            plans.push(MutationPlan::PutRelation {
                key: rel_proposes,
                generation: 0,
                source: p_src,
                target: p_tgt,
                predicate: sym(proposes_pred)?,
                content_field: None,
            });
        }

        // Compile each plan through the translator and gather its single command
        // into ONE atomic write set — the four-verb boundary, enforced per plan.
        let ancestry = vec![format!("session:{}", session.session_id)];
        let mut commands: Vec<UniverseCommand> = Vec::with_capacity(plans.len());
        for mp in &plans {
            let ws = translate_mutation_proposal(
                mp,
                &proposal,
                &store,
                base,
                idempotency_key.clone(),
                ancestry.clone(),
            )?;
            commands.extend(ws.commands);
        }

        let write_set = UniverseWriteSet {
            base_revision: base,
            idempotency_key,
            causal_ancestry: ancestry,
            commands,
        };
        // A prepare rejection (conflict/validation) surfaces here as `Err` and
        // commits nothing — the supervisor's snapshot is untouched.
        let transaction = UniverseTransaction::prepare(supervisor.snapshot(), write_set)?;
        supervisor.enqueue(transaction);
        let receipts = supervisor.advance(&mut NoopHook)?;
        let idempotent = matches!(receipts.first(), Some(CommitReceipt::AlreadyCommitted { .. }));

        // Independent readback: a fresh reopen, never the committing snapshot.
        let fresh = supervisor.independent_readback()?;
        let construct_present = fresh.entities.iter().any(|e| e.key == construct);
        let moment_present = fresh.entities.iter().any(|e| e.key == moment);
        let constructed_by_present = fresh.relations.iter().any(|r| r.key == rel_constructed);

        // On an idempotent re-run nothing NEW is committed this call; the nodes
        // are already present (the readback evidence still proves it).
        let mut committed_effects = Vec::new();
        if !idempotent {
            committed_effects.push(
                json!({ "put_entity": construct.to_string(), "kind": "construct", "intent": intent }),
            );
            committed_effects
                .push(json!({ "put_entity": moment.to_string(), "kind": "construction_moment" }));
            committed_effects.push(
                json!({ "put_relation": rel_constructed.to_string(), "predicate": constructed_pred }),
            );
            if let Some(target_key) = target_key {
                committed_effects.push(json!({
                    "put_relation": rel_proposes.to_string(),
                    "predicate": proposes_pred,
                    "target": target_key.to_string(),
                }));
            }
        }

        Ok(ProposalOutcome {
            from_revision: base.0,
            to_revision: fresh.revision.0,
            idempotent,
            committed_effects,
            evidence: vec![json!({
                "independent_readback": {
                    "revision": fresh.revision.0,
                    "construct_present": construct_present,
                    "moment_present": moment_present,
                    "constructed_by_present": constructed_by_present,
                }
            })],
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
    /// It mirrors [`World::commit_proposal`] exactly: the same fail-closed
    /// `Propose` gate, the same generic four-verb write through
    /// `translate_mutation_proposal` (a mutation compiles to exactly one kernel
    /// verb, a type-level guarantee), the same canonical-predicate discipline
    /// (`PART_OF` is already canonical → 0 new predicate symbols), and the same
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
    /// The actor key is deterministic from the session id (the same fnv-based
    /// `kbase` scheme `commit_proposal` uses, in a disjoint prefix block), so
    /// re-arriving with the same session id is idempotent — `AlreadyCommitted`,
    /// no duplicate inhabitant.
    pub fn materialize_actor(
        &mut self,
        session: &ActorSession,
    ) -> Result<EmbodimentOutcome, UniverseError> {
        // Fail-closed authority gate, identical to `commit_proposal`. Being
        // present is free; embodying a durable inhabitant is a write, and a write
        // with no `Propose` is refused before any store handle is opened.
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

        // `PART_OF` is already a canonical predicate → the remap is the identity
        // and no new predicate symbol is minted.
        let (part_of_pred, part_of_swap) = canonical_predicate("PART_OF")
            .ok_or_else(|| UniverseError::Validation("PART_OF has no canonical mapping".into()))?;

        // The `actor` type symbol should already exist in the store; plan its
        // interning so a MISSING symbol is interned and REPORTED, never assumed.
        let requested: Vec<String> = ["actor", part_of_pred]
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

        // Deterministic key from the session id (mirrors `commit_proposal`'s
        // fnv-based `kbase`), in a disjoint `0x0AC0` prefix block so it never
        // collides with an `act` construct (`0x0AC7`). Re-arriving with the same
        // session id lands the same key + idempotency key → AlreadyCommitted.
        let idempotency_key = format!("mcp:arrive:embody:{}", session.session_id);
        let seed = fnv64(&idempotency_key);
        let kbase = (0x0AC0u128 << 96) | ((seed as u128) << 8);
        let actor = EntityKey(kbase);
        let rel_part_of = RelationKey(kbase | 1);

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

        // Compile each plan through the translator, gather into ONE atomic write
        // set, and commit at the next tick boundary — the same path as `act`.
        let ancestry = vec![format!("session:{}", session.session_id)];
        let mut commands: Vec<UniverseCommand> = Vec::with_capacity(plans.len());
        for mp in &plans {
            let ws = translate_mutation_proposal(
                mp,
                &proposal,
                &store,
                base,
                idempotency_key.clone(),
                ancestry.clone(),
            )?;
            commands.extend(ws.commands);
        }

        let write_set = UniverseWriteSet {
            base_revision: base,
            idempotency_key,
            causal_ancestry: ancestry,
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
            interned_symbols,
            committed_effects,
            evidence: vec![json!({
                "independent_readback": {
                    "revision": fresh.revision.0,
                    "actor_present": actor_present,
                    "part_of_beacon_present": edge_present,
                }
            })],
        })
    }
}

/// Canonical predicate remap: an authored (portable) edge name -> an
/// active-voice canonical predicate + a `swap` flag (reverse source/target).
/// Ported from the `inject_energy_pen` bin — the injector this generalises.
///
/// FOLLOWUP: this table is duplicated from the `inject_*` bins because `mcp` is
/// a detached workspace (see `Cargo.toml`). When the canonical remap earns a
/// shared home (e.g. an exported helper in `universe-e2e`/`universe-store`),
/// collapse both copies onto it. Every right-hand side below is a symbol from
/// `fixtures/ontology/canonical-ontology.json`; the left-hand sides are not.
fn canonical_predicate(authored: &str) -> Option<(&'static str, bool)> {
    Some(match authored {
        "PART_OF" => ("PART_OF", false),
        "IMPLEMENTED_IN" => ("IMPLEMENTS", true),
        "DEFINED_BY_CODE" => ("DEFINES", true),
        "IMPLEMENTED_BY" => ("COMPILES_TO", false),
        "JUSTIFIED_BY" => ("GROUNDS", true),
        "VALIDATED_BY" => ("TESTS", true),
        "OBSERVED_BY" => ("OBSERVES", true),
        "PRODUCES" => ("PRODUCES", false),
        "FEEDS" => ("FEEDS", false),
        "SUPPORTS" => ("MOTIVATES", false),
        // Write-path edges (the default `act` construct). Authored names that
        // are NOT canonical; remapped so `act` mints no new predicate symbols.
        // `CONSTRUCTED_BY` follows the JUSTIFIED_BY pattern: the construction
        // moment GROUNDS the construct, so swap. `PROPOSES_ON` is the construct
        // proposing a change to its target — canonical `PROPOSES_CHANGE_TO`.
        "CONSTRUCTED_BY" => ("GROUNDS", true),
        "PROPOSES_ON" => ("PROPOSES_CHANGE_TO", false),
        _ => return None,
    })
}

/// Member subtypes that are themselves canonical node-type symbols.
const CANONICAL_TYPE_SUBTYPES: &[&str] = &["metric", "validation"];

impl World {
    /// Injects an authored fixture subgraph (root node + members + relations)
    /// into the one reality as ONE atomic transaction, then reads it back
    /// independently. This generalises the `inject_energy_pen` bin: it maps
    /// authored predicates to canonical ones, keeps only relations whose both
    /// endpoints are injected (dropping the rest, reported — never dangled), and
    /// interns any missing symbols (reported — never silently). Re-injecting the
    /// same fixture is idempotent: if the root key already exists, it commits
    /// nothing and reports the subgraph as already present.
    pub fn inject_fixture(
        &mut self,
        fixture_path: &str,
        session: &ActorSession,
    ) -> Result<InjectionOutcome, String> {
        let World::Mounted { supervisor, .. } = self else {
            return Err("no Universe mounted".to_owned());
        };

        let doc: Value = serde_json::from_slice(
            &fs::read(fixture_path).map_err(|e| format!("read {fixture_path}: {e}"))?,
        )
        .map_err(|e| format!("parse {fixture_path}: {e}"))?;
        let root_id = doc
            .get("id")
            .and_then(Value::as_str)
            .ok_or("fixture has no top-level id")?
            .to_owned();

        // Nodes = root + members, keyed deterministically in a per-fixture block.
        let mut raw_nodes = vec![doc.clone()];
        if let Some(members) = doc.get("members").and_then(Value::as_array) {
            raw_nodes.extend(members.iter().cloned());
        }
        let seed = fnv64(&root_id);
        let kbase = (0x0FEEu128 << 96) | ((seed as u128) << 16);
        let node_id = |v: &Value| v.get("id").and_then(Value::as_str).map(str::to_owned);

        let mut id_to_key = std::collections::BTreeMap::new();
        for (i, node) in raw_nodes.iter().enumerate() {
            let id = node_id(node).ok_or("a node has no id")?;
            if id_to_key.insert(id.clone(), EntityKey(kbase | i as u128)).is_some() {
                return Err(format!("duplicate node id {id}"));
            }
        }
        let root_key = *id_to_key.get(&root_id).expect("root indexed");
        let base = supervisor.revision();

        // Idempotent: if the root already exists, the fixture is already injected.
        if supervisor.snapshot().entities.iter().any(|e| e.key == root_key) {
            let fresh = supervisor
                .independent_readback()
                .map_err(|e| e.to_string())?;
            let present = fresh.entities.iter().filter(|e| {
                id_to_key.values().any(|k| *k == e.key)
            }).count();
            return Ok(InjectionOutcome {
                fixture_id: root_id,
                from_revision: base.0,
                to_revision: fresh.revision.0,
                idempotent: true,
                nodes_injected: 0,
                relations_kept: 0,
                relations_dropped: Vec::new(),
                interned_symbols: Vec::new(),
                committed_effects: Vec::new(),
                evidence: vec![json!({
                    "independent_readback": { "revision": fresh.revision.0, "nodes_present": present }
                })],
            });
        }

        // Relations: remap to canonical, keep both-endpoints-present, drop rest.
        struct Kept {
            source: EntityKey,
            target: EntityKey,
            predicate: String,
        }
        let mut kept = Vec::new();
        let mut dropped = Vec::new();
        for r in doc.get("relations").and_then(Value::as_array).unwrap_or(&Vec::new()) {
            let source = r.get("source").and_then(Value::as_str).unwrap_or("");
            let target = r.get("target").and_then(Value::as_str).unwrap_or("");
            let authored = r.get("predicate").and_then(Value::as_str).unwrap_or("");
            let (predicate, swap) = canonical_predicate(authored)
                .ok_or_else(|| format!("authored predicate {authored} has no canonical mapping"))?;
            match (id_to_key.get(source), id_to_key.get(target)) {
                (Some(s), Some(t)) => {
                    let (src, tgt) = if swap { (*t, *s) } else { (*s, *t) };
                    kept.push(Kept { source: src, target: tgt, predicate: predicate.to_owned() });
                }
                _ => dropped.push(json!({
                    "source": source, "predicate": predicate, "target": target,
                    "reason": "endpoint not in injected set",
                })),
            }
        }

        // Symbols: entity type symbols + kept predicates. Intern any missing.
        let entity_symbol = |node: &Value| -> String {
            let subtype = node.get("subtype").and_then(Value::as_str).unwrap_or("");
            if CANONICAL_TYPE_SUBTYPES.contains(&subtype) {
                subtype.to_owned()
            } else {
                node.get("node_type").and_then(Value::as_str).unwrap_or("thing").to_owned()
            }
        };
        let mut requested: Vec<String> = raw_nodes.iter().map(entity_symbol).collect();
        requested.extend(kept.iter().map(|k| k.predicate.clone()));
        requested.sort();
        requested.dedup();
        let plan = supervisor
            .snapshot()
            .plan_symbol_interning(&requested)
            .map_err(|e| e.to_string())?;
        let interned_symbols = plan.additions.clone();
        let sym = |name: &str| -> Result<u32, String> {
            plan.assignments
                .get(name)
                .copied()
                .ok_or_else(|| format!("symbol {name} not planned"))
        };

        // Build the atomic write-set.
        let mut commands = Vec::new();
        if !plan.additions.is_empty() {
            commands.push(UniverseCommand::InternSymbols { symbols: plan.additions.clone() });
        }
        let mut committed_effects = Vec::new();
        for node in &raw_nodes {
            let id = node_id(node).unwrap();
            let content = json!({
                "canonical_id": id,
                "node_type": node.get("node_type"),
                "subtype": node.get("subtype"),
                "content": node.get("content"),
                "injected_by_session": session.session_id,
            });
            let content_ref = supervisor.append_content(&content).map_err(|e| e.to_string())?;
            commands.push(UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key: id_to_key[&id],
                    generation: 0,
                    symbol: sym(&entity_symbol(node))?,
                    content: Some(content_ref),
                },
            });
            committed_effects.push(json!({ "put_entity": id_to_key[&id].to_string(), "canonical_id": id }));
        }
        for (i, k) in kept.iter().enumerate() {
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(kbase | 0x8000 | i as u128),
                    generation: 0,
                    source: k.source,
                    target: k.target,
                    predicate: sym(&k.predicate)?,
                    content: None,
                },
            });
            committed_effects.push(json!({ "put_relation": k.predicate }));
        }

        let write_set = UniverseWriteSet {
            base_revision: base,
            idempotency_key: format!("mcp:inject:{root_id}"),
            causal_ancestry: vec![format!("session:{}", session.session_id)],
            commands,
        };
        let transaction =
            UniverseTransaction::prepare(supervisor.snapshot(), write_set).map_err(|e| e.to_string())?;
        supervisor.enqueue(transaction);
        supervisor.advance(&mut NoopHook).map_err(|e| e.to_string())?;

        // Independent readback: every injected node present by canonical_id.
        let fresh = supervisor.independent_readback().map_err(|e| e.to_string())?;
        let nodes_present = id_to_key
            .values()
            .filter(|k| fresh.entities.iter().any(|e| e.key == **k))
            .count();

        Ok(InjectionOutcome {
            fixture_id: root_id,
            from_revision: base.0,
            to_revision: fresh.revision.0,
            idempotent: false,
            nodes_injected: raw_nodes.len(),
            relations_kept: kept.len(),
            relations_dropped: dropped,
            interned_symbols,
            committed_effects,
            evidence: vec![json!({
                "independent_readback": {
                    "revision": fresh.revision.0,
                    "nodes_present": nodes_present,
                    "nodes_expected": id_to_key.len(),
                }
            })],
        })
    }
}

/// The measured outcome of injecting a fixture subgraph.
pub struct InjectionOutcome {
    pub fixture_id: String,
    pub from_revision: u64,
    pub to_revision: u64,
    pub idempotent: bool,
    pub nodes_injected: usize,
    pub relations_kept: usize,
    pub relations_dropped: Vec<Value>,
    pub interned_symbols: Vec<String>,
    pub committed_effects: Vec<Value>,
    pub evidence: Vec<Value>,
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

/// The measured outcome of a committed proposal — real effects plus independent
/// readback evidence.
#[derive(Debug)]
pub struct ProposalOutcome {
    pub from_revision: u64,
    pub to_revision: u64,
    pub idempotent: bool,
    pub committed_effects: Vec<serde_json::Value>,
    pub evidence: Vec<serde_json::Value>,
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

/// Resolves a 32-hex EntityKey or a symbol name to an existing entity key.
fn resolve_key(snapshot: &UniverseSnapshot, wanted: &str) -> Option<EntityKey> {
    if wanted.len() == 32 {
        if let Ok(raw) = u128::from_str_radix(wanted, 16) {
            let key = EntityKey(raw);
            if snapshot.entities.iter().any(|e| e.key == key) {
                return Some(key);
            }
        }
    }
    let index = snapshot.symbols.iter().position(|s| s == wanted)? as u32;
    snapshot
        .entities
        .iter()
        .find(|e| e.symbol == index)
        .map(|e| e.key)
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
    use crate::session::{admit, AdmissionRequest};

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
        let session = admit(&req, session_id.to_owned(), 0, 100).0;
        assert!(session.has(Capability::Propose), "builder must be able to write");
        session
    }

    /// An unauthenticated walk-in — observe + speak only, never `Propose`.
    fn walk_in(session_id: &str) -> ActorSession {
        let session = admit(&AdmissionRequest::default(), session_id.to_owned(), 0, 100).0;
        assert!(!session.has(Capability::Propose), "a walk-in cannot write");
        session
    }

    #[test]
    fn commit_proposal_advances_the_revision_and_is_read_back_independently() {
        let (_dir, mut world) = mounted();
        let before = world.snapshot().unwrap().revision.0;

        let outcome = world
            .commit_proposal("connect two beacons", None, &builder("builder-1"))
            .expect("a mounted commit succeeds");

        // A REAL revision advance — not the committing snapshot's own word, but a
        // fresh store replay.
        assert_eq!(outcome.from_revision, before);
        assert!(
            outcome.to_revision > before,
            "revision must advance: {} -> {}",
            outcome.from_revision,
            outcome.to_revision
        );
        assert!(!outcome.idempotent);

        // Independent readback evidence: the construct, its moment, and their
        // relation all survive a fresh reopen.
        let readback = &outcome.evidence[0]["independent_readback"];
        assert_eq!(readback["revision"], outcome.to_revision);
        assert_eq!(readback["construct_present"], true);
        assert_eq!(readback["moment_present"], true);
        assert_eq!(readback["constructed_by_present"], true);

        // The four-verb write path recorded real committed effects.
        assert!(outcome
            .committed_effects
            .iter()
            .any(|effect| effect["kind"] == "construct"));

        // And the supervisor's live snapshot agrees with the independent readback.
        assert_eq!(
            world.snapshot().unwrap().revision.0,
            outcome.to_revision,
            "the following `sense` sees the advanced revision"
        );
    }

    #[test]
    fn re_running_the_same_act_is_idempotent_and_writes_nothing_new() {
        let (_dir, mut world) = mounted();
        let session = builder("builder-2");

        let first = world
            .commit_proposal("build a room here", None, &session)
            .unwrap();
        assert!(!first.idempotent);

        let second = world
            .commit_proposal("build a room here", None, &session)
            .unwrap();
        assert!(
            second.idempotent,
            "same builder + intent must re-commit nothing"
        );
        assert!(
            second.committed_effects.is_empty(),
            "an idempotent re-run commits no NEW effects"
        );
        // The construct written by the first call still survives an independent
        // readback on the re-run.
        assert_eq!(
            second.evidence[0]["independent_readback"]["construct_present"],
            true
        );
    }

    #[test]
    fn a_walk_in_without_propose_commits_nothing_and_is_told_why() {
        let (_dir, mut world) = mounted();
        let before = world.snapshot().unwrap().revision.0;

        // The same intent the authorised builder commits, but as a walk-in with
        // no `Propose` capability: it must be refused, fail-closed.
        let refused = world
            .commit_proposal("connect two beacons", None, &walk_in("intruder"))
            .expect_err("a walk-in without Propose is refused at the write site");
        let reason = refused.to_string();
        assert!(
            reason.contains("Propose"),
            "the refusal names the missing capability: {reason}"
        );
        assert!(
            reason.contains("intruder"),
            "the refusal is attributable to the session: {reason}"
        );

        // Fail-closed: nothing was committed — the revision did not advance, and an
        // independent readback confirms no new entity from this refused write.
        assert_eq!(
            world.snapshot().unwrap().revision.0,
            before,
            "a refused write must not advance the revision"
        );
        let fresh = match &world {
            World::Mounted { supervisor, .. } => supervisor.independent_readback().unwrap(),
            World::Unmounted { .. } => unreachable!("mounted above"),
        };
        assert_eq!(
            fresh.revision.0, before,
            "independent readback agrees: the refused write left no trace"
        );

        // Contrast: an actor that DOES hold Propose still commits the same intent.
        let outcome = world
            .commit_proposal("connect two beacons", None, &builder("authorised"))
            .expect("a session with Propose commits");
        assert!(outcome.to_revision > before, "the authorised write advances the revision");
        assert_eq!(outcome.evidence[0]["independent_readback"]["construct_present"], true);
    }

    #[test]
    fn commit_on_an_unmounted_world_errors_and_writes_nothing() {
        let mut world = World::Unmounted {
            reason: "no store".into(),
        };
        let result = world.commit_proposal("anything", None, &builder("builder-3"));
        assert!(
            result.is_err(),
            "an unmounted world commits nothing and says why"
        );
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
            causal_ancestry: vec![],
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
