//! Injects the Lumina Prime house-alarm construct
//! (`fixtures/ontology/lumina-prime-house-alarm-v0.json`) into a SCRATCH store as
//! a real GRAPH OBJECT — attributed entities + relations, ONE atomic transaction,
//! ZERO non-canonical symbols — then INDEPENDENTLY reads the whole subgraph back
//! from a fresh reopen.
//!
//! WHY a scratch store, not the live store: the house-alarm needs to be PRESENT
//! in a graph so a later step can bound it with `cluster::cluster_from_space` and
//! a resolver can read its authored `alarm_atom_circuit` from the committed
//! `code` node. This bin proves that landing on a fresh, throwaway store booted
//! from `fixtures/genesis/minimal-genesis.json` (the same scratch pattern as
//! `bin/house_alarm_fire.rs`). It NEVER defaults to, and never writes to, the
//! live ontology store — the store dir is an OPTIONAL arg and defaults to a
//! unique temp dir.
//!
//! It is the same LOWER write layer as `inject_energy_pen` / `inject_construct`:
//! a hand-built write-set (InternSymbols + N PutEntity + M PutRelation), committed
//! at a tick boundary. It is NOT the permanent semantic-intent path.
//!
//! Difference from the live-store injectors: `minimal-genesis` interns only the
//! CAPITALIZED kernel vocabulary (`Space`/`Narrative`/`Thing`, `contains`, ...),
//! not the lowercase CANONICAL vocabulary the portable projection draws on
//! (`space`/`narrative`/`thing`/`metric`/`validation` + the remapped predicates).
//! So on a fresh scratch seed those canonical symbols are interned for the FIRST
//! time, atomically, via `UniverseCommand::InternSymbols` in the same transaction.
//! The conformance bar is unchanged and hard: `canonical::new_symbols` over the
//! injected symbols MUST be empty — every symbol is drawn from the shared
//! canonical vocabulary, never minted ad-hoc.
//!
//! Epistemic honesty:
//!   * Every member of the portable projection becomes one canonical entity
//!     carrying its original `canonical_id`; no member is silently dropped.
//!   * A relation whose source OR target is absent from the injected id-set is
//!     SKIPPED and reported (the parent-city `PART_OF` edge is dropped, not
//!     dangled — the city is not built in this scratch store).
//!   * A non-canonical authored predicate is a hard error, never minted.
//!   * Readback re-derives the `alarm_atom_circuit` from the committed `code`
//!     node and compares it BYTE-FOR-BYTE with the fixture — proving the circuit
//!     survived the round-trip, not just that some bytes landed.
//!   * Readback rebuilds an `AdjacencyIndex` and walks the space's incident edges
//!     to prove the space + its member relations are present and walkable, and
//!     runs `cluster_from_space` over the committed graph to report, honestly,
//!     what the downstream selector actually sees.
//!
//! Usage: `inject_house_alarm [scratch-store-dir]`
//!   scratch-store-dir defaults to a fresh unique dir under the system temp dir.
//!   NEVER pass the live store: this boots a fresh Genesis and needs an empty dir.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use universe_core::{EntityKey, RelationKey, Tick};
use universe_e2e::canonical::{canonical_predicate, entity_symbol, new_symbols};
use universe_e2e::cluster::{cluster_from_space, ClusterSelectionBudget};
use universe_query::{AdjacencyIndex, LocalGraph, LocalRelation};
use universe_store::{EntityRecord, RelationRecord, UniverseStore};
use universe_supervisor::Supervisor;
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

// The authored -> canonical predicate remap and the subtype-promotion rule are
// shared with the other injectors in `universe_e2e::canonical`, the single source
// of truth. An authored predicate absent from that table is a hard error here —
// the injector never mints a non-canonical predicate into the store.

/// The construct's `code` member id — the node whose content carries the
/// `alarm_atom_circuit` that a downstream resolver reads. Readback re-derives the
/// circuit from THIS committed node and compares it byte-for-byte with the
/// fixture.
const CODE_NODE_ID: &str = "code:l2:lumina-prime:house-alarm-v0";

// Disjoint key block for this construct on the scratch store. Kernel minimal
// genesis tops out at 0x12; the other one-off blocks use 0xB000/0xC000. The
// house-alarm takes the 0xD000 window (space + members) and 0xDD00 for edges.
const ENTITY_BASE: u128 = 0xD000;
const REL_BASE: u128 = 0xDD00;

