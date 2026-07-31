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
}
