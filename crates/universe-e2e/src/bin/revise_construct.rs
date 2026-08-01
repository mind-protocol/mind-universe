//! Revise a construct that is ALREADY in a live `UniverseStore`, in place, from
//! its authoritative fixture — the revision counterpart of `inject_construct`
//! (which refuses a key that already exists).
//!
//! `inject_construct` adds a construct that is not there. This bin revises one
//! that is: it upserts every member's content under its EXISTING key (the key
//! never moves, so every referencing relation holds), adds members that are new,
//! and adds authored relations that are not yet stored. It never deletes and
//! never retypes.
//!
//! Contract & honesty:
//!   * The authored predicate -> canonical predicate remap is read from the
//!     FIXTURE (`content.spine_encoding.predicate_remap`), not hard-coded here.
//!     The encoding is Universe data; this bin only applies it. An authored
//!     predicate with no mapping is a hard error.
//!   * A clean revision interns ZERO new symbols. A predicate or node symbol
//!     that is not already canonical is a hard error, never minted.
//!   * A node whose content is byte-identical is left alone (no event, no
//!     generation bump). Only real changes are written.
//!   * Retyping is refused: a node whose canonical symbol would change errors
//!     out rather than silently changing what kind of thing it is.
//!   * The whole revision is ONE atomic transaction at a tick boundary, with a
//!     bounded retry if a concurrent writer moves the base revision under us.
//!   * Readback is INDEPENDENT (fresh reopen from disk) and re-derives the
//!     construct's type spine from the committed store through the fixture's own
//!     `spine_encoding`, so the spine is proven traversable rather than declared.
//!
//!   * A revision is GATED. Authority is a graph fact: the acting capability set
//!     is READ from the actor's held `USED` edges and adjudicated by the sealed
//!     `revision_gate` the target's PROTOCOL declares — resolved by following the
//!     target's own `APPLIES_IN` edge, never by a constant in this binary. A
//!     refused revision fails CLOSED and commits a rejection receipt; an admitted
//!     one commits its EffectReceipt in the SAME transaction as the change, so no
//!     construct is ever revised without an attributable holder behind it.
//!
//! Bootstrap note: the gate itself was placed by this bin BEFORE this bin
//! enforced it — the same chicken-and-egg `underground_change_ground` had when it
//! was authored with one raw edit. That single ungated write is store revision
//! 320; every revision after it goes through the door.
//!
//! Usage: `revise_construct <fixture.json> --actor <canonical_id>
//!                          --justification <why> [--store <dir>] [--dry-run]
//!                          [--retire-orphans]`

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    path::PathBuf,
};

use universe_capabilities::{MutateAdmission, SealedCapabilityPort};
use universe_core::{EntityKey, RelationKey, Tick};
use universe_query::read_actor_capability_set;
use universe_store::{
    EntityRecord, IndexedUniverseSnapshot, RelationRecord, UniverseSnapshot, UniverseStore,
};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// The canonical predicate an actor's held capability set is read from (the
/// grant's authored HOLDS_CAPABILITY remaps to USED; see the grant fixture).
const GRANT_PREDICATE: &str = "USED";
/// The capability-entity content field naming the capability it confers.
const CAPABILITY_FIELD: &str = "capability";
/// Bounded relation budget for the actor's capability read (never a full scan).
const CAPABILITY_READ_BUDGET: usize = 64;
/// The canonical predicate carrying a construct's conformance claim. Following
/// it is how the protocol — and therefore the gate — is found in the graph.
const CONFORMANCE_PREDICATE: &str = "APPLIES_IN";
/// Receipt key block, disjoint from every construct block and from
/// `underground_change_ground`'s 0x00E0_0000 receipts.
const RECEIPT_ENTITY_BASE: u128 = 0x00D0_0000;
const RECEIPT_REL_BASE: u128 = 0x00D8_0000;

/// Member subtypes that are ALSO canonical node-type symbols.
const CANONICAL_TYPE_SUBTYPES: &[&str] = &["metric", "validation"];

/// The 12 role subtypes a construct must carry.
const REQUIRED_ROLES: &[&str] = &[
    "objective",
    "pattern",
    "vocabulary",
    "behavior",
    "algorithm",
    "code",
    "implementation",
    "justification",
    "validation",
    "observability_algorithm",
    "metric",
    "health",
];

