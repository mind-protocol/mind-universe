//! Wield the Construction toolkit: furnish the ordered one-verb set for a `room`
//! AND a `route` from the SAME modular mechanism, aggregate each into ONE atomic
//! `UniverseWriteSet` through the GENERIC translator, commit to a fresh SCRATCH
//! store seeded with the canonical ontology, and prove each construction by
//! INDEPENDENT readback of its placement RELATIONS from a fresh reopen.
//!
//! What this proves (Construction toolkit v0, where-is-a-projection model):
//!   * one construction = ONE atomic set of one-verb MutationBonds
//!     (put_entity structure -> put_relation PART_OF region -> [route only]
//!      put_relation COMMUNICATES from-anchor -> put_relation COMMUNICATES
//!      to-anchor -> put_entity Moment -> put_relation PRODUCES -> put_entity
//!      justification -> put_relation GROUNDS)
//!   * placement is RELATIONAL — NO coordinate is ever written: no built_position
//!     field, no x/y/z, no path polyline, no boundary polygon. WHERE a structure
//!     is lives entirely in its relations (PART_OF its region; for a route,
//!     CONNECTS its two anchor NODES), and the readback asserts those relations
//!     and asserts the structure content carries NO position field.
//!   * only CANONICAL predicates are emitted (PART_OF, COMMUNICATES, PRODUCES,
//!     GROUNDS); the ad-hoc HAS_POSITION / CONNECTS / CONSTRUCTED_BY / JUSTIFIED_BY
//!     are gone; 0 new symbols
//!   * `structure_kind`, its region predicate AND its connection predicate are
//!     READ FROM THE GRAPH (the construction-toolkit fixture's `kind_profiles`),
//!     not dispatched in code
//!   * all-or-nothing: a deliberately-bad step (PART_OF onto a non-existent
//!     region) makes `prepare` reject the whole set — nothing is committed
//!
//! Usage: `wield_construction` (uses a throwaway scratch store; never the live current).

use std::{error::Error, path::Path};

