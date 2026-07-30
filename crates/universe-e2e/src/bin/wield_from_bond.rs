//! The last mile: the MutationBond in the graph IS the action, proven LIVE.
//!
//! A store is stood up with the canonical seed + the `mutation-bond-authority`
//! fixture activated. This bin then: reads the bond FROM the store, projects its
//! shape (`project_mutation_bond`), runs an IR gesture that supplies the content,
//! validates the content against the bond's contract, and commits an ATOMIC SET
//! (intern the projected content type + PutEntity) — then reads it back from a
//! fresh store. Nothing about the mutation is hardcoded: command_kind, the written
//! type, the content field and the required fields all come from the bond.
//!
//! Usage: `wield_from_bond` (uses a throwaway temp store; never the live current).

use std::collections::{BTreeMap, BTreeSet};
use std::{error::Error, path::Path};

use universe_core::{EntityKey, Tick};
use universe_e2e::mutation_translate::{project_mutation_bond, translate_mutation_receipt};
use universe_ir::{CodeDefinition, QuerySpec, Value};
use universe_store::UniverseStore;
use universe_testkit::install_authority_fixture;
use universe_transactions::{UniverseCommand, UniverseTransaction};
use universe_vm::{execute_program, ExecutionLimits, VmHost};

const PLACED: u128 = 0x9030;

struct NullHost;
impl VmHost for NullHost {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn capabilities(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn open_query(&mut self, _s: &QuerySpec, _o: &Value, _sel: &Value) -> Result<Value, String> {
        Err("no query".into())
    }
    fn await_query(&mut self, _h: &Value) -> Result<Value, String> {
        Err("no query".into())
    }
    fn follow_one(&mut self, _s: &Value, _p: &Value) -> Result<Value, String> {
        Err("no follow".into())
    }
    fn entity_symbol(&mut self, _e: &Value) -> Result<Value, String> {
        Err("no symbol".into())
    }
    fn hydrate(&mut self, _s: &[Value], _m: u32) -> Result<Vec<Value>, String> {
        Err("no hydrate".into())
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("WIELD-FROM-BOND FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store_dir = std::env::temp_dir().join("mind-wield-from-bond-store");
    let _ = std::fs::remove_dir_all(&store_dir); // install_authority_fixture needs an empty store

    // 1. Stand up a store: canonical seed + the mutation-bond authority.
    let install = install_authority_fixture(
        repo.join("fixtures/ontology/mutation-bond-authority.json"),
        &store_dir,
    )?;
    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = install.snapshot;
    let base_revision = snapshot.revision;
    println!(
        "store: {} entities, revision {} (seed + mutation-bond authority)",
        snapshot.entities.len(),
        base_revision.0
    );

    // 2. Read the bond FROM the store and follow USES_FIELD_SCHEMA to its schema.
    let read = |key: EntityKey| -> Result<serde_json::Value, Box<dyn Error>> {
        let entity = snapshot
            .entities
            .iter()
            .find(|e| e.key == key)
            .ok_or_else(|| format!("entity {key} absent"))?;
        Ok(store.read_content(entity.content.as_ref().ok_or("no content")?)?)
    };
    let bond = snapshot
        .entities
        .iter()
        .find(|e| {
            e.content
                .as_ref()
                .and_then(|c| store.read_content(c).ok())
                .map(|c| c["kind"] == "mutation_bond_instance")
                .unwrap_or(false)
        })
        .ok_or("no mutation_bond_instance in store")?
        .key;
    let uses_field_schema = snapshot
        .symbols
        .iter()
        .position(|s| s == "USES_FIELD_SCHEMA")
        .ok_or("USES_FIELD_SCHEMA symbol absent")? as u32;
    let field_schema = snapshot
        .relations
        .iter()
        .find(|r| r.source == bond && r.predicate == uses_field_schema)
        .ok_or("bond has no USES_FIELD_SCHEMA relation")?
        .target;
    let projection = project_mutation_bond(&read(bond)?, &read(field_schema)?)?;
    println!(
        "projected from bond {bond}: verb={:?} content_kind={} content_field={} required={:?}",
        projection.command_kind,
        projection.content_kind,
        projection.content_field,
        projection.required_fields
    );

    // 3. An IR gesture supplies the content.
    let code: CodeDefinition =
        serde_json::from_slice(&std::fs::read(repo.join("fixtures/graph-ir/wield-from-bond.json"))?)?;
    let mut host = NullHost;
    let receipt = execute_program(
        &code,
        &mut host,
        &BTreeMap::new(),
        base_revision,
        snapshot.tick,
        ExecutionLimits {
            fuel: 64,
            max_proposals: 1,
        },
    )
    .map_err(|error| format!("VM: {error:?}"))?;

    // 4. Validate the proposed content against the bond's contract.
    let proposal =
        universe_e2e::mutation_translate::ir_value_to_json(&receipt.proposals[0].command);
    projection.validate_content(
        proposal
            .get(&projection.content_field)
            .ok_or("proposal lacks the content field")?,
    )?;

    // 5. Resolve the projected content type (interning it if new) and build the plan.
    let symbol_plan = snapshot.plan_symbol_interning(&[projection.content_kind.clone()])?;
    let symbol = symbol_plan.assignments[&projection.content_kind];
    let plan = projection.into_put_entity_plan(EntityKey(PLACED), symbol)?;

    // 6. Translate the gesture into a write set, then front it with the intern so
    //    the whole placement commits as ONE atomic SET (the "lié au set" model).
    let mut write_set = translate_mutation_receipt(
        &plan,
        &receipt,
        &store,
        base_revision,
        "mutation:wield-from-bond:v0".into(),
        vec!["changeset:wield-from-bond-v0".into(), receipt.code_hash.clone()],
    )?
    .ok_or("translator produced no write set")?;
    if !symbol_plan.additions.is_empty() {
        write_set.commands.insert(
            0,
            UniverseCommand::InternSymbols {
                symbols: symbol_plan.additions,
            },
        );
    }
    let command_count = write_set.commands.len();

    // 7. Commit the set at a tick boundary.
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let commit = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("committed {command_count} commands as one atomic set: {commit:?}");

    // 8. Independent readback.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    let placed = after
        .entities
        .iter()
        .find(|e| e.key == EntityKey(PLACED))
        .ok_or("placed entity not found on readback")?;
    let content = fresh.read_content(placed.content.as_ref().ok_or("no content")?)?;
    println!("readback: revision {} -> {}", base_revision.0, after.revision.0);
    println!("entity {PLACED:#x} content: {content}");

    println!(
        "\nRESULT: the MutationBond in the graph furnished the plan (verb, type, content \
         field, contract); an IR gesture supplied the content; committed as an atomic set \
         (intern + put) and read back. The loop is closed LIVE — the bond is the action."
    );
    Ok(())
}