const MAX_COMMIT_ATTEMPTS: usize = 5;

struct Node {
    id: String,
    node_type: String,
    subtype: String,
    content: serde_json::Value,
}

/// An authored predicate resolved through the fixture's encoding table.
struct Encoded {
    canonical: String,
    swap: bool,
}

/// Commit one standalone EffectReceipt Moment, linked from the protocol with
/// PRODUCES. Used ONLY for a rejection: an admitted revision carries its receipt
/// inside the revision transaction itself, so there is no window in which the
/// construct is changed and the attribution is not.
fn commit_moment(
    store: &UniverseStore,
    snapshot: &mut UniverseSnapshot,
    protocol_key: EntityKey,
    content: &serde_json::Value,
    idempotency_key: String,
) -> Result<(EntityKey, universe_transactions::CommitReceipt), Box<dyn Error>> {
    let (receipt_key, relation_key) = receipt_keys(snapshot);
    let commands = receipt_commands(store, snapshot, protocol_key, content, receipt_key, relation_key)?;
    let write_set = UniverseWriteSet {
        base_revision: snapshot.revision,
        idempotency_key,
        commands,
    };
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(snapshot, write_set)?;
    let receipt = transaction.commit(store, snapshot, boundary_tick)?;
    Ok((receipt_key, receipt))
}

/// Receipt entity + relation keys for this tick, in the receipt block.
fn receipt_keys(snapshot: &UniverseSnapshot) -> (EntityKey, RelationKey) {
    let salt = snapshot.tick.0 as u128;
    (
        EntityKey(RECEIPT_ENTITY_BASE + salt),
        RelationKey(RECEIPT_REL_BASE + salt),
    )
}

/// The two commands that record an effect receipt: the Moment, and the PRODUCES
/// edge from the protocol that adjudicated it.
fn receipt_commands(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    protocol_key: EntityKey,
    content: &serde_json::Value,
    receipt_key: EntityKey,
    relation_key: RelationKey,
) -> Result<Vec<UniverseCommand>, Box<dyn Error>> {
    if snapshot.entities.iter().any(|e| e.key == receipt_key) {
        return Err(format!("receipt key {:#x} already exists", receipt_key.0).into());
    }
    let moment_symbol = snapshot
        .symbol_id("moment")
        .ok_or("canonical symbol 'moment' is not interned in this store")?;
    let produces_symbol = snapshot
        .symbol_id("PRODUCES")
        .ok_or("canonical predicate 'PRODUCES' is not interned in this store")?;
    Ok(vec![
        UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: receipt_key,
                generation: 0,
                symbol: moment_symbol,
                content: Some(store.append_content(content)?),
            },
        },
        UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: relation_key,
                generation: 0,
                source: protocol_key,
                target: receipt_key,
                predicate: produces_symbol,
                content: None,
            },
        },
    ])
}

