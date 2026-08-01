//! Sets down the Lumina Prime Memory Stele in the LIVE canonical store as ONE
//! additive, atomic, attributed transaction — then walks away and reads it back
//! through a fresh reopen that trusts nothing the write said about itself.
//!
//! It is a sibling of `inject_orientation_beacon` / `inject_energy_pen` /
//! `inject_construct`, and differs from them in exactly three ways, each of
//! which exists because setting a stele down surfaced a hazard the earlier
//! injectors did not have to face:
//!
//!   1. THE PREDICATE REMAP IS UNIVERSE DATA, NOT A CONSTANT IN THIS BINARY.
//!      The authored spine vocabulary (GOVERNS, SHAPES, EVIDENCES, CONFORMS_TO,
//!      …) is encoded into canonical predicates by the `spine_encoding.
//!      predicate_remap` table carried on the construct-validity protocol node
//!      IN THE STORE. This bin READS that table at injection time. If the
//!      protocol node is absent, or carries no encoding table, the injection
//!      REFUSES: there is no built-in fallback, because a fallback would
//!      silently substitute a native default for missing Universe state, and
//!      the remap would drift per binary exactly as it once drifted per file.
//!      A predicate absent from the table is admitted only if it is ITSELF an
//!      already-interned canonical symbol (identity encoding); anything else is
//!      a hard error and is never minted.
//!
//!   2. EXTERNAL ENDPOINTS RESOLVE BY CANONICAL ID.
//!      The earlier injectors dropped any relation with an endpoint outside the
//!      fixture, which silently discarded the construct's conformance edge. Here
//!      a relation endpoint that is not in the fixture is looked up by
//!      `canonical_id` in the committed store; found, it is wired; absent, it is
//!      SKIPPED and reported by name. This is how `CONFORMS_TO
//!      construct-validity-v0` and `DEPENDS_ON orientation-beacon-v0` actually
//!      land instead of quietly vanishing.
//!
//!   3. BOUNDED RETRY AGAINST A CONCURRENT WRITER.
//!      Another session writes this same store (the MCP arrival path embodies
//!      visitors as durable inhabitants). A write-set prepared against a
//!      snapshot that has since moved commits an event whose `revision` no
//!      longer chains, and `events.jsonl` stops replaying from that point on —
//!      which corrupts every later reader, not just this one. So the whole plan
//!      (reopen, replay, resolve, plan symbols, append content, prepare, commit)
//!      is rebuilt from scratch on each attempt, up to `MAX_ATTEMPTS`, and a
//!      `RevisionConflict` retries rather than failing or forcing.
//!
//! Honesty:
//!   * Every member of the portable projection becomes one canonical entity
//!     carrying its original `canonical_id`; no member is silently dropped.
//!   * A clean injection interns ZERO new symbols. A non-empty additions set is
//!     a refusal, never a mint.
//!   * NO `built_position` / `HAS_POSITION` is written. The stele authors no
//!     coordinate: its place is solver-inferred, and the fixture says so.
//!   * NO `PART_OF` to any parent Space is written. Nothing in this store
//!     contains the stele, and inventing a parent would make the stele commit
//!     the very un-founded containment its own inscription records.
//!   * The final word is a readback from a FRESH reopen: the store is re-opened,
//!     the log re-replayed (which would fail outright if this commit broke the
//!     chain), and every node, every edge and the construct contract are
//!     re-derived from what is on disk.
//!
//! Usage: `inject_memory_stele [fixture.json] [store-dir]`
//!   fixture.json defaults to fixtures/ontology/lumina-prime-memory-stele-v0.json
//!   store-dir    defaults to artifacts/ontology-registry/current/store

