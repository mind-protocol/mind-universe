//! Chicken-egg bootstrap of the MutationBond: land ONE `Built` position on the
//! LIVE store as a single atomic SET (intern + 3 PutEntity + 3 PutRelation),
//! then INDEPENDENTLY read it back from a fresh reopen.
//!
//! This proves the layout-built pivot: a node's position becomes an authored,
//! provenance-carrying `Built` fact (with a construction Moment as its evidence)
//! instead of a value the layout engine re-derives. The forgery check —
//! `provenance:"built"` with NO `CONSTRUCTED_BY` Moment is rejected — is the
//! observer discipline the real MutationBond will enforce.
//!
//! It is the write path's LOWER layer (hand-built write-set): the seed the
//! generic translator generalizes, NOT the permanent way to mutate.
//!
//! Usage: `place_built_position [store-dir]`
//!   store-dir defaults to `artifacts/ontology-registry/current/store`.

use std::{env, error::Error, path::PathBuf};

use universe_core::{EntityKey, RelationKey, Tick};
use universe_store::{EntityRecord, RelationRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

// Existing canonical anchor we place a position FOR (the protocol manifest).
const PLACED_NODE: u128 = 0x1000;
// Disjoint keys for the new construction (far above the seed's 0x1000-0x2xxx).
const POSITION: u128 = 0x9000;
const CONSTRUCTION: u128 = 0x9001;
const JUSTIFICATION: u128 = 0x9002;
const REL_HAS_POSITION: u128 = 0x9100;
const REL_CONSTRUCTED_BY: u128 = 0x9101;
const REL_JUSTIFIED_BY: u128 = 0x9102;

fn main() {
    if let Err(error) = run() {
        eprintln!("PLACEMENT FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let store_dir = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    println!("store dir: {}", store_dir.display());

    // 1. Open the LIVE store and replay to the authoritative snapshot.
    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;
    println!(
        "base revision: {} | entities: {} | relations: {}",
        base_revision.0,
        snapshot.entities.len(),
        snapshot.relations.len()
    );

    // The node we place a position for must already exist.
    if !snapshot.entities.iter().any(|e| e.key == EntityKey(PLACED_NODE)) {
        return Err(format!("placed_node {:#x} does not exist in the store", PLACED_NODE).into());
    }

    // 2. Plan the new overlay symbols (deterministic ids from the same event).
    let requested: Vec<String> = [
        "built_position",
        "placement_construction",
        "placement_justification",
        "HAS_POSITION",
        "CONSTRUCTED_BY",
        "JUSTIFIED_BY",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let plan = snapshot.plan_symbol_interning(&requested)?;
    let sym = |name: &str| -> Result<u32, Box<dyn Error>> {
        plan.assignments
            .get(name)
            .copied()
            .ok_or_else(|| format!("symbol {name} was not planned").into())
    };

    // 3. Build the atomic SET: intern symbols + 3 PutEntity + 3 PutRelation.
    let position_content = serde_json::json!({
        "kind": "built_position",
        "space": "salle",
        "placed_node": format!("{:#x}", PLACED_NODE),
        "x": 12.0, "y": 0.0, "z": -4.0,
        "provenance": "built"
    });
    let construction_content = serde_json::json!({
        "kind": "placement_construction",
        "authored_by": "a.inchauspe@digitalkin.ai",
        "base_revision": base_revision.0,
        "note": "first built placement — MutationBond chicken-egg bootstrap (layout-built pivot)"
    });
    let justification_content = serde_json::json!({
        "kind": "placement_justification",
        "statement": "Placed by hand so this position is Built (authored + provenance), not Derived by the layout engine."
    });

    let mut commands = Vec::new();
    if !plan.additions.is_empty() {
        commands.push(UniverseCommand::InternSymbols {
            symbols: plan.additions.clone(),
        });
    }
    commands.push(UniverseCommand::PutEntity {
        entity: EntityRecord {
            key: EntityKey(POSITION),
            generation: 0,
            symbol: sym("built_position")?,
            content: Some(store.append_content(&position_content)?),
        },
    });
    commands.push(UniverseCommand::PutEntity {
        entity: EntityRecord {
            key: EntityKey(CONSTRUCTION),
            generation: 0,
            symbol: sym("placement_construction")?,
            content: Some(store.append_content(&construction_content)?),
        },
    });
    commands.push(UniverseCommand::PutEntity {
        entity: EntityRecord {
            key: EntityKey(JUSTIFICATION),
            generation: 0,
            symbol: sym("placement_justification")?,
            content: Some(store.append_content(&justification_content)?),
        },
    });
    commands.push(UniverseCommand::PutRelation {
        relation: RelationRecord {
            key: RelationKey(REL_HAS_POSITION),
            generation: 0,
            source: EntityKey(PLACED_NODE),
            target: EntityKey(POSITION),
            predicate: sym("HAS_POSITION")?,
            content: None,
        },
    });
    commands.push(UniverseCommand::PutRelation {
        relation: RelationRecord {
            key: RelationKey(REL_CONSTRUCTED_BY),
            generation: 0,
            source: EntityKey(POSITION),
            target: EntityKey(CONSTRUCTION),
            predicate: sym("CONSTRUCTED_BY")?,
            content: None,
        },
    });
    commands.push(UniverseCommand::PutRelation {
        relation: RelationRecord {
            key: RelationKey(REL_JUSTIFIED_BY),
            generation: 0,
            source: EntityKey(POSITION),
            target: EntityKey(JUSTIFICATION),
            predicate: sym("JUSTIFIED_BY")?,
            content: None,
        },
    });

    let command_count = commands.len();
    let write_set = UniverseWriteSet {
        base_revision,
        // Deterministic → re-running yields AlreadyCommitted (idempotent).
        idempotency_key: "mutation:built-placement:v0".to_string(),
        causal_ancestry: vec!["changeset:built-placement-v0".to_string()],
        commands,
    };

    // 4. Prepare + commit the whole set as ONE atomic transaction.
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("committed {command_count} commands as one atomic set");
    println!("commit receipt: {receipt:?}");

    // 5. INDEPENDENT readback: fresh reopen from disk (never trust the installer's snapshot).
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    println!("\n-- independent readback (fresh reopen) --");
    println!("revision advanced: {} -> {}", base_revision.0, after.revision.0);

    let position = after
        .entities
        .iter()
        .find(|e| e.key == EntityKey(POSITION))
        .ok_or("built_position entity not found on readback")?;
    let content = fresh.read_content(
        position
            .content
            .as_ref()
            .ok_or("built_position has no content")?,
    )?;
    println!("built_position {:#x} content: {}", POSITION, content);

    let has_pos = after.relations.iter().any(|r| {
        r.key == RelationKey(REL_HAS_POSITION)
            && r.source == EntityKey(PLACED_NODE)
            && r.target == EntityKey(POSITION)
    });
    let constructed_by = after.relations.iter().any(|r| {
        r.key == RelationKey(REL_CONSTRUCTED_BY)
            && r.source == EntityKey(POSITION)
            && r.target == EntityKey(CONSTRUCTION)
    });
    let justified_by = after.relations.iter().any(|r| {
        r.key == RelationKey(REL_JUSTIFIED_BY)
            && r.source == EntityKey(POSITION)
            && r.target == EntityKey(JUSTIFICATION)
    });
    println!(
        "edges — HAS_POSITION: {has_pos} | CONSTRUCTED_BY: {constructed_by} | JUSTIFIED_BY: {justified_by}"
    );

    // The observer's forgery check: a Built position without a construction
    // Moment is unfalsifiable and must be rejected.
    let provenance = content
        .get("provenance")
        .and_then(|v| v.as_str())
        .unwrap_or("(none)");
    if provenance == "built" && !constructed_by {
        return Err("FORGERY: provenance=built with no CONSTRUCTED_BY construction Moment".into());
    }
    if !(has_pos && constructed_by && justified_by) {
        return Err("readback is missing one of the placement edges".into());
    }

    let moment = after
        .entities
        .iter()
        .find(|e| e.key == EntityKey(CONSTRUCTION))
        .ok_or("construction Moment not found on readback")?;
    let moment_content = fresh.read_content(
        moment
            .content
            .as_ref()
            .ok_or("construction Moment has no content")?,
    )?;
    println!("construction Moment {:#x} content: {}", CONSTRUCTION, moment_content);

    println!(
        "\nRESULT: position {:#x} is BUILT (provenance={provenance}) with an authored construction Moment, read back from a fresh store — not layout-derived.",
        POSITION
    );
    Ok(())
}