fn main() {
    if let Err(error) = run() {
        eprintln!("REVISION FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut positional: Vec<String> = Vec::new();
    let mut store_dir = PathBuf::from("artifacts/ontology-registry/current/store");
    let mut dry_run = false;
    let mut retire_orphans = false;
    let mut actor: Option<String> = None;
    let mut justification: Option<String> = None;
    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--store" => store_dir = PathBuf::from(args.next().ok_or("--store needs a value")?),
            "--dry-run" => dry_run = true,
            "--retire-orphans" => retire_orphans = true,
            "--actor" => actor = Some(args.next().ok_or("--actor needs a canonical_id")?),
            "--justification" => justification = Some(args.next().ok_or("--justification needs a value")?),
            _ => positional.push(a),
        }
    }
    let fixture_path = PathBuf::from(
        positional
            .first()
            .ok_or("usage: revise_construct <fixture.json> [--store dir] [--dry-run]")?,
    );
    println!("fixture  : {}", fixture_path.display());
    println!("store dir: {}", store_dir.display());
    if dry_run {
        println!("mode     : DRY RUN (nothing will be written)");
    }

    // ---- 1. Parse the authoritative fixture. ------------------------------
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&fixture_path)?)?;
    let root_id = doc
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("fixture has no top-level id")?
        .to_string();

    let contract_kind = doc
        .pointer("/content/contractKind")
        .and_then(|v| v.as_str())
        .unwrap_or("(none)");
    if contract_kind != "construct" {
        return Err(format!("fixture root contractKind is '{contract_kind}', expected 'construct'").into());
    }

    let node_from = |v: &serde_json::Value| -> Result<Node, Box<dyn Error>> {
        Ok(Node {
            id: v.get("id").and_then(|x| x.as_str()).ok_or("node without id")?.to_string(),
            node_type: v.get("node_type").and_then(|x| x.as_str()).unwrap_or("thing").to_string(),
            subtype: v.get("subtype").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            content: v.get("content").cloned().unwrap_or(serde_json::Value::Null),
        })
    };
    let mut nodes: Vec<Node> = vec![node_from(&doc)?];
    for member in doc
        .get("members")
        .and_then(|v| v.as_array())
        .ok_or("fixture has no members array")?
    {
        nodes.push(node_from(member)?);
    }
    for role in REQUIRED_ROLES {
        if !nodes.iter().any(|n| n.subtype == *role) {
            return Err(format!("not a valid construct: missing role subtype '{role}'").into());
        }
    }

    // The encoding table is DATA carried by the construct, not policy in this bin.
    let remap: BTreeMap<String, Encoded> = doc
        .pointer("/content/spine_encoding/predicate_remap")
        .and_then(|v| v.as_object())
        .ok_or("fixture carries no content.spine_encoding.predicate_remap")?
        .iter()
        .map(|(authored, spec)| {
            let canonical = spec
                .get("canonical")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("remap entry {authored} has no canonical"))?
                .to_string();
            let swap = spec.get("swap").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok::<_, String>((authored.clone(), Encoded { canonical, swap }))
        })
        .collect::<Result<_, _>>()?;
    println!("spine encoding: {} authored predicates mapped (from the fixture)", remap.len());

    // ---- 2. Open the live store and resolve identity. ---------------------
    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    println!(
        "\nbase revision: {} | entities: {} | relations: {}",
        snapshot.revision.0,
        snapshot.entities.len(),
        snapshot.relations.len()
    );

    let id_to_key = canonical_index(&store, &snapshot)?;
    let root_key = *id_to_key
        .get(&root_id)
        .ok_or_else(|| format!("{root_id} is NOT in this store — use inject_construct, not revise_construct"))?;

    // ---- 2b. THE GATE. Admission precedes every planned write. ------------
    // Authority is a graph fact. The gate is found by following the target's own
    // conformance edge to its protocol and reading the `revision_gate` that
    // protocol declares; the acting set is read from the actor's held USED
    // edges. Neither is a constant here, and neither is the request's word.
    let actor = actor.ok_or(
        "revising a construct requires --actor <canonical_id>: a revision with no holder behind it \
         is exactly what this gate exists to stop",
    )?;
    let justification = justification.ok_or("--justification <why> is required and is recorded on the receipt")?;

    let conformance_symbol = snapshot
        .symbol_id(CONFORMANCE_PREDICATE)
        .ok_or("canonical predicate 'APPLIES_IN' is not interned in this store")?;
    let protocol_keys: Vec<EntityKey> = snapshot
        .relations
        .iter()
        .filter(|r| r.source == root_key && r.predicate == conformance_symbol)
        .map(|r| r.target)
        .collect();
    let [protocol_key] = protocol_keys[..] else {
        return Err(format!(
            "{root_id} carries {} APPLIES_IN edge(s); exactly one is required to locate the \
             protocol whose revision_gate governs it (a construct conforming to no protocol \
             declares no door, and is not revisable through this path)",
            protocol_keys.len()
        )
        .into());
    };
    let protocol_content = {
        let entity = snapshot
            .entities
            .iter()
            .find(|e| e.key == protocol_key)
            .ok_or("the protocol this construct conforms to is not in the store")?;
        store.read_content(entity.content.as_ref().ok_or("protocol node has no content")?)?
    };
    let protocol_id = protocol_content
        .get("canonical_id")
        .and_then(|v| v.as_str())
        .unwrap_or("(unnamed)")
        .to_string();
    // No fallback: a missing gate refuses. Substituting a native default for
    // absent Universe state is how an ungated write path comes back.
    let gate_value = protocol_content
        .pointer("/content/revision_gate")
        .cloned()
        .ok_or_else(|| {
            format!("protocol {protocol_id} declares no revision_gate in the store — refusing (there is no default door)")
        })?;
    let port: SealedCapabilityPort = serde_json::from_value(gate_value)?;
    println!(
        "\nrevision gate: {} requires '{}' (read from protocol {protocol_id})",
        port.port_id, port.required_mutate_capability
    );

    let grant_predicate = snapshot
        .symbol_id(GRANT_PREDICATE)
        .ok_or("canonical grant predicate 'USED' is not interned in this store")?;
    let indexed = IndexedUniverseSnapshot::new(snapshot.clone())?;
    let (held, actor_resolved, read_status) = match id_to_key.get(&actor) {
        Some(key) => {
            let set = read_actor_capability_set(
                &indexed,
                &store,
                *key,
                grant_predicate,
                CAPABILITY_FIELD,
                CAPABILITY_READ_BUDGET,
            )?;
            (set.capabilities, true, format!("{:?}", set.status))
        }
        // An actor with no graph identity has an empty acting set. That is a
        // real fail-closed decision, not an accident: an empty set holds nothing.
        None => (BTreeSet::new(), false, "actor_not_in_graph".to_string()),
    };
    let held_list: Vec<String> = held.iter().cloned().collect();
    println!("acting set for {actor} (resolved={actor_resolved}, read={read_status}): {held_list:?}");

    let admission = port.resolve_mutate(&held);
    if let MutateAdmission::Rejected {
        port_id,
        required_capability,
        reason,
    } = &admission
    {
        let rejection = serde_json::json!({
            "canonical_id": format!("moment:l2:mind-universe:construct-revision-rejected:{}", snapshot.tick.0),
            "node_type": "moment",
            "subtype": "effect_receipt",
            "content": {
                "kind": "construct_revision_rejection",
                "effect": "revise_construct",
                "outcome": "rejected",
                "reason": reason,
                "target_construct": root_id,
                "protocol": protocol_id,
                "port_id": port_id,
                "actor": actor,
                "actor_resolved": actor_resolved,
                "capability_read_status": read_status,
                "held_capabilities": held_list,
                "required_capability": required_capability,
                "justification": justification,
                "authority_source": "actor graph-held USED edges (not a request field)",
                "state_delta": "none (fail closed; nothing was planned, nothing was written)",
            }
        });
        let idempotency_key = format!("effect:construct-revision-rejected:{}:{actor}", snapshot.tick.0);
        let (rkey, rcpt) = commit_moment(&store, &mut snapshot, protocol_key, &rejection, idempotency_key)?;
        println!("revision gate: REFUSED — rejection receipt committed ({:#x}) {rcpt:?}", rkey.0);
        return Err(format!(
            "actor '{actor}' does not hold '{required_capability}' in the graph (held {held_list:?}) \
             — fail closed, {root_id} NOT revised"
        )
        .into());
    }
    let admitted_capability = match &admission {
        MutateAdmission::Admitted { capability, .. } => capability.clone(),
        MutateAdmission::Rejected { .. } => unreachable!("rejection returned above"),
    };
    println!("revision gate: ADMITTED — '{actor}' holds '{admitted_capability}' via a graph grant edge");

    // The construct's disjoint key block, derived from the root key it was
    // injected under: entities at base+i, relations at base+0x8000+i.
    let entity_base = root_key.0;
    let rel_base = entity_base + 0x8000;
    let mut next_entity = snapshot
        .entities
        .iter()
        .map(|e| e.key.0)
        .filter(|k| *k >= entity_base && *k < rel_base)
        .max()
        .map(|k| k + 1)
        .unwrap_or(entity_base + 1);
    let mut next_relation = snapshot
        .relations
        .iter()
        .map(|r| r.key.0)
        .filter(|k| *k >= rel_base && *k < entity_base + 0x1_0000)
        .max()
        .map(|k| k + 1)
        .unwrap_or(rel_base);
    println!(
        "construct: {root_id} (root {:#x}) | next free entity {:#x}, next free relation {:#x}",
        root_key.0, next_entity, next_relation
    );

    // ---- 3. Plan entity upserts. -----------------------------------------
    let entity_symbol = |node: &Node| -> String {
        if CANONICAL_TYPE_SUBTYPES.contains(&node.subtype.as_str()) {
            node.subtype.clone()
        } else {
            node.node_type.clone()
        }
    };
    let sym = |snapshot: &UniverseSnapshot, name: &str| -> Result<u32, Box<dyn Error>> {
        snapshot.symbol_id(name).ok_or_else(|| {
            format!("'{name}' is not an interned canonical symbol — revision would mint it (refused)").into()
        })
    };

    struct PlannedEntity {
        key: EntityKey,
        generation: u32,
        symbol: u32,
        content: serde_json::Value,
        id: String,
        status: &'static str,
    }
    let mut planned_entities: Vec<PlannedEntity> = Vec::new();
    let mut node_keys: BTreeMap<String, EntityKey> = BTreeMap::new();

    for node in &nodes {
        let symbol_name = entity_symbol(node);
        let symbol = sym(&snapshot, &symbol_name)?;
        let content = serde_json::json!({
            "canonical_id": node.id,
            "node_type": node.node_type,
            "subtype": node.subtype,
            "content": node.content,
        });
        match id_to_key.get(&node.id) {
            Some(key) => {
                let existing = snapshot
                    .entities
                    .iter()
                    .find(|e| e.key == *key)
                    .ok_or_else(|| format!("indexed key {:#x} vanished", key.0))?;
                if existing.symbol != symbol {
                    return Err(format!(
                        "refusing to retype {}: stored symbol '{}' != fixture symbol '{}'",
                        node.id,
                        snapshot.symbols.get(existing.symbol as usize).map(String::as_str).unwrap_or("?"),
                        symbol_name
                    )
                    .into());
                }
                let unchanged = match existing.content.as_ref() {
                    Some(ptr) => store.read_content(ptr)? == content,
                    None => false,
                };
                node_keys.insert(node.id.clone(), *key);
                if unchanged {
                    println!("  SAME     {}", short(&node.id));
                    continue;
                }
                println!("  REVISE   {}  (gen {} -> {})", short(&node.id), existing.generation, existing.generation + 1);
                planned_entities.push(PlannedEntity {
                    key: *key,
                    generation: existing.generation + 1,
                    symbol,
                    content,
                    id: node.id.clone(),
                    status: "revised",
                });
            }
            None => {
                let key = EntityKey(next_entity);
                next_entity += 1;
                println!("  NEW      {}  ({:#x})", short(&node.id), key.0);
                node_keys.insert(node.id.clone(), key);
                planned_entities.push(PlannedEntity {
                    key,
                    generation: 0,
                    symbol,
                    content,
                    id: node.id.clone(),
                    status: "added",
                });
            }
        }
    }

    // ---- 4. Plan relation additions (through the fixture's encoding). -----
    struct PlannedRelation {
        key: RelationKey,
        source: EntityKey,
        target: EntityKey,
        predicate: String,
        predicate_symbol: u32,
        authored: String,
    }
    let mut planned_relations: Vec<PlannedRelation> = Vec::new();
    let resolve = |id: &str| -> Option<EntityKey> {
        node_keys.get(id).copied().or_else(|| id_to_key.get(id).copied())
    };
    let empty = Vec::new();
    for r in doc.get("relations").and_then(|v| v.as_array()).unwrap_or(&empty) {
        let source_id = r.get("source").and_then(|v| v.as_str()).unwrap_or("");
        let target_id = r.get("target").and_then(|v| v.as_str()).unwrap_or("");
        let authored = r.get("predicate").and_then(|v| v.as_str()).unwrap_or("");
        let encoded = remap
            .get(authored)
            .ok_or_else(|| format!("authored predicate '{authored}' is not in the fixture's spine_encoding"))?;
        let (Some(s), Some(t)) = (resolve(source_id), resolve(target_id)) else {
            return Err(format!("relation endpoint missing: {source_id} -[{authored}]-> {target_id}").into());
        };
        let (src, tgt) = if encoded.swap { (t, s) } else { (s, t) };
        let predicate_symbol = sym(&snapshot, &encoded.canonical)?;
        let exists = snapshot
            .relations
            .iter()
            .any(|x| x.source == src && x.target == tgt && x.predicate == predicate_symbol);
        if exists {
            println!("  HAVE     {:<16} {} -> {}", encoded.canonical, short(source_id), short(target_id));
            continue;
        }
        println!(
            "  WIRE     {:<16} {} -> {}   (encodes {} {} {})",
            encoded.canonical,
            short(if encoded.swap { target_id } else { source_id }),
            short(if encoded.swap { source_id } else { target_id }),
            short(source_id),
            authored,
            short(target_id)
        );
        planned_relations.push(PlannedRelation {
            key: RelationKey(next_relation),
            source: src,
            target: tgt,
            predicate: encoded.canonical.clone(),
            predicate_symbol,
            authored: authored.to_string(),
        });
        next_relation += 1;
    }

    // ---- 4b. Orphans: edges in the construct's OWN key block that the fixture
    //          no longer authors. A revision that cannot retract what it stopped
    //          authoring leaves the graph asserting things nobody stands behind.
    //          Reported always; retired only when asked, because a relation this
    //          bin did not write may still be someone else's authored edge.
    let authored: Vec<(EntityKey, EntityKey, u32)> = doc
        .get("relations")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty)
        .iter()
        .filter_map(|r| {
            let s = resolve(r.get("source")?.as_str()?)?;
            let t = resolve(r.get("target")?.as_str()?)?;
            let e = remap.get(r.get("predicate")?.as_str()?)?;
            let p = snapshot.symbol_id(&e.canonical)?;
            Some(if e.swap { (t, s, p) } else { (s, t, p) })
        })
        .collect();
    let mut orphans: Vec<(RelationKey, u32, String)> = Vec::new();
    for relation in &snapshot.relations {
        if !(rel_base..entity_base + 0x1_0000).contains(&relation.key.0) {
            continue;
        }
        if authored.contains(&(relation.source, relation.target, relation.predicate)) {
            continue;
        }
        let label = format!(
            "{:#x} -[{}]-> {:#x}",
            relation.source.0,
            snapshot.symbols.get(relation.predicate as usize).map(String::as_str).unwrap_or("?"),
            relation.target.0
        );
        println!(
            "  ORPHAN   {:#x}  {}{}",
            relation.key.0,
            label,
            if retire_orphans { "   (retiring)" } else { "   (kept — pass --retire-orphans to tombstone)" }
        );
        orphans.push((relation.key, relation.generation, label));
    }
    if !retire_orphans {
        orphans.clear();
    }

    println!(
        "\nplan: {} entity write(s), {} relation write(s), {} tombstone(s), 0 new symbols",
        planned_entities.len(),
        planned_relations.len(),
        orphans.len()
    );
    if planned_entities.is_empty() && planned_relations.is_empty() && orphans.is_empty() {
        println!("Nothing to revise — the store already holds this fixture. No event written.");
        return Ok(());
    }
    if dry_run {
        println!("DRY RUN: nothing written.");
        return Ok(());
    }

    // ---- 5. One atomic transaction, with bounded retry. -------------------
    // The live store has more than one writer; a concurrent commit moves the
    // base revision under us. Retrying is honest here because the plan is
    // idempotent by construction (it re-derives what is already present).
    let mut attempt = 0;
    let (receipt, receipt_key) = loop {
        attempt += 1;
        let base_revision = snapshot.revision;
        let mut commands = Vec::new();
        for e in &planned_entities {
            commands.push(UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key: e.key,
                    generation: e.generation,
                    symbol: e.symbol,
                    content: Some(store.append_content(&e.content)?),
                },
            });
        }
        for r in &planned_relations {
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: r.key,
                    generation: 0,
                    source: r.source,
                    target: r.target,
                    predicate: r.predicate_symbol,
                    content: None,
                },
            });
        }
        for (key, generation, _) in &orphans {
            // Compare-and-swap, not a bump: the store requires the generation to
            // MATCH the record being retired, so a stale plan cannot tombstone a
            // relation someone else has since rewritten.
            commands.push(UniverseCommand::TombstoneRelation {
                relation: *key,
                generation: *generation,
            });
        }
        // The attribution rides INSIDE the same command set as the change. It
        // names the base revision and the idempotency key rather than the
        // resulting revision, which is not knowable before the commit and is
        // readable from the store afterwards anyway.
        let idempotency_key = format!("mutation:revise-construct:{root_id}:{}", base_revision.0);
        let (receipt_key, relation_key) = receipt_keys(&snapshot);
        let receipt_content = serde_json::json!({
            "canonical_id": format!("moment:l2:mind-universe:construct-revision:{}", snapshot.tick.0),
            "node_type": "moment",
            "subtype": "effect_receipt",
            "content": {
                "kind": "construct_revision_receipt",
                "effect": "revise_construct",
                "outcome": "committed",
                "target_construct": root_id,
                "protocol": protocol_id,
                "port_id": port.port_id,
                "actor": actor,
                "admitted_capability": admitted_capability,
                "held_capabilities": held_list,
                "required_capability": port.required_mutate_capability,
                "capability_read_status": read_status,
                "authority_source": "actor graph-held USED edges (not a request field)",
                "justification": justification,
                "source_fixture": fixture_path.display().to_string(),
                "base_revision": base_revision.0,
                "transaction": idempotency_key,
                "atomic_with_the_change": true,
                "state_delta": {
                    "entities_written": planned_entities.len(),
                    "entities_added": planned_entities.iter().filter(|e| e.status == "added").count(),
                    "relations_added": planned_relations.len(),
                    "relations_retired": orphans.len(),
                    "symbols_interned": 0,
                },
                "epistemic": "This receipt records that the change was ADMITTED and committed. \
                              It is not evidence that the construct is correct, conformant, wired or \
                              healthy — read the nodes back for that.",
            }
        });
        commands.extend(receipt_commands(
            &store,
            &snapshot,
            protocol_key,
            &receipt_content,
            receipt_key,
            relation_key,
        )?);
        let write_set = UniverseWriteSet {
            base_revision,
            idempotency_key,
            commands,
        };
        let boundary_tick = Tick(snapshot.tick.0 + 1);
        match UniverseTransaction::prepare(&snapshot, write_set)
            .and_then(|t| t.commit(&store, &mut snapshot, boundary_tick))
        {
            Ok(receipt) => break (receipt, receipt_key),
            Err(error) => {
                // Retry ONLY a lost race. Re-read: if the revision has not moved,
                // nobody raced us and the error is ours — retrying would just
                // repeat it while reporting it as contention.
                snapshot = store.replay(store.load_snapshot()?)?;
                if snapshot.revision == base_revision || attempt >= MAX_COMMIT_ATTEMPTS {
                    return Err(error.into());
                }
                eprintln!(
                    "  attempt {attempt} lost the race (base {} -> {}); re-reading and retrying",
                    base_revision.0, snapshot.revision.0
                );
            }
        }
    };
    println!("\ncommitted as ONE atomic transaction (attempt {attempt}): {receipt:?}");

    // ---- 6. INDEPENDENT readback from a fresh reopen. ---------------------
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    println!("\n-- independent readback (fresh reopen) --");
    println!("revision now: {} | entities: {} | relations: {}", after.revision.0, after.entities.len(), after.relations.len());

    for e in &planned_entities {
        let entity = after
            .entities
            .iter()
            .find(|x| x.key == e.key)
            .ok_or_else(|| format!("{} ({:#x}) not found on readback", e.id, e.key.0))?;
        let content = fresh.read_content(
            entity.content.as_ref().ok_or_else(|| format!("{} has no content on readback", e.id))?,
        )?;
        if content != e.content {
            return Err(format!("{} read back with content that is not what was written", e.id).into());
        }
        println!("  {:<8} {}  gen {}", e.status, short(&e.id), entity.generation);
    }
    for r in &planned_relations {
        let present = after
            .relations
            .iter()
            .any(|x| x.source == r.source && x.target == r.target && x.predicate == r.predicate_symbol);
        if !present {
            return Err(format!("relation {} ({}) missing on readback", r.predicate, r.authored).into());
        }
    }
    for (key, _, label) in &orphans {
        if after.relations.iter().any(|x| x.key == *key) {
            return Err(format!("orphan {label} survived its tombstone on readback").into());
        }
        println!("  retired  {label}");
    }
    println!(
        "  {} entity write(s), {} relation write(s) and {} tombstone(s) read back verbatim",
        planned_entities.len(),
        planned_relations.len(),
        orphans.len()
    );

    // The receipt is read back from the store like anything else. A receipt this
    // process merely believes it wrote is not attribution.
    let receipt_entity = after
        .entities
        .iter()
        .find(|e| e.key == receipt_key)
        .ok_or("the revision receipt is not in the store on readback — the change is unattributed")?;
    let receipt_readback =
        fresh.read_content(receipt_entity.content.as_ref().ok_or("receipt has no content")?)?;
    let receipted_actor = receipt_readback
        .pointer("/content/actor")
        .and_then(|v| v.as_str())
        .unwrap_or("(none)");
    if receipted_actor != actor {
        return Err(format!("receipt attributes the revision to '{receipted_actor}', not '{actor}'").into());
    }
    println!(
        "  receipt {:#x} reads back: {} admitted via '{}', PRODUCES edge from {protocol_id}",
        receipt_key.0, receipted_actor, admitted_capability
    );

    // ---- 7. Re-derive the type spine FROM THE COMMITTED STORE. ------------
    // The point of the encoding table: a reader must be able to traverse the
    // canonical stored relations and recover the authored spine. Anything less
    // and the spine is a sentence, not a structure.
    let bindings: BTreeMap<String, String> = doc
        .pointer("/content/spine_encoding/role_bindings")
        .and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let after_index = canonical_index(&fresh, &after)?;
    let type_edges = doc
        .pointer("/content/protocol/type_edges")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut resolved = 0usize;
    let mut unresolved: Vec<String> = Vec::new();
    for edge in &type_edges {
        let from = edge.get("from").and_then(|v| v.as_str()).unwrap_or("");
        let to = edge.get("to").and_then(|v| v.as_str()).unwrap_or("");
        let authored = edge.get("predicate").and_then(|v| v.as_str()).unwrap_or("");
        let label = format!("{from} {authored} {to}");
        let (Some(from_id), Some(to_id)) = (bindings.get(from), bindings.get(to)) else {
            unresolved.push(format!("{label}  (no role_binding)"));
            continue;
        };
        let (Some(encoded), Some(s), Some(t)) =
            (remap.get(authored), after_index.get(from_id), after_index.get(to_id))
        else {
            unresolved.push(format!("{label}  (unmapped predicate or absent endpoint)"));
            continue;
        };
        let (src, tgt) = if encoded.swap { (*t, *s) } else { (*s, *t) };
        let Some(predicate_symbol) = after.symbol_id(&encoded.canonical) else {
            unresolved.push(format!("{label}  (canonical symbol {} absent)", encoded.canonical));
            continue;
        };
        if after
            .relations
            .iter()
            .any(|x| x.source == src && x.target == tgt && x.predicate == predicate_symbol)
        {
            resolved += 1;
        } else {
            unresolved.push(format!("{label}  (no stored {} edge)", encoded.canonical));
        }
    }
    println!(
        "\n-- type spine re-derived from the committed store through spine_encoding --"
    );
    println!("  resolved: {resolved}/{}", type_edges.len());
    for u in &unresolved {
        println!("  UNRESOLVED  {u}");
    }
    if !unresolved.is_empty() {
        // The transaction ALREADY committed — this check runs against the
        // committed store because that is the only thing worth checking. Say so
        // plainly rather than letting a non-zero exit imply nothing was written.
        return Err(format!(
            "the revision COMMITTED at revision {} and is durable, but {} type edge(s) do not \
             traverse it. The construct is now stored and NON-CONFORMANT on those edges; fix the \
             fixture's spine_encoding or relations and re-run (this bin is idempotent).",
            after.revision.0,
            unresolved.len()
        )
        .into());
    }

    println!("\nRESULT: revised construct {root_id} in place as ONE atomic transaction, 0 new symbols.");
    println!("        Every type edge traverses canonical stored relations through the encoding the construct itself carries.");
    println!("        graph_status: WRITTEN. wiring/runtime/health remain not_wired / not_running / not_measured.");
    Ok(())
}

/// canonical_id -> EntityKey over the whole store.
fn canonical_index(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
) -> Result<BTreeMap<String, EntityKey>, Box<dyn Error>> {
    let mut index = BTreeMap::new();
    for entity in &snapshot.entities {
        if let Some(ptr) = entity.content.as_ref() {
            let content = store.read_content(ptr)?;
            if let Some(cid) = content.get("canonical_id").and_then(|v| v.as_str()) {
                index.insert(cid.to_string(), entity.key);
            }
        }
    }
    Ok(index)
}

/// A canonical id reads `role:level:world:construct`. Inside one construct the
/// tail is identical for every member, so the ROLE is what identifies a node.
fn short(id: &str) -> &str {
    id.split(':').next().unwrap_or(id)
}
