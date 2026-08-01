//! Bind the city's toolkits onto the Toolkit Shelf, and mint the one capability
//! that opens it — the BIND verb of `space:l2:mind-universe:toolkit-shelf-v0`.
//!
//! The shelf exists so an arriving inhabitant can be handed every toolkit in ONE
//! grant, and hold them as a BEARING rather than a copy: the grant edge names the
//! shelf's access capability, the shelf's members name the toolkit definitions,
//! and every read resolves at the reader's current revision. A revised blueprint
//! is therefore held revised, with nothing to synchronise.
//!
//! This bin carries NO policy of its own. What stands on the shelf and what the
//! access capability is called are read from the SHELF NODE in the graph
//! (`content.shelved`, `content.access_capability`), so adding a toolkit to the
//! city is a revision of that node plus a re-run — never an edit to this file.
//!
//!   shelf (resolved by canonical identity, never a baked key)
//!     -> mint the access capability if absent, content VERBATIM so `capability`
//!        sits at the TOP level (the field `universe_query::read_actor_capability_set`
//!        resolves from a USED target; a nested one would make that read FAIL)
//!     -> capability --APPLIES_IN--> shelf
//!     -> for each shelved canonical id: toolkit --PART_OF--> shelf
//!     -> ONE atomic transaction at a tick boundary
//!     -> independent readback from a FRESH reopen
//!
//! Honesty rules it keeps:
//!   * a shelved id that resolves to nothing is reported UNRESOLVED and produces
//!     NO edge — a declared-but-absent toolkit is a visible gap, never a silent
//!     omission and never a dangling edge;
//!   * `shelved` (authored intention) and `bound` (committed edges) are reported
//!     as SEPARATE numbers and never collapsed;
//!   * idempotent: a second run over an unchanged membership commits nothing and
//!     writes no event;
//!   * 0 new symbols — `thing`, `PART_OF` and `APPLIES_IN` are all canonical; a
//!     missing one is a hard error, never minted.
//!
//! Usage: `shelve_toolkits [--shelf <canonical-id>] [--store <dir>]`
//!   shelf default: space:l2:mind-universe:toolkit-shelf-v0
//!   store default: artifacts/ontology-registry/current/store

use std::{collections::BTreeMap, env, error::Error, path::PathBuf};

