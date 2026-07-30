//! Place a Built position on a chosen (RENDERED) node, so the layout-built pivot
//! is visible in the real city. Same atomic-set + readback pattern as
//! `place_built_position`, but the placed node and coordinates are arguments and
//! the keys are disjoint (`0x9010+`) so it composes with the first placement.
//!
//! Usage: `place_built_on [store-dir] [placed_node_hex] [x] [y] [z]`
//!   defaults: current store, placed_node 0x1100 (canonical `actor`), [50,0,50].

use std::{env, error::Error, path::PathBuf};

use universe_core::{EntityKey, RelationKey, Tick};
use universe_store::{EntityRecord, RelationRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

const POSITION: u128 = 0x9010;
const CONSTRUCTION: u128 = 0x9011;
const JUSTIFICATION: u128 = 0x9012;
const REL_HAS_POSITION: u128 = 0x9110;
const REL_CONSTRUCTED_BY: u128 = 0x9111;
const REL_JUSTIFIED_BY: u128 = 0x9112;

fn main() {
    if let Err(error) = run() {
        eprintln!("PLACEMENT FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let store_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    let placed_node: u128 = args
        .next()
        .map(|a| u128::from_str_radix(a.trim_start_matches("0x"), 16))
        .transpose()?
        .unwrap_or(0x1100);
    let x: f64 = args.next().map(|a| a.parse()).transpose()?.unwrap_or(50.0);
    let y: f64 = args.next().map(|a| a.parse()).transpose()?.unwrap_or(0.0);
    let z: f64 = args.next().map(|a| a.parse()).transpose()?.unwrap_or(50.0);

    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;
    println!(
        "store: {} entities, {} relations, revision {}",
        snapshot.entities.len(),
        snapshot.relations.len(),
        base_revision.0
    );

    let placed = snapshot
        .entities
        .iter()
        .find(|e| e.key == EntityKey(placed_node))
        .ok_or_else(|| format!("placed_node {placed_node:#x} not in store"))?;
    let placed_label = placed
        .content
        .as_ref()
        .map(|c| store.read_content(c))
        .transpose()?
        .and_then(|c| {
            c.get("canonical_id")
                .or_else(|| c.get("id"))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "(no canonical_id)".into());
    println!("placing a Built position on {placed_node:#x} = `{placed_label}` at [{x}, {y}, {z}]");

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

    let position_content = serde_json::json!({
        "kind": "built_position", "space": "salle",
        "placed_node": format!("{placed_node:#x}"), "placed_label": placed_label,
        "x": x, "y": y, "z": z, "provenance": "built"
    });
    let construction_content = serde_json::json!({
        "kind": "placement_construction", "authored_by": "a.inchauspe@digitalkin.ai",
        "base_revision": base_revision.0,
        "note": "placement on a RENDERED node — make the layout-built pivot visible in the city"
    });
    let justification_content = serde_json::json!({
        "kind": "placement_justification",
        "statement": "Placed by hand on a rendered node so the built position wins in the materialized city, not just in the layout unit test."
    });

    let mut commands = Vec::new();
    if !plan.additions.is_empty() {
        commands.push(UniverseCommand::InternSymbols { symbols: plan.additions.clone() });
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
            source: EntityKey(placed_node),
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
        idempotency_key: format!("mutation:built-placement-rendered:{placed_node:#x}:v0"),
        causal_ancestry: vec!["changeset:built-placement-rendered-v0".to_string()],
        commands,
    };
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("committed {command_count} commands as one atomic set: {receipt:?}");

    // Independent readback.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    let position = after
        .entities
        .iter()
        .find(|e| e.key == EntityKey(POSITION))
        .ok_or("built_position not found on readback")?;
    let content = fresh.read_content(position.content.as_ref().ok_or("no content")?)?;
    println!("readback: revision {} -> {}", base_revision.0, after.revision.0);
    println!("built_position {POSITION:#x}: {content}");
    println!(
        "\nRESULT: `{placed_label}` ({placed_node:#x}) now carries a Built position [{x},{y},{z}]. \
         Re-run the materializer to see it win in the city."
    );
    Ok(())
}
