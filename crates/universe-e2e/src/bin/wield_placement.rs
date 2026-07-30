//! T-W3: place a node with a RUNTIME gesture. An IR program (a `Propose`) runs on
//! the VM; its proposal is compiled by the GENERIC `translate_mutation_receipt`
//! into a write set and committed at a tick boundary — no hand-built write set.
//! This closes "wield an action that mutates the graph": execute_program → Propose
//! → translate_mutation_receipt → commit → independent readback.
//!
//! Usage: `wield_placement [store-dir]`
//!   store-dir defaults to `artifacts/ontology-registry/current/store`.

use std::collections::{BTreeMap, BTreeSet};
use std::{env, error::Error, path::PathBuf};

use universe_core::{EntityKey, Tick};
use universe_e2e::mutation_translate::{translate_mutation_receipt, MutationPlan};
use universe_ir::{CodeDefinition, QuerySpec, Value};
use universe_store::UniverseStore;
use universe_transactions::UniverseTransaction;
use universe_vm::{execute_program, ExecutionLimits, VmHost};

const POSITION: u128 = 0x9020;

/// A VM host for a program that only builds and proposes a record — it performs
/// no graph read, so every read hook is an honest error that is never reached.
struct NullHost;

impl VmHost for NullHost {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn capabilities(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn open_query(&mut self, _s: &QuerySpec, _o: &Value, _sel: &Value) -> Result<Value, String> {
        Err("wield-placement performs no query".into())
    }
    fn await_query(&mut self, _handle: &Value) -> Result<Value, String> {
        Err("wield-placement performs no query".into())
    }
    fn follow_one(&mut self, _source: &Value, _predicate: &Value) -> Result<Value, String> {
        Err("wield-placement follows no relation".into())
    }
    fn entity_symbol(&mut self, _entity: &Value) -> Result<Value, String> {
        Err("wield-placement reads no symbol".into())
    }
    fn hydrate(&mut self, _selected: &[Value], _max_bytes: u32) -> Result<Vec<Value>, String> {
        Err("wield-placement hydrates nothing".into())
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("WIELD FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let store_dir = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;
    println!(
        "store: {} entities, revision {}",
        snapshot.entities.len(),
        base_revision.0
    );

    // The `built_position` semanticType must already be interned (a prior placement
    // did it). The runtime gesture places an instance; it does not re-declare a type.
    let symbol = snapshot
        .symbols
        .iter()
        .position(|s| s == "built_position")
        .ok_or("`built_position` symbol not interned — run place_built_position first")?
        as u32;

    // 1. Load + execute the IR placement program on the VM.
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let code: CodeDefinition =
        serde_json::from_slice(&std::fs::read(repo.join("fixtures/graph-ir/wield-placement.json"))?)?;
    let mut host = NullHost;
    let inputs: BTreeMap<String, Value> = BTreeMap::new();
    let receipt = execute_program(
        &code,
        &mut host,
        &inputs,
        base_revision,
        snapshot.tick,
        ExecutionLimits {
            fuel: 64,
            max_proposals: 1,
        },
    )
    .map_err(|error| format!("VM error: {error:?}"))?;
    println!(
        "IR executed: {} proposal(s), fuel used {}",
        receipt.proposals.len(),
        receipt.fuel_used
    );

    // 2. Compile the runtime proposal into a write set via the GENERIC translator.
    let plan = MutationPlan::PutEntity {
        key: EntityKey(POSITION),
        generation: 0,
        symbol,
        content_field: Some("content".into()),
    };
    let write_set = translate_mutation_receipt(
        &plan,
        &receipt,
        &store,
        base_revision,
        "mutation:wield-placement:v0".into(),
        vec![
            "changeset:wield-placement-v0".into(),
            receipt.code_hash.clone(),
        ],
    )?
    .ok_or("translator produced no write set")?;

    // 3. Commit the write set at a tick boundary.
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let commit = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("committed via runtime gesture: {commit:?}");

    // 4. Independent readback.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    let entity = after
        .entities
        .iter()
        .find(|e| e.key == EntityKey(POSITION))
        .ok_or("placed entity not found on readback")?;
    let content = fresh.read_content(entity.content.as_ref().ok_or("no content")?)?;
    println!("readback: revision {} -> {}", base_revision.0, after.revision.0);
    println!("entity {POSITION:#x} content: {content}");

    println!(
        "\nRESULT: entity {POSITION:#x} was placed by a RUNTIME IR gesture \
         (execute_program -> Propose -> translate_mutation_receipt -> commit), read back \
         from a fresh store. The write path is wieldable — no hand-built write set."
    );
    Ok(())
}
