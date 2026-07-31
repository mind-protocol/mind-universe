//! Generic additive injector for a `mind-universe-authority-fixture`
//! (`entities[]` / `relations[]` envelope) into a LIVE, already-seeded
//! `UniverseStore`, with independent readback.
//!
//! This is the live-store counterpart of `universe_testkit::install_authority_fixture`
//! and the authority-fixture counterpart of `inject_construct`:
//!
//!   * `install_authority_fixture` stands a store up from the canonical seed
//!     (it calls `install_seed`, which refuses a non-empty store), so it cannot
//!     overlay a fixture onto the live ontology-registry store.
//!   * `inject_construct` overlays a LIVE store, but only for the `members[]`
//!     construct envelope (string ids, hashed key block) — not the
//!     `entities[]` / `relations[]` authority-fixture shape (explicit keys,
//!     verbatim entity content).
//!
//! This bin fills the gap: it appends the authored authority-fixture subgraph to
//! a store that is already populated, as ONE atomic transaction, interning ZERO
//! new symbols. It is bootstrap tooling, NOT the permanent semantic-intent write
//! path.
//!
//! Contract & honesty:
//!   * The authored fixture already carries an explicit DISJOINT key block (the
//!     grant uses 0x5a00.. entities / 0x5b00.. relations). That block IS the
//!     disjoint allocation; this bin verifies it is free in the live store and
//!     hard-errors on ANY pre-existing key (never overwrites). The authored keys
//!     are used verbatim so the numeric relation endpoints stay valid and the
//!     grant reads back at the keys the tests expect (e.g. actor 0x5a20).
//!   * `canonical_predicate_remap` is a documentation block in the fixture: its
//!     relations already carry the canonical predicate (the grant rides `USED`,
//!     not the authored `HOLDS_CAPABILITY`). This bin performs NO remap; it
//!     interns exactly the authored `entity.symbol` + `relation.predicate` +
//!     top-level `symbols[]`, and a clean injection interns 0 new symbols. Any
//!     non-canonical symbol is a hard error, never minted into the store.
//!   * Entity content is stored VERBATIM (as `install_authority_fixture` does),
//!     so a capability entity's top-level `capability` field and an actor's
//!     top-level `canonical_id` are exactly what a bounded reader materializes.
//!   * The whole subgraph is read back from a fresh reopen: every entity key is
//!     present, and every grant edge is confirmed with its actor -> capability
//!     resolved from the committed content.
//!
//! Usage: `inject_authority_fixture <fixture.json> [store-dir]`
//!   store-dir defaults to artifacts/ontology-registry/current/store

use std::{collections::BTreeSet, env, error::Error, path::PathBuf};