use serde_json::Value;
use universe_core::{EntityKey, RelationKey, Tick};
use universe_store::{EntityRecord, RelationRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

const DEFAULT_SHELF_ID: &str = "space:l2:mind-universe:toolkit-shelf-v0";

/// Entity-key block for the minted access capability. Allocated AFTER the
/// highest key already used in the block: `PutEntity` upserts, so a fixed base
/// would silently overwrite whatever a previous run put here.
const ENTITY_BASE: u128 = 0x000E_5A00_0000;
const ENTITY_BLOCK_END: u128 = 0x000E_5B00_0000;
/// Relation-key block for the membership + scope edges, allocated the same way.
const REL_BASE: u128 = 0x000E_5B00_0000;
const REL_BLOCK_END: u128 = 0x000E_6000_0000;

/// The node-type symbol a capability entity is written under (the same one
/// `underground-maintenance-grant.json` uses for its capability nodes).
const CAPABILITY_SYMBOL: &str = "thing";
/// Membership: a toolkit stands on the shelf.
const MEMBERSHIP_PREDICATE: &str = "PART_OF";
/// Scope: the access capability applies in the shelf space.
const SCOPE_PREDICATE: &str = "APPLIES_IN";

fn main() {
    if let Err(error) = run() {
        eprintln!("SHELVE FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut shelf_id = DEFAULT_SHELF_ID.to_owned();
    let mut store_dir = PathBuf::from("artifacts/ontology-registry/current/store");
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shelf" => shelf_id = args.next().ok_or("--shelf needs a value")?,
            "--store" => store_dir = PathBuf::from(args.next().ok_or("--store needs a value")?),
            other => return Err(format!("unexpected argument {other:?}").into()),
        }
    }
    println!("shelf    : {shelf_id}");
    println!("store dir: {}", store_dir.display());

    // 1. Open the LIVE store and replay to the authoritative snapshot.
    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;
    println!(
        "\nbase revision: {} | entities: {} | relations: {}",
        base_revision.0,
        snapshot.entities.len(),
        snapshot.relations.len()
    );

    // 2. Resolve every canonical identity ONCE, from committed content. Identity
    //    is how anything is found here; no key is ever baked into this file.
    let mut id_to_key: BTreeMap<String, EntityKey> = BTreeMap::new();
    for entity in &snapshot.entities {
        if let Some(ptr) = entity.content.as_ref() {
            let content = store.read_content(ptr)?;
            if let Some(cid) = content.get("canonical_id").and_then(Value::as_str) {
                id_to_key.insert(cid.to_owned(), entity.key);
            }
        }
    }
    let shelf_key = *id_to_key
        .get(&shelf_id)
        .ok_or_else(|| format!("shelf {shelf_id} is not in this store (inject it first)"))?;
    println!("shelf resolved: {shelf_key}");

    // 3. Read the shelf's OWN authored membership and access capability. The
    //    injector nests a construct's authored content one level down; a shelf
    //    authored flat is accepted too, and which shape was read is reported.
    let shelf_entity = snapshot
        .entities
        .iter()
        .find(|entity| entity.key == shelf_key)
        .ok_or("shelf key resolved but no entity carries it")?;
    let shelf_content = store.read_content(
        shelf_entity
            .content
            .as_ref()
            .ok_or("the shelf node carries no content")?,
    )?;
    let (authored, shape) = match shelf_content.get("content") {
        Some(nested) if nested.get("shelved").is_some() => (nested, "content.content"),
        _ => (&shelf_content, "content"),
    };
    println!("shelf content read from `{shape}`");

    let shelved: Vec<String> = authored
        .get("shelved")
        .and_then(Value::as_array)
        .ok_or("the shelf node carries no `shelved` array — nothing is declared to stand on it")?
        .iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();
    let access = authored
        .get("access_capability")
        .ok_or("the shelf node carries no `access_capability` block")?
        .clone();
    let access_id = access
        .get("canonical_id")
        .and_then(Value::as_str)
        .ok_or("the access_capability block carries no canonical_id")?
        .to_owned();
    let access_capability = access
        .get("capability")
        .and_then(Value::as_str)
        .ok_or("the access_capability block carries no `capability` string — a USED target \
                without one makes the bounded capability read FAIL, not skip")?
        .to_owned();
    println!(
        "declared: {} shelved id(s) | access capability: {access_id} ({access_capability})",
        shelved.len()
    );

    // 4. Symbol conformance: everything this bin writes is already canonical.
    let sym = |name: &str| -> Result<u32, Box<dyn Error>> {
        snapshot
            .symbol_id(name)
            .ok_or_else(|| format!("symbol '{name}' is not interned in this store").into())
    };
    let capability_symbol = sym(CAPABILITY_SYMBOL)?;
    let membership_symbol = sym(MEMBERSHIP_PREDICATE)?;
    let scope_symbol = sym(SCOPE_PREDICATE)?;
    println!("symbol conformance: 0 new symbols (thing / PART_OF / APPLIES_IN all canonical)");

    // Allocate after the highest key already used in each block: both verbs
    // upsert, so a fixed base would overwrite a previous run's work.
    let next_free = |used: Box<dyn Iterator<Item = u128> + '_>, base: u128, end: u128| -> u128 {
        used.filter(|key| (base..end).contains(key))
            .max()
            .map(|key| key + 1)
            .unwrap_or(base)
    };
    let mut next_entity = next_free(
        Box::new(snapshot.entities.iter().map(|entity| entity.key.0)),
        ENTITY_BASE,
        ENTITY_BLOCK_END,
    );
    let mut next_relation = next_free(
        Box::new(snapshot.relations.iter().map(|relation| relation.key.0)),
        REL_BASE,
        REL_BLOCK_END,
    );

    let mut commands = Vec::new();

    // 5. The access capability: minted only if absent, content VERBATIM from the
    //    shelf's own block so `capability` lands at the TOP level.
    let access_key = match id_to_key.get(&access_id) {
        Some(key) => {
            println!("\n  SKIP  access capability {access_id} (already present at {key})");
            *key
        }
        None => {
            let key = EntityKey(next_entity);
            next_entity += 1;
            println!("\n  MINT  access capability {access_id} -> {key}");
            commands.push(UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key,
                    generation: 0,
                    symbol: capability_symbol,
                    content: Some(store.append_content(&access)?),
                },
            });
            key
        }
    };

    // 6. Scope edge: the capability applies in the shelf space. This is the hop a
    //    reader takes from a held grant to the collection it opens.
    let scope_present = snapshot.relations.iter().any(|relation| {
        relation.source == access_key
            && relation.target == shelf_key
            && relation.predicate == scope_symbol
    });
    if scope_present {
        println!("  SKIP  {access_id} --APPLIES_IN--> shelf (already wired)");
    } else {
        println!(
            "  WIRE  {access_id} --APPLIES_IN--> {shelf_id}   ({:#x})",
            next_relation
        );
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: RelationKey(next_relation),
                generation: 0,
                source: access_key,
                target: shelf_key,
                predicate: scope_symbol,
                content: None,
            },
        });
        next_relation += 1;
    }

    // 7. Membership: one PART_OF edge per shelved toolkit that RESOLVES. An id
    //    that names nothing here produces no edge and is reported.
    println!();
    let mut planned: Vec<(String, EntityKey)> = Vec::new();
    let mut already: Vec<String> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    for id in &shelved {
        let Some(&toolkit_key) = id_to_key.get(id) else {
            println!("  UNRESOLVED  {id}  (declared, but no node in THIS store carries that identity)");
            unresolved.push(id.clone());
            continue;
        };
        let bound = snapshot.relations.iter().any(|relation| {
            relation.source == toolkit_key
                && relation.target == shelf_key
                && relation.predicate == membership_symbol
        });
        if bound {
            println!("  SKIP        {id}  (already on the shelf)");
            already.push(id.clone());
            continue;
        }
        println!("  SHELVE      {id}  --PART_OF--> shelf   ({next_relation:#x})");
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: RelationKey(next_relation),
                generation: 0,
                source: toolkit_key,
                target: shelf_key,
                predicate: membership_symbol,
                content: None,
            },
        });
        next_relation += 1;
        planned.push((id.clone(), toolkit_key));
    }

    // 8. Commit as ONE atomic transaction, or say plainly that nothing changed.
    println!(
        "\nnext free keys in this bin's blocks: entity {next_entity:#x} | relation {next_relation:#x}"
    );
    if commands.is_empty() {
        println!("\nNothing to bind — the shelf already carries every resolvable member. No event written.");
    } else {
        let count = commands.len();
        let write_set = UniverseWriteSet {
            base_revision,
            idempotency_key: format!("shelve-toolkits:{shelf_id}:{}", snapshot.tick.0),
            commands,
        };
        let boundary_tick = Tick(snapshot.tick.0 + 1);
        let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
        let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
        println!("\ncommitted {count} command(s) as one atomic set");
        println!("commit receipt: {receipt:?}");
    }

    // 9. INDEPENDENT readback: a fresh reopen, and the bound set re-derived from
    //    COMMITTED edges — never from the shelf's authored `shelved` list.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    println!("\n-- independent readback (fresh reopen) --");
    println!(
        "revision: {} -> {} | entities: {} | relations: {}",
        base_revision.0,
        after.revision.0,
        after.entities.len(),
        after.relations.len()
    );

    let membership_after = after
        .symbol_id(MEMBERSHIP_PREDICATE)
        .ok_or("PART_OF vanished from the symbol table")?;
    let scope_after = after
        .symbol_id(SCOPE_PREDICATE)
        .ok_or("APPLIES_IN vanished from the symbol table")?;

    // The access capability must read back carrying a TOP-LEVEL `capability`
    // string; that is the exact shape the bounded capability read resolves.
    let access_entity = after
        .entities
        .iter()
        .find(|entity| entity.key == access_key)
        .ok_or("the access capability is missing on readback")?;
    let access_content = fresh.read_content(
        access_entity
            .content
            .as_ref()
            .ok_or("the access capability carries no content on readback")?,
    )?;
    let read_capability = access_content
        .get("capability")
        .and_then(Value::as_str)
        .ok_or("the access capability has no TOP-LEVEL `capability` string on readback")?;
    println!("access capability: {access_id} -> capability={read_capability:?} (top-level, resolvable)");
    if read_capability != access_capability {
        return Err(format!(
            "the committed capability {read_capability:?} is not the authored one {access_capability:?}"
        )
        .into());
    }
    let scope_ok = after.relations.iter().any(|relation| {
        relation.source == access_key
            && relation.target == shelf_key
            && relation.predicate == scope_after
    });
    if !scope_ok {
        return Err("the APPLIES_IN edge from the capability to the shelf is missing on readback".into());
    }
    println!("scope edge : capability --APPLIES_IN--> shelf  PRESENT");

    for (id, toolkit_key) in &planned {
        let present = after.relations.iter().any(|relation| {
            relation.source == *toolkit_key
                && relation.target == shelf_key
                && relation.predicate == membership_after
        });
        if !present {
            return Err(format!("{id} was shelved but its PART_OF edge is missing on readback").into());
        }
    }

    // The bound count is TRAVERSED, not trusted: incoming PART_OF edges on the
    // shelf, resolved back to the identities they came from.
    let mut bound_ids: Vec<String> = Vec::new();
    for relation in after.relations.iter() {
        if relation.target != shelf_key || relation.predicate != membership_after {
            continue;
        }
        let identity = after
            .entities
            .iter()
            .find(|entity| entity.key == relation.source)
            .and_then(|entity| entity.content.as_ref())
            .and_then(|ptr| fresh.read_content(ptr).ok())
            .and_then(|content| {
                content
                    .get("canonical_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| format!("<unnamed {}>", relation.source));
        bound_ids.push(identity);
    }
    bound_ids.sort();
    println!("\n-- what stands on the shelf, traversed from committed edges --");
    for id in &bound_ids {
        println!("  ON SHELF  {id}");
    }

    println!("\nMETRIC (dimensions, never collapsed):");
    println!("  shelved_declared    : {}", shelved.len());
    println!("  shelved_bound       : {}", bound_ids.len());
    println!("  shelved_unresolved  : {}  {:?}", unresolved.len(), unresolved);
    println!("  newly_bound_this_run: {}", planned.len());
    println!("  already_bound       : {}", already.len());
    println!("  access_capability   : present, top-level `capability` resolvable");
    let health = if unresolved.is_empty() { "healthy" } else { "degraded" };
    println!("  health              : {health}  (degraded = the shelf works and is visibly incomplete)");

    println!(
        "\nRESULT: {} toolkit(s) stand on {shelf_id}; {} declared id(s) resolve to nothing in this store.",
        bound_ids.len(),
        unresolved.len()
    );
    println!("        An inhabitant granted {access_capability} reaches every one of them at the CURRENT revision.");
    println!("        Reach is not authority: a mutate still fails closed at the sealed port.");
    Ok(())
}
