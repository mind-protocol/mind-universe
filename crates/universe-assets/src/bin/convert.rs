use std::path::PathBuf;
use universe_assets::census::CensusPolicy;
use universe_assets::conversion::convert_sources;
use universe_core::UniverseError;
use universe_store::{load_seed, UniverseStore};

/// Reconstructs a canonical Universe, converts its declared `required` source
/// Nodes into Asset projections through one authorized change, and writes the
/// conversion receipt as evidence. Re-running is idempotent.
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
    let receipt = convert_sources(&store_root, &policy)?;
    let bytes = serde_json::to_vec_pretty(&receipt)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(artifact_dir.join("conversion-receipt.json"), bytes)
        .map_err(|error| UniverseError::Io(error.to_string()))?;

    println!(
        "conversion change={} newly_committed={} converted={} nodes_preserved={} before={:?} after={:?}",
        receipt.change_id,
        receipt.newly_committed,
        receipt.converted.len(),
        receipt.nodes_preserved,
        receipt.census_before,
        receipt.census_after
    );
    Ok(())
}

fn usage() -> UniverseError {
    UniverseError::Validation("usage: convert <seed.json> <policy.json> <artifact-dir>".into())
}
