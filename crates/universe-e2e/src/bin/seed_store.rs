//! Installs a `GraphSeed` JSON into a fresh Universe store — a small utility so
//! demonstrations (e.g. a citizen world for the desktop projector) can stand up a
//! real store from a committed fixture. Idempotent: an already-seeded store is
//! left untouched.

use std::{env, error::Error, path::PathBuf};
use universe_store::{GraphSeed, UniverseStore};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let seed_path = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: seed_store <seed.json> <store-dir>")?;
    let store_root = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: seed_store <seed.json> <store-dir>")?;
    if args.next().is_some() {
        return Err("usage: seed_store <seed.json> <store-dir>".into());
    }

    let store = UniverseStore::open(&store_root)?;
    if store_root.join("snapshot.json").exists() {
        println!("store already seeded at {}", store_root.display());
        return Ok(());
    }
    let seed: GraphSeed = serde_json::from_slice(&std::fs::read(&seed_path)?)?;
    let snapshot = store.install_seed(&seed)?;
    println!(
        "seeded universe={} entities={} relations={}",
        snapshot.universe,
        snapshot.entities.len(),
        snapshot.relations.len()
    );
    Ok(())
}