use std::{
    collections::{hash_map::DefaultHasher, BTreeMap},
    env,
    error::Error,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use universe_core::{EntityKey, RelationKey, Tick, UniverseError};
use universe_e2e::canonical::entity_symbol;
use universe_store::{EntityRecord, RelationRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// The construct that carries the authored -> canonical spine encoding. Read
/// from the store by canonical id; never a key, never a constant table.
const PROTOCOL_ID: &str = "space:l2:mind-universe:construct-validity-v0";

/// A second writer shares this store. Rebuild and retry this many times before
/// giving up; never force, never write against a stale base revision.
const MAX_ATTEMPTS: usize = 6;

/// Who asked for this, and why. Carried on every entity so the stele's presence
/// in the city is attributable to a request rather than to whoever ran a binary.
const ATTRIBUTION_INTENT: &str =
    "Commemorate the accreted dormitory of Lumina Prime: hold, sealed and provenance-tagged, \
     the lore of why session bodies piled into a ball at the origin.";
const ATTRIBUTION_REQUESTED_BY: &str = "operator (nlr_ai), 2026-08-01";

fn main() {
    if let Err(error) = run() {
        eprintln!("STELE NOT SET DOWN: {error}");
        std::process::exit(1);
    }
}

/// One node of the portable projection.
struct Node {
    id: String,
    node_type: String,
    subtype: String,
    content: serde_json::Value,
}

/// One relation whose endpoints have been resolved to real keys.
struct Rel {
    source: EntityKey,
    target: EntityKey,
    predicate: String,
    label: String,
}

fn node_from(v: &serde_json::Value) -> Result<Node, Box<dyn Error>> {
    Ok(Node {
        id: v
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or("node without id")?
            .to_string(),
        node_type: v
            .get("node_type")
            .and_then(|x| x.as_str())
            .unwrap_or("thing")
            .to_string(),
        subtype: v
            .get("subtype")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        content: v.get("content").cloned().unwrap_or(serde_json::Value::Null),
    })
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let fixture_path = args.next().map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from("fixtures/ontology/lumina-prime-memory-stele-v0.json")
    });
    let store_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    println!("fixture  : {}", fixture_path.display());
    println!("store dir: {}", store_dir.display());

    // 1. Parse the portable projection: the stele Space itself, then every member.
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&fixture_path)?)?;
    let root_id = doc
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("fixture has no top-level id")?
        .to_string();
    let mut nodes: Vec<Node> = vec![node_from(&doc)?];
    for member in doc
        .get("members")
        .and_then(|v| v.as_array())
        .ok_or("fixture has no members array")?
    {
        nodes.push(node_from(member)?);
    }

    // A construct or nothing: the contract is checked BEFORE anything is written.
    let contract_kind = doc
        .pointer("/content/contractKind")
        .and_then(|v| v.as_str())
        .unwrap_or("(none)");
    if contract_kind != "construct" {
        return Err(format!("fixture contractKind is '{contract_kind}', expected 'construct'").into());
    }
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
    for role in REQUIRED_ROLES {
        if !nodes.iter().any(|n| n.subtype == *role) {
            return Err(format!("not a valid construct: missing role subtype '{role}'").into());
        }
    }
    // Reactive profile: the protocol requires the trigger anatomy and rearm
    // semantics. A reactive construct without a re-arm discipline is
    // self-excitation, so refuse it here rather than discover it in the field.
    let profile = doc
        .pointer("/content/execution_profile")
        .and_then(|v| v.as_str())
        .ok_or("fixture declares no execution_profile")?;
    if profile == "reactive"
        && !nodes.iter().any(|n| {
            n.subtype == "algorithm" && n.content.get("rearm_semantics").is_some()
        })
    {
        return Err("reactive profile declared without rearm_semantics on the algorithm".into());
    }
    println!("contract : {contract_kind} | execution_profile: {profile} | 12/12 roles present");

    // Fixture-specific disjoint key block, derived from the root id so distinct
    // constructs never collide (same scheme as inject_construct).
    let mut hasher = DefaultHasher::new();
    root_id.hash(&mut hasher);
    let entity_base: u128 = 0x0001_0000 + ((hasher.finish() as u128 & 0x0FFF) << 16);
    let rel_base: u128 = entity_base + 0x8000;

    let mut id_to_key: BTreeMap<String, EntityKey> = BTreeMap::new();
    for (i, node) in nodes.iter().enumerate() {
        let key = EntityKey(entity_base + i as u128);
        if id_to_key.insert(node.id.clone(), key).is_some() {
            return Err(format!("duplicate node id {}", node.id).into());
        }
    }
    println!(
        "construct: {root_id}\nnodes to set down: {} (stele = {:#x}, key block {:#x}..)",
        nodes.len(),
        id_to_key[&root_id].0,
        entity_base
    );

    let authored_relations: Vec<serde_json::Value> = doc
        .get("relations")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 2. Attempt loop. EVERYTHING is rebuilt per attempt: another writer may have
    //    interned symbols, added entities, or moved the revision since the last try.
    for attempt in 1..=MAX_ATTEMPTS {
        println!("\n=== attempt {attempt}/{MAX_ATTEMPTS} ===");
        match attempt_injection(
            &store_dir,
            &root_id,
            &nodes,
            &id_to_key,
            &authored_relations,
            rel_base,
        ) {
            Ok(outcome) => {
                verify_independently(&store_dir, &root_id, &nodes, &id_to_key, &outcome)?;
                return Ok(());
            }
            Err(error) => {
                // A revision conflict is the concurrent writer, not a fault:
                // rebuild against the world as it now is. Anything else is real.
                let conflict = error
                    .downcast_ref::<UniverseError>()
                    .map(|e| matches!(e, UniverseError::RevisionConflict { .. }))
                    .unwrap_or(false);
                if conflict && attempt < MAX_ATTEMPTS {
                    println!("  revision moved under us ({error}) — rebuilding against the new snapshot");
                    continue;
                }
                return Err(error);
            }
        }
    }
    Err(format!("gave up after {MAX_ATTEMPTS} attempts: the store never held still long enough").into())
}