use serde_json::{json, Value};
use universe_core::{EntityKey, RelationKey, Tick};
use universe_e2e::mutation_translate::{translate_mutation_proposal, MutationPlan};
use universe_store::{load_seed, UniverseSnapshot, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

// Region + anchor nodes: pre-existing canonical seed nodes a structure is
// PART_OF / CONNECTS. (The protocol manifest and a canonical `terme` stand in
// for a building / a district — the proof is about the construction mechanism
// and its relational placement, not region semantics.)
const REGION_BUILDING: u128 = 0x1000; // protocol manifest (seed) — building / route from-anchor
const REGION_DISTRICT: u128 = 0x1100; // a canonical `terme` (seed) — district / route to-anchor
const NONEXISTENT_REGION: u128 = 0x0000_0000_0000_0000_0000_0000_0000_dead;

/// A route endpoint: a relation key plus the OTHER NODE it CONNECTS. Placement is
/// carried by these relations — there is no coordinate anywhere.
struct Anchor {
    rel_key: u128,
    node: u128,
}

/// Disjoint key block for one construction (structure, Moment, justification and
/// their relations), far above the seed's 0x1000-0x2xxx range. `connections` is
/// empty for a room and holds the two anchor edges for a route.
struct Keys {
    structure: u128,
    moment: u128,
    justification: u128,
    rel_part_of: u128,
    rel_produces: u128,
    rel_grounds: u128,
    connections: Vec<Anchor>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("WIELD-CONSTRUCTION FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let seed_path = repo.join("fixtures/ontology/canonical-ontology.json");
    let toolkit_path = repo.join("fixtures/ontology/construction-toolkit-v0.json");

    // Fresh scratch store (never the live artifacts store).
    let store_dir = std::env::temp_dir().join("mind-wield-construction-store");
    if store_dir.exists() {
        std::fs::remove_dir_all(&store_dir)?;
    }
    std::fs::create_dir_all(&store_dir)?;

    // Seed the scratch store with the canonical ontology (hash-checked loader).
    let store = UniverseStore::open(&store_dir)?;
    let seed = load_seed(&seed_path)?;
    let mut snapshot = store.install_seed(&seed)?;
    let seed_symbol_count = snapshot.symbols.len();
    println!(
        "seeded scratch store: {} entities, {} relations, {} symbols, revision {}",
        snapshot.entities.len(),
        snapshot.relations.len(),
        seed_symbol_count,
        snapshot.revision.0
    );

    // The construction toolkit fixture is the GRAPH DATA that furnishes the shape:
    // structure_kind -> region_predicate / connection_predicate are read from
    // here, never dispatched in code.
    let toolkit: Value = serde_json::from_slice(&std::fs::read(&toolkit_path)?)?;
    let profile_str = |kind: &str, field: &str| -> Option<String> {
        toolkit
            .pointer(&format!(
                "/content/modularity/kind_profiles/{kind}/{field}"
            ))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let region_predicate = |kind: &str| -> Result<String, Box<dyn Error>> {
        profile_str(kind, "region_predicate")
            .ok_or_else(|| format!("kind `{kind}` has no region_predicate in the toolkit fixture").into())
    };
    // Optional: `null` connection_predicate means the kind has no CONNECTS edges.
    let connection_predicate = |kind: &str| -> Option<String> { profile_str(kind, "connection_predicate") };

    // ---- Construction 1: a ROOM (infrastructure), PART_OF a building. --------
    // Geometry is SCALAR SIZE only (width/height/depth) — a dimension, never a
    // coordinate. Placement is the PART_OF region edge, nothing else.
    let room_keys = Keys {
        structure: 0xA000,
        moment: 0xA001,
        justification: 0xA002,
        rel_part_of: 0xA100,
        rel_produces: 0xA101,
        rel_grounds: 0xA102,
        connections: Vec::new(),
    };
    let room_structure = json!({
        "kind": "built_structure",
        "structure_kind": "room",
        "provenance": "built",
        // scalar SIZE fields (data, per the fixture kind_profile) — not a position
        "width": 6.0, "height": 3.0, "depth": 5.0,
        "membrane": "closed", "door_ports": 1
    });
    construct(
        &store,
        &mut snapshot,
        "room",
        &room_keys,
        REGION_BUILDING,
        &region_predicate("room")?,
        connection_predicate("room").as_deref(),
        &room_structure,
        "Placed by belonging: this room is PART_OF the building's public wing, which is where it is — no coordinate is authored.",
        &store_dir,
    )?;

    // ---- Construction 2: a ROUTE (civic), PART_OF a district, CONNECTS anchors.
    // The route's whole place is its district plus its two anchor NODES, bound by
    // CONNECTS (canonical COMMUNICATES). No path polyline, no coordinate.
    let route_connection_predicate =
        connection_predicate("route").ok_or("route kind_profile has no connection_predicate")?;
    let route_keys = Keys {
        structure: 0xB000,
        moment: 0xB001,
        justification: 0xB002,
        rel_part_of: 0xB100,
        rel_produces: 0xB101,
        rel_grounds: 0xB102,
        connections: vec![
            Anchor { rel_key: 0xB200, node: REGION_BUILDING }, // from-anchor
            Anchor { rel_key: 0xB201, node: REGION_DISTRICT }, // to-anchor
        ],
    };
    let route_structure = json!({
        "kind": "built_structure",
        "structure_kind": "route",
        "provenance": "built",
        "surface": "paved", "lanes": 2
        // NO coordinate: no path, no x/y/z. The endpoints are CONNECTS relations.
    });
    construct(
        &store,
        &mut snapshot,
        "route",
        &route_keys,
        REGION_DISTRICT,
        &region_predicate("route")?,
        Some(route_connection_predicate.as_str()),
        &route_structure,
        "Laid by connection: this route CONNECTS the building and the civic district and is PART_OF that district — its place is those relations, not a polyline.",
        &store_dir,
    )?;

    // ---- Construction 3: a DELIBERATELY-BAD set — must commit NOTHING. --------
    // Same furnished set, but the PART_OF relation targets a non-existent region.
    // `UniverseTransaction::prepare` applies the whole set to a candidate and
    // validates it; a dangling relation endpoint rejects the ENTIRE set, so no
    // valid prefix (not even the structure entity) is published.
    let bad_keys = Keys {
        structure: 0xC000,
        moment: 0xC001,
        justification: 0xC002,
        rel_part_of: 0xC100,
        rel_produces: 0xC101,
        rel_grounds: 0xC102,
        connections: Vec::new(),
    };
    let bad_structure = json!({
        "kind": "built_structure", "structure_kind": "room", "provenance": "built",
        "width": 1.0, "height": 1.0, "depth": 1.0, "membrane": "closed", "door_ports": 0
    });
    let base_revision = snapshot.revision;
    let commands = furnish_construction_set(
        &store,
        &snapshot,
        &bad_keys,
        NONEXISTENT_REGION, // <- dangling PART_OF endpoint
        "PART_OF",
        None,
        &bad_structure,
        "This construction must never commit — its region does not exist.",
        base_revision,
    )?;
    let bad_write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: "construction:bad-region:v0".into(),
        commands,
    };
    match UniverseTransaction::prepare(&snapshot, bad_write_set) {
        Ok(_) => return Err("bad construction was NOT rejected — atomicity violated".into()),
        Err(error) => println!(
            "\n[bad set] correctly rejected before commit (all-or-nothing): {error}"
        ),
    }

    // Independent proof that the bad set committed nothing: fresh reopen, the
    // structure key is absent and the revision is unchanged from the route commit.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    if after
        .entities
        .iter()
        .any(|e| e.key == EntityKey(bad_keys.structure))
    {
        return Err("bad structure leaked into the store — not all-or-nothing".into());
    }
    if after.revision != base_revision {
        return Err(format!(
            "revision advanced ({} -> {}) despite the bad set committing nothing",
            base_revision.0, after.revision.0
        )
        .into());
    }
    println!(
        "[bad set] independent readback: structure {:#x} absent, revision still {} — nothing committed.",
        bad_keys.structure, after.revision.0
    );

    // ---- Global invariant: 0 new symbols interned across every construction. --
    if after.symbols.len() != seed_symbol_count {
        return Err(format!(
            "symbol table grew {} -> {} — a non-canonical symbol was interned",
            seed_symbol_count,
            after.symbols.len()
        )
        .into());
    }
    println!(
        "\nZERO new symbols interned across both constructions (symbol table stayed at {seed_symbol_count})."
    );
    println!(
        "RESULT: the SAME Construction toolkit built a Built room AND a Built route as one atomic set each, \
         over a fresh canonical store, placement carried ENTIRELY by relations (PART_OF region; the route \
         CONNECTS two anchor nodes via COMMUNICATES), NO coordinate written, only canonical predicates \
         (PART_OF/COMMUNICATES/PRODUCES/GROUNDS), proven by independent readback; a bad set committed nothing."
    );
    Ok(())
}

/// Resolve a symbol id from the (seed) snapshot, erroring if it is not already
/// interned — this is how "0 new symbols" is enforced: every node-type symbol and
/// predicate the construction needs MUST pre-exist in the canonical seed.
fn resolve(snapshot: &UniverseSnapshot, symbol: &str) -> Result<u32, Box<dyn Error>> {
    snapshot
        .symbol_id(symbol)
        .ok_or_else(|| format!("symbol `{symbol}` is not in the canonical seed (would need interning)").into())
}

/// Push exactly one command produced by the GENERIC translator for a single
/// one-verb step. Each step is translated through `translate_mutation_proposal`
/// (the generic write-side translator) and its single command is collected; the
/// caller aggregates all of them into ONE `UniverseWriteSet`.
fn translate_step(
    store: &UniverseStore,
    base_revision: universe_core::Revision,
    plan: &MutationPlan,
    proposal: &Value,
    commands: &mut Vec<UniverseCommand>,
) -> Result<(), Box<dyn Error>> {
    let ws = translate_mutation_proposal(
        plan,
        proposal,
        store,
        base_revision,
        "construction:step:v0".into(),
    )?;
    // The generic translator emits exactly one kernel verb per step.
    if ws.commands.len() != 1 {
        return Err(format!("translator produced {} commands, expected 1", ws.commands.len()).into());
    }
    commands.extend(ws.commands);
    Ok(())
}

/// Furnish the ordered one-verb step set for one construction and translate each
/// through the generic translator into the aggregated command vector. Placement
/// is RELATIONS only: PART_OF the region, plus (for a route) one CONNECTS edge
/// per anchor node — never a coordinate.
#[allow(clippy::too_many_arguments)]
fn furnish_construction_set(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    keys: &Keys,
    region: u128,
    region_predicate: &str,
    connection_predicate: Option<&str>,
    structure_content: &Value,
    justification_statement: &str,
    base_revision: universe_core::Revision,
) -> Result<Vec<UniverseCommand>, Box<dyn Error>> {
    // Only canonical predicates are allowed to reach the store.
    let canonical = ["PART_OF", "COMMUNICATES", "PRODUCES", "GROUNDS"];
    let mut used = vec![region_predicate, "PRODUCES", "GROUNDS"];
    if let Some(conn) = connection_predicate {
        used.push(conn);
    }
    for predicate in used {
        if !canonical.contains(&predicate) {
            return Err(format!("non-canonical predicate `{predicate}` refused").into());
        }
    }
    // A structure must never carry a stored coordinate: WHERE is a projection.
    for coord in ["x", "y", "z", "built_position", "path", "boundary"] {
        if structure_content.get(coord).is_some() {
            return Err(format!("structure carries a forbidden position field `{coord}` — placement is relational").into());
        }
    }

    let sym_space = resolve(snapshot, "space")?; // BuiltStructure node-type
    let sym_moment = resolve(snapshot, "moment")?; // construction Moment node-type
    let sym_rationale = resolve(snapshot, "design_rationale")?; // justification node-type
    let pred_part_of = resolve(snapshot, region_predicate)?;
    let pred_produces = resolve(snapshot, "PRODUCES")?;
    let pred_grounds = resolve(snapshot, "GROUNDS")?;

    let moment_content = json!({
        "kind": "construction",
        "authored_by": "a.inchauspe@digitalkin.ai",
        "structure_kind": structure_content.get("structure_kind").cloned().unwrap_or(Value::Null),
        "base_revision": base_revision.0,
        "note": "construction gesture wielded via the Construction toolkit (generic translator, atomic set, relational placement)"
    });
    let justification_content = json!({
        "kind": "justification",
        "statement": justification_statement
    });

    let mut commands = Vec::new();
    // 1. put_entity structure (scalar geometry only; NO position field)
    translate_step(
        store,
        base_revision,
        &MutationPlan::PutEntity {
            key: EntityKey(keys.structure),
            generation: 0,
            symbol: sym_space,
            content_field: Some("content".into()),
        },
        &json!({ "content": structure_content }),
        &mut commands,
    )?;
    // 2. put_relation structure PART_OF region (primary placement statement)
    translate_step(
        store,
        base_revision,
        &MutationPlan::PutRelation {
            key: RelationKey(keys.rel_part_of),
            generation: 0,
            source: EntityKey(keys.structure),
            target: EntityKey(region),
            predicate: pred_part_of,
            content_field: None,
        },
        &json!({}),
        &mut commands,
    )?;
    // 2b. (route only) put_relation structure CONNECTS anchor, one per endpoint.
    //     The endpoints are OTHER NODES, referenced by relation, never coordinates.
    if let Some(conn) = connection_predicate {
        let pred_connect = resolve(snapshot, conn)?;
        for anchor in &keys.connections {
            translate_step(
                store,
                base_revision,
                &MutationPlan::PutRelation {
                    key: RelationKey(anchor.rel_key),
                    generation: 0,
                    source: EntityKey(keys.structure),
                    target: EntityKey(anchor.node),
                    predicate: pred_connect,
                    content_field: None,
                },
                &json!({}),
                &mut commands,
            )?;
        }
    } else if !keys.connections.is_empty() {
        return Err("connections furnished but the kind has no connection_predicate".into());
    }
    // 3. put_entity construction Moment
    translate_step(
        store,
        base_revision,
        &MutationPlan::PutEntity {
            key: EntityKey(keys.moment),
            generation: 0,
            symbol: sym_moment,
            content_field: Some("content".into()),
        },
        &json!({ "content": moment_content }),
        &mut commands,
    )?;
    // 4. put_relation Moment PRODUCES structure
    translate_step(
        store,
        base_revision,
        &MutationPlan::PutRelation {
            key: RelationKey(keys.rel_produces),
            generation: 0,
            source: EntityKey(keys.moment),
            target: EntityKey(keys.structure),
            predicate: pred_produces,
            content_field: None,
        },
        &json!({}),
        &mut commands,
    )?;
    // 5. put_entity justification
    translate_step(
        store,
        base_revision,
        &MutationPlan::PutEntity {
            key: EntityKey(keys.justification),
            generation: 0,
            symbol: sym_rationale,
            content_field: Some("content".into()),
        },
        &json!({ "content": justification_content }),
        &mut commands,
    )?;
    // 6. put_relation justification GROUNDS structure
    translate_step(
        store,
        base_revision,
        &MutationPlan::PutRelation {
            key: RelationKey(keys.rel_grounds),
            generation: 0,
            source: EntityKey(keys.justification),
            target: EntityKey(keys.structure),
            predicate: pred_grounds,
            content_field: None,
        },
        &json!({}),
        &mut commands,
    )?;
    Ok(commands)
}

/// Furnish -> aggregate -> commit -> INDEPENDENT readback for one construction.
#[allow(clippy::too_many_arguments)]
fn construct(
    store: &UniverseStore,
    snapshot: &mut UniverseSnapshot,
    kind: &str,
    keys: &Keys,
    region: u128,
    region_predicate: &str,
    connection_predicate: Option<&str>,
    structure_content: &Value,
    justification_statement: &str,
    store_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let base_revision = snapshot.revision;
    let commands = furnish_construction_set(
        store,
        snapshot,
        keys,
        region,
        region_predicate,
        connection_predicate,
        structure_content,
        justification_statement,
        base_revision,
    )?;
    let command_count = commands.len();

    // The four MutationBond write verbs are now the WHOLE of UniverseCommand, so
    // the closed-verb guard is carried by the type rather than by a runtime check
    // that can no longer fire: the fifth verb it excluded no longer exists
    // anywhere in the kernel.
    // No InternSymbols in a clean construction (0 new symbols).
    if commands
        .iter()
        .any(|c| matches!(c, UniverseCommand::InternSymbols { .. }))
    {
        return Err(format!("{kind}: construction emitted InternSymbols — expected 0 new symbols").into());
    }

    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: format!("construction:{kind}:v0"),
        commands,
    };

    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(snapshot, write_set)?;
    let receipt = transaction.commit(store, snapshot, boundary_tick)?;
    println!(
        "\n[{kind}] committed {command_count} commands as ONE atomic set: {receipt:?}"
    );

    // INDEPENDENT readback: fresh reopen from disk.
    let fresh = UniverseStore::open(store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;

    let structure = after
        .entities
        .iter()
        .find(|e| e.key == EntityKey(keys.structure))
        .ok_or_else(|| format!("{kind}: structure {:#x} not found on readback", keys.structure))?;
    let content = fresh.read_content(
        structure
            .content
            .as_ref()
            .ok_or_else(|| format!("{kind}: structure has no content"))?,
    )?;

    // provenance 'built'
    let provenance = content.get("provenance").and_then(Value::as_str).unwrap_or("(none)");
    if provenance != "built" {
        return Err(format!("{kind}: structure provenance is `{provenance}`, expected `built`").into());
    }
    // structure carries the correct structure_kind (read from graph, not dispatched)
    let stored_kind = content.get("structure_kind").and_then(Value::as_str).unwrap_or("(none)");
    if stored_kind != kind {
        return Err(format!("{kind}: structure_kind is `{stored_kind}`, expected `{kind}`").into());
    }
    // NO coordinate field on the structure — WHERE is a projection, not a datum.
    for coord in ["x", "y", "z", "built_position", "path", "boundary"] {
        if content.get(coord).is_some() {
            return Err(format!("{kind}: structure carries a forbidden position field `{coord}` — placement must be relational").into());
        }
    }
    // and the HAS_POSITION predicate symbol never leaked into the store.
    if after.symbols.iter().any(|s| s == "HAS_POSITION") {
        return Err(format!("{kind}: HAS_POSITION symbol leaked — position must be relations, not an edge").into());
    }

    // PART_OF region edge (primary placement)
    let part_of = after.relations.iter().any(|r| {
        r.key == RelationKey(keys.rel_part_of)
            && r.source == EntityKey(keys.structure)
            && r.target == EntityKey(region)
            && after.symbols.get(r.predicate as usize).map(String::as_str) == Some("PART_OF")
    });
    // For a route: each anchor is CONNECTS-ed via the canonical connection predicate.
    let mut connections_ok = true;
    let mut connections_report = String::new();
    if let Some(conn) = connection_predicate {
        for anchor in &keys.connections {
            let present = after.relations.iter().any(|r| {
                r.key == RelationKey(anchor.rel_key)
                    && r.source == EntityKey(keys.structure)
                    && r.target == EntityKey(anchor.node)
                    && after.symbols.get(r.predicate as usize).map(String::as_str) == Some(conn)
            });
            connections_ok &= present;
            connections_report.push_str(&format!(" {}->{:#x}={present}", conn, anchor.node));
        }
    }
    // Moment PRODUCES structure
    let produces = after.relations.iter().any(|r| {
        r.key == RelationKey(keys.rel_produces)
            && r.source == EntityKey(keys.moment)
            && r.target == EntityKey(keys.structure)
            && after.symbols.get(r.predicate as usize).map(String::as_str) == Some("PRODUCES")
    });
    // justification GROUNDS structure
    let grounds = after.relations.iter().any(|r| {
        r.key == RelationKey(keys.rel_grounds)
            && r.source == EntityKey(keys.justification)
            && r.target == EntityKey(keys.structure)
            && after.symbols.get(r.predicate as usize).map(String::as_str) == Some("GROUNDS")
    });

    // Construction Moment present (attributable, non-anonymous)
    let moment_present = after.entities.iter().any(|e| e.key == EntityKey(keys.moment));

    println!(
        "[{kind}] readback rev {} -> {} | provenance={provenance} kind={stored_kind} (no coordinate stored) | PART_OF={part_of}{connections_report} PRODUCES={produces} GROUNDS={grounds} moment={moment_present}",
        base_revision.0,
        after.revision.0,
    );

    // Forgery check: provenance 'built' with no construction Moment producing it
    // is unfalsifiable and must be rejected.
    if provenance == "built" && !(produces && moment_present) {
        return Err(format!("{kind}: FORGERY — provenance=built with no construction Moment PRODUCES edge").into());
    }
    if !(part_of && produces && grounds && moment_present && connections_ok) {
        return Err(format!("{kind}: readback is missing a required construction/placement edge").into());
    }
    Ok(())
}