use serde::Deserialize;
use universe_core::{EntityKey, Tick, UniverseId};
use universe_store::{EntityRecord, RelationRecord, SeedEntity, SeedRelation, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// The `mind-universe-authority-fixture` envelope (arbitrary keys). Mirrors the
/// private testkit `AuthorityFixture`, reusing the public `SeedEntity` /
/// `SeedRelation` (whose keys deserialize from 32-hex-digit strings). Unknown
/// top-level fields (`canonical_predicate_remap`, `symbols` notes) are ignored.
#[derive(Deserialize)]
struct AuthorityFixture {
    contract: String,
    version: u16,
    universe: UniverseId,
    #[serde(default)]
    symbols: Vec<String>,
    entities: Vec<SeedEntity>,
    #[serde(default)]
    relations: Vec<SeedRelation>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("INJECTION FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let fixture_path = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: inject_authority_fixture <fixture.json> [store-dir]")?;
    let store_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    println!("fixture  : {}", fixture_path.display());
    println!("store dir: {}", store_dir.display());

    // 1. Parse + contract-check the authority fixture.
    let fixture: AuthorityFixture = serde_json::from_slice(&std::fs::read(&fixture_path)?)?;
    if fixture.contract != "mind-universe-authority-fixture" || fixture.version != 0 {
        return Err(format!(
            "unsupported fixture: contract={:?} version={} (want mind-universe-authority-fixture v0)",
            fixture.contract, fixture.version
        )
        .into());
    }
    println!(
        "authority fixture: {} entities, {} relations",
        fixture.entities.len(),
        fixture.relations.len()
    );

    // 2. Open the LIVE store and replay to the authoritative snapshot.
    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;
    println!(
        "\nbase revision: {} | universe: {} | entities: {} | relations: {}",
        base_revision.0,
        snapshot.universe,
        snapshot.entities.len(),
        snapshot.relations.len()
    );
    if fixture.universe != snapshot.universe {
        return Err(format!(
            "fixture targets universe {} but the live store is universe {}",
            fixture.universe, snapshot.universe
        )
        .into());
    }

    // 3. Additive-only guards: no duplicate keys within the fixture, no collision
    //    with the live store, and every relation endpoint is known (base + fixture).
    let base_entities: BTreeSet<EntityKey> =
        snapshot.entities.iter().map(|entity| entity.key).collect();
    let base_relations: BTreeSet<_> =
        snapshot.relations.iter().map(|relation| relation.key).collect();
    let mut fixture_entities = BTreeSet::new();
    for entity in &fixture.entities {
        if !fixture_entities.insert(entity.key) {
            return Err(format!("fixture has duplicate entity key {}", entity.key).into());
        }
        if base_entities.contains(&entity.key) {
            return Err(format!(
                "entity key {} already exists in the live store (no overwrite)",
                entity.key
            )
            .into());
        }
    }
    let mut fixture_relations = BTreeSet::new();
    for relation in &fixture.relations {
        if !fixture_relations.insert(relation.key) {
            return Err(format!("fixture has duplicate relation key {}", relation.key).into());
        }
        if base_relations.contains(&relation.key) {
            return Err(format!(
                "relation key {} already exists in the live store (no overwrite)",
                relation.key
            )
            .into());
        }
    }
    let known_entities: BTreeSet<EntityKey> =
        base_entities.union(&fixture_entities).copied().collect();
    for relation in &fixture.relations {
        if !known_entities.contains(&relation.source) || !known_entities.contains(&relation.target) {
            return Err(format!(
                "relation {} has an unknown endpoint ({} -> {})",
                relation.key, relation.source, relation.target
            )
            .into());
        }
    }
    println!(
        "additive guard OK: {} entity keys + {} relation keys are all free in the live store",
        fixture_entities.len(),
        fixture_relations.len()
    );

    // 4. Symbol conformance: intern ZERO new symbols. The fixture rides only
    //    canonical predicates (the grant's authored HOLDS_CAPABILITY is already
    //    remapped to USED in the file). A non-canonical symbol is a hard error.
    let mut requested = fixture.symbols.clone();
    requested.extend(fixture.entities.iter().map(|entity| entity.symbol.clone()));
    requested.extend(fixture.relations.iter().map(|relation| relation.predicate.clone()));
    requested.sort();
    requested.dedup();
    let plan = snapshot.plan_symbol_interning(&requested)?;
    if !plan.additions.is_empty() {
        return Err(format!(
            "conformance violation: injection would intern new symbols {:?} (expected none)",
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

    // 5. Build the atomic write-set: entities (verbatim content), then relations.
    let mut commands = Vec::with_capacity(fixture.entities.len() + fixture.relations.len());
    for entity in &fixture.entities {
        commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: entity.key,
                generation: entity.generation,
                symbol: sym(&entity.symbol)?,
                content: Some(store.append_content(&entity.content)?),
            },
        });
    }
    for relation in &fixture.relations {
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: relation.key,
                generation: relation.generation,
                source: relation.source,
                target: relation.target,
                predicate: sym(&relation.predicate)?,
                content: relation
                    .content
                    .as_ref()
                    .map(|content| store.append_content(content))
                    .transpose()?,
            },
        });
    }

    // 6. Prepare + commit as ONE atomic transaction at a tick boundary.
    let stem = fixture_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("authority-fixture");
    let command_count = commands.len();
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: format!("mutation:inject-authority-fixture:{stem}"),
        causal_ancestry: vec![format!("changeset:{stem}")],
        commands,
    };
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("\ncommitted {command_count} commands as one atomic set");
    println!("commit receipt: {receipt:?}");

    // 7. INDEPENDENT readback: fresh reopen from disk.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    println!("\n-- independent readback (fresh reopen) --");
    println!("revision advanced: {} -> {}", base_revision.0, after.revision.0);
    println!("entities: {} | relations: {}", after.entities.len(), after.relations.len());

    // 7a. Every injected entity is present; report its canonical_id when carried.
    let canonical_of = |snap: &universe_store::UniverseSnapshot, key: EntityKey| -> Option<String> {
        let entity = snap.entities.iter().find(|e| e.key == key)?;
        let content = fresh.read_content(entity.content.as_ref()?).ok()?;
        content
            .get("canonical_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    for entity in &fixture.entities {
        let present = after.entities.iter().find(|e| e.key == entity.key).ok_or_else(|| {
            format!("injected entity {} not found on readback", entity.key)
        })?;
        let content = fresh.read_content(
            present.content.as_ref().ok_or("injected entity has no content on readback")?,
        )?;
        let canonical = content.get("canonical_id").and_then(|v| v.as_str()).unwrap_or("-");
        let kind = content.get("kind").and_then(|v| v.as_str()).unwrap_or("-");
        println!("  entity {} kind={:<26} canonical_id={}", entity.key, kind, canonical);
    }
    println!("all {} injected entities read back", fixture.entities.len());

    // 7b. Every grant edge is present; resolve actor -> capability from content so
    //     the read that later drives the sealed hatch is proven materializable.
    let used_symbol = after
        .symbol_id("USED")
        .ok_or("canonical predicate 'USED' is not interned in this store")?;
    for relation in &fixture.relations {
        let predicate_symbol = sym(&relation.predicate)?;
        let present = after.relations.iter().any(|r| {
            r.source == relation.source
                && r.target == relation.target
                && r.predicate == predicate_symbol
        });
        if !present {
            return Err(format!(
                "injected relation {} ({} -[{}]-> {}) missing on readback",
                relation.key, relation.source, relation.predicate, relation.target
            )
            .into());
        }
        if predicate_symbol == used_symbol {
            let actor = canonical_of(&after, relation.source).unwrap_or_else(|| "-".into());
            // The capability an actor holds is the target entity's `capability`.
            let capability = after
                .entities
                .iter()
                .find(|e| e.key == relation.target)
                .and_then(|e| e.content.as_ref())
                .and_then(|ptr| fresh.read_content(ptr).ok())
                .and_then(|c| c.get("capability").and_then(|v| v.as_str()).map(str::to_string))
                .unwrap_or_else(|| "-".into());
            println!("  GRANT  {actor}  --USED-->  {capability}");
        }
    }
    println!("all {} injected relations read back", fixture.relations.len());

    println!(
        "\nRESULT: injected authority fixture {stem} ({} entities, {} relations) into the LIVE store",
        fixture.entities.len(),
        fixture.relations.len()
    );
    println!("        as one atomic transaction, interned 0 new symbols, and read the grant edges back.");
    println!("        The grant is DATA; whether a mutate is admitted is decided by the sealed port against the read set.");
    Ok(())
}