/// What one successful attempt committed, for the independent reader to check.
struct Outcome {
    base_revision: u64,
    committed_revision: u64,
    kept: Vec<Rel>,
}

fn attempt_injection(
    store_dir: &Path,
    root_id: &str,
    nodes: &[Node],
    id_to_key: &BTreeMap<String, EntityKey>,
    authored_relations: &[serde_json::Value],
    rel_base: u128,
) -> Result<Outcome, Box<dyn Error>> {
    // 2a. Open and replay to the authoritative snapshot.
    let store = UniverseStore::open(store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;
    println!(
        "base revision: {} | entities: {} | relations: {}",
        base_revision.0,
        snapshot.entities.len(),
        snapshot.relations.len()
    );

    // 2b. Index canonical_id -> key over the COMMITTED store, so external
    //     endpoints (the protocol, the beacon) resolve by name and never by key.
    let mut store_ids: BTreeMap<String, EntityKey> = BTreeMap::new();
    for entity in &snapshot.entities {
        if let Some(ptr) = entity.content.as_ref() {
            if let Ok(content) = store.read_content(ptr) {
                if let Some(cid) = content.get("canonical_id").and_then(|v| v.as_str()) {
                    store_ids.insert(cid.to_string(), entity.key);
                }
            }
        }
    }

    // 2c. Read the spine encoding FROM THE STORE. No fallback: a missing table
    //     is a refusal, because inventing one here is how the remap drifts.
    let protocol_key = store_ids
        .get(PROTOCOL_ID)
        .ok_or_else(|| format!("the construct protocol {PROTOCOL_ID} is not in this store — refusing to guess an encoding"))?;
    let protocol_entity = snapshot
        .entities
        .iter()
        .find(|e| e.key == *protocol_key)
        .ok_or("protocol node vanished between index and read")?;
    let protocol_content = store.read_content(
        protocol_entity
            .content
            .as_ref()
            .ok_or("the protocol node carries no content")?,
    )?;
    let remap = protocol_content
        .pointer("/content/spine_encoding/predicate_remap")
        .and_then(|v| v.as_object())
        .ok_or("the protocol node carries no spine_encoding.predicate_remap — refusing to guess an encoding")?;
    println!(
        "spine encoding: {} entries, read from {PROTOCOL_ID} in the store (not from this binary)",
        remap.len()
    );

    // The encoder: authored predicate -> (canonical predicate, swap direction).
    // Table first; identity for an authored predicate that is ALREADY a canonical
    // interned symbol; hard error otherwise.
    let encode = |authored: &str| -> Result<(String, bool), Box<dyn Error>> {
        if let Some(entry) = remap.get(authored) {
            let canonical = entry
                .get("canonical")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("spine_encoding entry for {authored} has no canonical predicate"))?;
            let swap = entry.get("swap").and_then(|v| v.as_bool()).unwrap_or(false);
            return Ok((canonical.to_string(), swap));
        }
        if snapshot.symbol_id(authored).is_some() {
            return Ok((authored.to_string(), false));
        }
        Err(format!(
            "authored predicate {authored} is neither in the store's spine encoding nor an interned canonical symbol — refusing to mint it"
        )
        .into())
    };

    // 2d. Resolve relations. Endpoint inside the fixture, else by canonical_id in
    //     the store, else SKIPPED and named.
    let mut kept: Vec<Rel> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    for r in authored_relations {
        let source = r.get("source").and_then(|v| v.as_str()).unwrap_or("");
        let target = r.get("target").and_then(|v| v.as_str()).unwrap_or("");
        let authored = r.get("predicate").and_then(|v| v.as_str()).unwrap_or("");
        let (canonical, swap) = encode(authored)?;
        let resolve = |id: &str| id_to_key.get(id).or_else(|| store_ids.get(id)).copied();
        match (resolve(source), resolve(target)) {
            (Some(s), Some(t)) => {
                let (src, tgt) = if swap { (t, s) } else { (s, t) };
                let external = !id_to_key.contains_key(source) || !id_to_key.contains_key(target);
                if authored != canonical || external {
                    println!(
                        "  {:<8} {authored:<16} -> {canonical:<18}{}  ({} -> {})",
                        if external { "EXTERNAL" } else { "REMAP" },
                        if swap { " [swap dir]" } else { "" },
                        short(source),
                        short(target)
                    );
                }
                kept.push(Rel {
                    source: src,
                    target: tgt,
                    predicate: canonical,
                    label: format!("{} -[{}]-> {}", short(source), authored, short(target)),
                });
            }
            _ => dropped.push(format!("{source}  -[{authored}]->  {target}")),
        }
    }
    println!("relations resolved: {} | skipped (endpoint absent): {}", kept.len(), dropped.len());
    for d in &dropped {
        println!("  SKIPPED  {d}   (endpoint neither in the fixture nor in the store)");
    }

    // 2e. Never overwrite: an existing key in our block is a hard error.
    for node in nodes {
        let key = id_to_key[&node.id];
        if snapshot.entities.iter().any(|e| e.key == key) {
            return Err(format!("entity key {:#x} ({}) already exists in the store", key.0, node.id).into());
        }
    }

    // 2f. Symbol budget. A clean set-down interns NOTHING.
    let mut requested: Vec<String> = nodes
        .iter()
        .map(|n| entity_symbol(&n.node_type, &n.subtype))
        .collect();
    requested.extend(kept.iter().map(|r| r.predicate.clone()));
    requested.sort();
    requested.dedup();
    let plan = snapshot.plan_symbol_interning(&requested)?;
    if !plan.additions.is_empty() {
        return Err(format!(
            "conformance violation: this would intern new symbols {:?} (expected none)",
            plan.additions
        )
        .into());
    }
    println!("symbol conformance: 0 new symbols interned (all canonical / pre-existing)");
    let sym = |name: &str| -> Result<u32, Box<dyn Error>> {
        plan.assignments
            .get(name)
            .copied()
            .ok_or_else(|| format!("symbol {name} was not planned").into())
    };

    // 2g. Build ONE write-set: every node, then every edge. Each entity carries
    //     its attribution, so the stele's presence answers "who asked, and why".
    let mut commands = Vec::new();
    for node in nodes {
        let content = serde_json::json!({
            "canonical_id": node.id,
            "node_type": node.node_type,
            "subtype": node.subtype,
            "content": node.content,
            "attribution": {
                "requested_by": ATTRIBUTION_REQUESTED_BY,
                "intent": ATTRIBUTION_INTENT,
                "written_by": "crates/universe-e2e/src/bin/inject_memory_stele.rs",
                "base_revision": base_revision.0,
            },
        });
        commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: id_to_key[&node.id],
                generation: 0,
                symbol: sym(&entity_symbol(&node.node_type, &node.subtype))?,
                content: Some(store.append_content(&content)?),
            },
        });
    }
    for (i, r) in kept.iter().enumerate() {
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: RelationKey(rel_base + i as u128),
                generation: 0,
                source: r.source,
                target: r.target,
                predicate: sym(&r.predicate)?,
                content: None,
            },
        });
    }
    let command_count = commands.len();

    // 2h. Prepare + commit as ONE atomic transaction at a tick boundary.
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: format!("mutation:memory-stele:set-down:{root_id}"),
        commands,
    };
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("committed {command_count} commands as ONE atomic set");
    println!("commit receipt: {receipt:?}");

    Ok(Outcome {
        base_revision: base_revision.0,
        committed_revision: snapshot.revision.0,
        kept,
    })
}

