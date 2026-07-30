use std::path::PathBuf;
use universe_assets::census::{census_with_readback, CensusPolicy};
use universe_core::UniverseError;
use universe_store::{load_seed, UniverseStore};

/// Reconstructs a canonical Universe from a GraphSeed, then runs a read-only
/// Node→Asset conversion census over it and writes the receipt as evidence. The
/// store is never mutated by the census itself.
fn main() -> Result<(), UniverseError> {
    let mut args = std::env::args_os().skip(1);
    let seed_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let policy_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    std::fs::create_dir_all(&artifact_dir).map_err(|error| UniverseError::Io(error.to_string()))?;
    let store_root = artifact_dir.join("store");

    let seed = load_seed(&seed_path)?;
    let store = UniverseStore::open(&store_root)?;
    if !store_root.join("snapshot.json").exists() {
        store.install_seed(&seed)?;
    }

    let policy = CensusPolicy::load(&policy_path)?;
    let receipt = census_with_readback(&store_root, &policy)?;
    let bytes = serde_json::to_vec_pretty(&receipt)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(artifact_dir.join("census-receipt.json"), bytes)
        .map_err(|error| UniverseError::Io(error.to_string()))?;

    println!(
        "census policy={} nodes={} classes={:?} requirements={:?}",
        receipt.policy_id, receipt.total_nodes, receipt.class_counts, receipt.requirement_counts
    );
    Ok(())
}

fn usage() -> UniverseError {
    UniverseError::Validation("usage: census <seed.json> <policy.json> <artifact-dir>".into())
}