fn main() {
    if let Err(error) = run() {
        eprintln!("INJECTION FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    // Store dir is the ONLY arg and is OPTIONAL. Default = fresh unique scratch
    // dir. Fixture + genesis resolve from the crate manifest so cwd is irrelevant.
    let store_dir_arg = env::args_os().nth(1).map(PathBuf::from);
    let store_dir = store_dir_arg
        .clone()
        .unwrap_or_else(default_scratch_store);
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let genesis = repo.join("fixtures/genesis/minimal-genesis.json");
    let fixture_path = repo.join("fixtures/ontology/lumina-prime-house-alarm-v0.json");
    println!("scratch store: {}", store_dir.display());
    println!("genesis      : {}", genesis.display());
    println!("fixture      : {}", fixture_path.display());

    // 0. Boot the scratch store from minimal-genesis (writes the Genesis
    // checkpoint), exactly like house_alarm_fire. Scoped so the Supervisor's
    // store handle is dropped before we reopen the store for the injection.
    fs::create_dir_all(&store_dir)?;
    {
        let supervisor = Supervisor::boot(&store_dir, &genesis)?;
        println!(
            "\nbooted scratch store from genesis: revision {} tick {} (state {:?})",
            supervisor.revision().0,
            supervisor.tick().0,
            supervisor.state()
        );
    }

    // 1. Parse the portable projection.
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&fixture_path)?)?;
    let root_id = doc
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("fixture has no top-level id")?
        .to_string();

    struct Node {
        id: String,
        node_type: String,
        subtype: String,
        content: serde_json::Value,
    }
    let node_from = |v: &serde_json::Value| -> Result<Node, Box<dyn Error>> {
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
    };
    let mut nodes: Vec<Node> = Vec::new();
    nodes.push(node_from(&doc)?);
    for member in doc
        .get("members")
        .and_then(|v| v.as_array())
        .ok_or("fixture has no members array")?
    {
        nodes.push(node_from(member)?);
    }

    // id -> EntityKey (ordered, deterministic).
    let mut id_to_key: BTreeMap<String, EntityKey> = BTreeMap::new();
    for (i, node) in nodes.iter().enumerate() {
        let key = EntityKey(ENTITY_BASE + i as u128);
        if id_to_key.insert(node.id.clone(), key).is_some() {
            return Err(format!("duplicate node id {}", node.id).into());
        }
    }
    let key_to_id: BTreeMap<EntityKey, String> =
        id_to_key.iter().map(|(id, key)| (*key, id.clone())).collect();
    let space_key = *id_to_key.get(&root_id).expect("root is indexed");
    println!(
        "\nconstruct: {root_id}\nnodes to inject: {} (space = {:#x}, key block {:#x}..)",
        nodes.len(),
        space_key.0,
        ENTITY_BASE
    );

    // 2. Partition relations: keep only those whose BOTH endpoints are injected.
    //    Remap EVERY authored predicate through the shared canonical table; an
    //    unmapped predicate is a hard error (fail-closed, never invented).
    struct Rel {
        source: EntityKey,
        target: EntityKey,
        predicate: String,
    }
    // The full authored -> (canonical, swap) table actually used, for the report.
    let mut predicate_table: BTreeMap<String, (&'static str, bool)> = BTreeMap::new();
    let mut kept: Vec<Rel> = Vec::new();
    let mut dropped: Vec<(String, String, String)> = Vec::new();
    for r in doc
        .get("relations")
        .and_then(|v| v.as_array())
        .unwrap_or(&Vec::new())
    {
        let source = r.get("source").and_then(|v| v.as_str()).unwrap_or("");
        let target = r.get("target").and_then(|v| v.as_str()).unwrap_or("");
        let authored = r.get("predicate").and_then(|v| v.as_str()).unwrap_or("");
        let (predicate, swap) = canonical_predicate(authored).ok_or_else(|| {
            format!(
                "authored predicate {authored} has no canonical mapping — refusing to mint a \
                 non-canonical symbol (fail-closed)"
            )
        })?;
        predicate_table.insert(authored.to_string(), (predicate, swap));
        match (id_to_key.get(source), id_to_key.get(target)) {
            (Some(s), Some(t)) => {
                let (src, tgt) = if swap { (*t, *s) } else { (*s, *t) };
                kept.push(Rel {
                    source: src,
                    target: tgt,
                    predicate: predicate.to_string(),
                });
            }
            _ => dropped.push((source.to_string(), predicate.to_string(), target.to_string())),
        }
    }

    println!("\n-- predicate -> canonical remap table (every authored predicate) --");
    for (authored, (canonical, swap)) in &predicate_table {
        println!(
            "  {authored:<15} -> {canonical:<12} {}",
            if *swap { "[swap dir]" } else { "" }
        );
    }
    println!(
        "\nrelations kept: {} | dropped (dangling): {}",
        kept.len(),
        dropped.len()
    );
    for (s, p, t) in &dropped {
        println!("  DROPPED  {s}  -[{p}]->  {t}   (endpoint not in injected set)");
    }

    // 3. Open the scratch store and replay to the authoritative snapshot.
    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;
    println!(
        "\nbase revision: {} | entities: {} | relations: {}",
        base_revision.0,
        snapshot.entities.len(),
        snapshot.relations.len()
    );
    // Guard: never overwrite an existing key.
    for node in &nodes {
        let key = id_to_key[&node.id];
        if snapshot.entities.iter().any(|e| e.key == key) {
            return Err(
                format!("entity key {:#x} ({}) already exists in the store", key.0, node.id).into(),
            );
        }
    }

    // 4. Requested symbols: node_type/subtype symbols + remapped predicates.
    //    Subtype-promotion rule shared via `canonical::entity_symbol`.
    let mut requested: Vec<String> = Vec::new();
    for node in &nodes {
        requested.push(entity_symbol(&node.node_type, &node.subtype));
    }
    for r in &kept {
        requested.push(r.predicate.clone());
    }
    requested.sort();
    requested.dedup();

    // CONFORMANCE GUARANTEE (the hard bar): the projection draws ONLY on the
    // shared canonical vocabulary. This is independent of whatever the fresh
    // scratch seed happens to pre-intern — a NON-canonical symbol is refused.
    let non_canonical = new_symbols(requested.iter().map(String::as_str));
    if !non_canonical.is_empty() {
        return Err(format!(
            "conformance violation: injection would use NON-canonical symbols {non_canonical:?} \
             (canonical::new_symbols must be empty)"
        )
        .into());
    }
    println!(
        "\ncanonical::new_symbols (non-canonical, must be empty): {:?}",
        non_canonical
    );

    // Plan interning against the scratch store. On a fresh minimal-genesis seed
    // the lowercase canonical vocabulary is not yet present, so these additions
    // are all canonical (asserted above) and are committed atomically in the same
    // transaction via InternSymbols.
    let plan = snapshot.plan_symbol_interning(&requested)?;
    println!(
        "canonical symbols interned into the fresh scratch seed: {} -> {:?}",
        plan.additions.len(),
        plan.additions
    );
    let sym = |name: &str| -> Result<u32, Box<dyn Error>> {
        plan.assignments
            .get(name)
            .copied()
            .ok_or_else(|| format!("symbol {name} was not planned").into())
    };

    // 5. Build the atomic write-set: intern the canonical symbols, then the space
    //    + members, then intra-graph edges — one atomic set.
    let mut commands = Vec::new();
    if !plan.additions.is_empty() {
        commands.push(UniverseCommand::InternSymbols {
            symbols: plan.additions.clone(),
        });
    }
    for node in &nodes {
        let content = serde_json::json!({
            "canonical_id": node.id,
            "node_type": node.node_type,
            "subtype": node.subtype,
            "content": node.content,
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
                key: RelationKey(REL_BASE + i as u128),
                generation: 0,
                source: r.source,
                target: r.target,
                predicate: sym(&r.predicate)?,
                content: None,
            },
        });
    }

    let command_count = commands.len();
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: "mutation:lumina-house-alarm:v0".to_string(),
        commands,
    };

    // 6. Prepare + commit as ONE atomic transaction at a tick boundary.
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!(
        "\ncommitted {command_count} commands as one atomic set ({} InternSymbols + {} PutEntity + {} PutRelation)",
        usize::from(!plan.additions.is_empty()),
        nodes.len(),
        kept.len()
    );
    println!("commit receipt: {receipt:?}");

    // 7. INDEPENDENT readback: fresh reopen from disk.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    println!("\n-- independent readback (fresh reopen) --");
    println!("revision advanced: {} -> {}", base_revision.0, after.revision.0);
    println!(
        "entities: {} | relations: {}",
        after.entities.len(),
        after.relations.len()
    );

    // 7a. Every injected node present, by key + canonical_id.
    for node in &nodes {
        let key = id_to_key[&node.id];
        let entity = after
            .entities
            .iter()
            .find(|e| e.key == key)
            .ok_or_else(|| format!("injected node {} ({:#x}) not found on readback", node.id, key.0))?;
        let content = fresh.read_content(
            entity
                .content
                .as_ref()
                .ok_or_else(|| format!("node {} has no content", node.id))?,
        )?;
        let canonical = content
            .get("canonical_id")
            .and_then(|v| v.as_str())
            .unwrap_or("(none)");
        if canonical != node.id {
            return Err(
                format!("canonical_id mismatch for {:#x}: {} != {}", key.0, canonical, node.id).into(),
            );
        }
    }
    println!("all {} injected nodes read back with matching canonical_id", nodes.len());

    // 7b. Every kept relation present by exact (source, target, predicate).
    for r in &kept {
        let sym_id = sym(&r.predicate)?;
        let present = after
            .relations
            .iter()
            .any(|x| x.source == r.source && x.target == r.target && x.predicate == sym_id);
        if !present {
            return Err(format!(
                "kept relation {:#x} -[{}]-> {:#x} missing on readback",
                r.source.0, r.predicate, r.target.0
            )
            .into());
        }
    }
    println!("all {} intra-graph relations read back", kept.len());

    // 7c. Deep circuit round-trip: re-derive the `alarm_atom_circuit` from the
    // COMMITTED `code` node and compare it BYTE-FOR-BYTE with the fixture. This
    // proves the authored circuit a downstream resolver reads survived intact.
    let original_circuit = nodes
        .iter()
        .find(|n| n.id == CODE_NODE_ID)
        .ok_or("code node absent from fixture")?
        .content
        .get("alarm_atom_circuit")
        .cloned()
        .ok_or("fixture code node has no alarm_atom_circuit")?;
    let code_key = *id_to_key
        .get(CODE_NODE_ID)
        .ok_or("code node id absent from fixture")?;
    let code_entity = after
        .entities
        .iter()
        .find(|e| e.key == code_key)
        .ok_or("code node not found on readback")?;
    let code_content =
        fresh.read_content(code_entity.content.as_ref().ok_or("code node has no content")?)?;
    let readback_circuit = code_content
        .pointer("/content/alarm_atom_circuit")
        .cloned()
        .ok_or("committed code node has no content.alarm_atom_circuit")?;

    let original_bytes = serde_json::to_vec(&original_circuit)?;
    let readback_bytes = serde_json::to_vec(&readback_circuit)?;
    if original_circuit != readback_circuit || original_bytes != readback_bytes {
        return Err("alarm_atom_circuit did NOT round-trip byte-faithfully".into());
    }
    let atom_count = readback_circuit
        .pointer("/atoms")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let bond_count = readback_circuit
        .pointer("/bonds")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let trigger_atom = readback_circuit
        .pointer("/trigger_atom")
        .and_then(|v| v.as_str())
        .unwrap_or("(none)");
    let deposit_bond = readback_circuit
        .pointer("/deposit_bond")
        .and_then(|v| v.as_str())
        .unwrap_or("(none)");
    println!(
        "alarm_atom_circuit round-tripped byte-faithful: {} bytes, {} atoms, {} bonds \
         (trigger_atom={trigger_atom}, deposit_bond={deposit_bond})",
        readback_bytes.len(),
        atom_count,
        bond_count
    );

    // 7d. Walkability: rebuild an AdjacencyIndex from the committed graph (the
    // same shape cluster_from_space builds) and prove the space + its member
    // relations are present and walkable. The construct's members attach to the
    // space via role predicates (IMPLEMENTS/DEFINES/MOTIVATES), so we walk the
    // space's incident edges and confirm they reach injected members.
    let index = AdjacencyIndex::from_parts(
        after.entities.iter().map(|e| e.key),
        after.relations.iter().map(|r| LocalRelation {
            key: r.key,
            source: r.source,
            target: r.target,
        }),
    );
    if !index.contains(space_key) {
        return Err("space node absent from the committed adjacency index".into());
    }
    let member_keys: BTreeSet<EntityKey> = nodes
        .iter()
        .filter(|n| n.id != root_id)
        .map(|n| id_to_key[&n.id])
        .collect();
    let mut reached: BTreeSet<EntityKey> = BTreeSet::new();
    for relation in index.adjacent(space_key) {
        let other = if relation.source == space_key {
            relation.target
        } else {
            relation.source
        };
        if member_keys.contains(&other) {
            reached.insert(other);
        }
    }
    if reached.is_empty() {
        return Err("space node has no walkable relation to any injected member".into());
    }
    println!(
        "\n-- walkability (AdjacencyIndex over the committed graph) --"
    );
    println!("space {:#x} present and walkable; incident edges to members:", space_key.0);
    for relation in after
        .relations
        .iter()
        .filter(|r| r.source == space_key || r.target == space_key)
    {
        let predicate = after
            .symbols
            .get(relation.predicate as usize)
            .cloned()
            .unwrap_or_else(|| format!("#{}", relation.predicate));
        let src = key_to_id
            .get(&relation.source)
            .cloned()
            .unwrap_or_else(|| format!("{:#x}", relation.source.0));
        let tgt = key_to_id
            .get(&relation.target)
            .cloned()
            .unwrap_or_else(|| format!("{:#x}", relation.target.0));
        println!("  {}  -[{predicate}]->  {}", role(&src), role(&tgt));
    }
    println!(
        "space reaches {} member(s) directly: {:?}",
        reached.len(),
        reached
            .iter()
            .map(|k| role(key_to_id.get(k).map(String::as_str).unwrap_or("?")).to_string())
            .collect::<Vec<_>>()
    );

    // 7e. Run the ACTUAL downstream selector over the committed graph and report,
    // honestly, what it sees. `cluster_from_space` walks ONLY PART_OF/APPLIES_IN
    // membership predicates; this construct's single PART_OF edge (space -> city)
    // was dropped as dangling and its members attach by role predicates, so the
    // selector finds 0 member atoms here. Reporting the real result (not a
    // fabricated PART_OF) keeps the downstream gap visible.
    let selection = cluster_from_space(
        &after,
        space_key,
        ClusterSelectionBudget {
            max_atoms: 64,
            max_bonds: 128,
        },
    )
    .map_err(|error| format!("cluster_from_space failed: {error:?}"))?;
    println!(
        "cluster_from_space(space): status {:?}, {} member atom(s), {} bond(s) \
         (PART_OF/APPLIES_IN only — this construct's members attach by role predicates)",
        selection.status, selection.member_count, selection.bond_count
    );

    // 8. Manifest.
    println!("\n===== MANIFEST =====");
    println!("construct              : {root_id}");
    println!("scratch store          : {}", store_dir.display());
    println!("entities injected      : {}", nodes.len());
    println!("relations injected      : {} (dropped dangling: {})", kept.len(), dropped.len());
    println!("canonical::new_symbols : {:?}  (empty => ZERO non-canonical symbols)", non_canonical);
    println!("symbols interned       : {} (all canonical, fresh scratch seed)", plan.additions.len());
    println!("circuit round-trip     : byte-faithful ({} bytes)", readback_bytes.len());
    println!("space walkable         : yes ({} member(s) reached directly)", reached.len());
    println!("revision               : {} -> {}", base_revision.0, after.revision.0);
    println!("predicate -> canonical table:");
    for (authored, (canonical, swap)) in &predicate_table {
        println!(
            "  {authored:<15} -> {canonical:<12}{}",
            if *swap { " [swap dir]" } else { "" }
        );
    }
    println!("\nRESULT: injected the Lumina Prime house-alarm construct ({} nodes, {} intra-graph relations)", nodes.len(), kept.len());
    println!("        into a SCRATCH store as ONE atomic transaction, drew ZERO non-canonical symbols,");
    println!("        and independently read the whole subgraph back — including the byte-faithful");
    println!("        alarm_atom_circuit and the walkable space -> member edges.");
    println!("        graph_status: WRITTEN (scratch). wiring/runtime/health remain not_wired / not_running / not_measured.");

    // Best-effort scratch cleanup (only when we created a default temp dir). The
    // independent readback above already reopened from disk, so persistence is
    // proven before cleanup.
    if store_dir_arg.is_none() {
        let _ = fs::remove_dir_all(&store_dir);
    }
    Ok(())
}

fn default_scratch_store() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    env::temp_dir().join(format!("inject-house-alarm-{}-{nanos}", std::process::id()))
}

/// Role head of a canonical id (the segment BEFORE the first colon) for compact
/// logging. Distinct members of this construct share the `house-alarm-v0` TAIL
/// (`code:...:house-alarm-v0`, `implementation:...:house-alarm-v0`, ...), so the
/// head is what disambiguates them — `code`, `implementation`, `objective`.
fn role(id: &str) -> &str {
    id.split(':').next().unwrap_or(id)
}