/// The only thing that counts. Re-open the store from disk, re-replay the whole
/// log — which fails outright if this commit broke the chain — and re-derive
/// every claim from what is actually there. Nothing the write said about itself
/// is trusted, including the receipt.
fn verify_independently(
    store_dir: &Path,
    root_id: &str,
    nodes: &[Node],
    id_to_key: &BTreeMap<String, EntityKey>,
    outcome: &Outcome,
) -> Result<(), Box<dyn Error>> {
    println!("\n-- independent readback (fresh reopen, full log replay) --");
    let fresh = UniverseStore::open(store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    println!(
        "log replays clean to revision {} (base was {}, this commit made {})",
        after.revision.0, outcome.base_revision, outcome.committed_revision
    );
    println!("entities: {} | relations: {}", after.entities.len(), after.relations.len());

    for node in nodes {
        let key = id_to_key[&node.id];
        let entity = after
            .entities
            .iter()
            .find(|e| e.key == key)
            .ok_or_else(|| format!("node {} ({:#x}) is not there on readback", node.id, key.0))?;
        let content = fresh.read_content(
            entity
                .content
                .as_ref()
                .ok_or_else(|| format!("node {} carries no content", node.id))?,
        )?;
        let canonical = content.get("canonical_id").and_then(|v| v.as_str()).unwrap_or("(none)");
        if canonical != node.id {
            return Err(format!("canonical_id mismatch at {:#x}: {canonical} != {}", key.0, node.id).into());
        }
        if content.pointer("/attribution/requested_by").is_none() {
            return Err(format!("node {} came back without its attribution", node.id).into());
        }
    }
    println!("all {} nodes read back, each with matching canonical_id and attribution", nodes.len());

    for r in &outcome.kept {
        let predicate = after
            .symbol_id(&r.predicate)
            .ok_or_else(|| format!("predicate {} is not interned on readback", r.predicate))?;
        let present = after
            .relations
            .iter()
            .any(|x| x.source == r.source && x.target == r.target && x.predicate == predicate);
        if !present {
            return Err(format!("edge {} is missing on readback", r.label).into());
        }
    }
    println!("all {} edges read back", outcome.kept.len());

    // The construct contract, and the inscription, re-derived from the store —
    // not from the fixture file, which the store does not depend on.
    let stele = after
        .entities
        .iter()
        .find(|e| e.key == id_to_key[root_id])
        .ok_or("the stele node is not there on readback")?;
    let stele_content = fresh.read_content(stele.content.as_ref().ok_or("stele carries no content")?)?;
    let kind = stele_content
        .pointer("/content/contractKind")
        .and_then(|v| v.as_str())
        .unwrap_or("(none)");
    if kind != "construct" {
        return Err(format!("stele contractKind reads back as '{kind}', expected 'construct'").into());
    }
    if stele_content.pointer("/content/stance/placement").and_then(|v| v.as_str()) != Some("none_authored") {
        return Err("the stele read back claiming a placement it was not given".into());
    }

    let inscription_key = nodes
        .iter()
        .find(|n| n.subtype == "inscription")
        .map(|n| id_to_key[&n.id])
        .ok_or("no inscription among the members")?;
    let inscription = fresh.read_content(
        after
            .entities
            .iter()
            .find(|e| e.key == inscription_key)
            .and_then(|e| e.content.as_ref())
            .ok_or("the inscription is not there on readback")?,
    )?;
    let claims = inscription
        .pointer("/content/claims")
        .and_then(|v| v.as_array())
        .ok_or("the inscription read back with no claims")?;
    let mut by_status: BTreeMap<&str, usize> = BTreeMap::new();
    for claim in claims {
        let status = claim
            .get("epistemic_status")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                format!(
                    "claim {} read back with NO epistemic status — the one thing this stele must never do",
                    claim.get("claim_id").and_then(|v| v.as_str()).unwrap_or("?")
                )
            })?;
        if claim.get("observed_at_revision").is_none() {
            return Err("a claim read back without the revision it was observed at".into());
        }
        *by_status.entry(status).or_default() += 1;
    }
    let contested = inscription
        .pointer("/content/contested_claims")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    for c in inscription
        .pointer("/content/contested_claims")
        .and_then(|v| v.as_array())
        .unwrap_or(&Vec::new())
    {
        if c.get("as_received").is_none() || c.get("as_measured").is_none() {
            return Err("a contested claim read back with only one side".into());
        }
    }
    println!(
        "inscription read back: {} claims ({}), {contested} contested — each side of each contest present",
        claims.len(),
        by_status
            .iter()
            .map(|(s, n)| format!("{n} {s}"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    println!("\nRESULT: the Memory Stele stands in the LIVE store.");
    println!(
        "        {} nodes + {} edges, ONE atomic attributed transaction, 0 new symbols, revision {} -> {}.",
        nodes.len(),
        outcome.kept.len(),
        outcome.base_revision,
        outcome.committed_revision
    );
    println!("        No Built pose. No parent PART_OF. Nothing contains it, and it says so.");
    println!(
        "        graph_status: WRITTEN. wiring/runtime/seal/health remain not_wired / not_running / unsealed / not_measured —"
    );
    println!("        no collider is in any field, so nothing can cross it, so the stele reveals nothing yet.");
    Ok(())
}

/// Short tail of a canonical id, for compact logging.
fn short(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}
