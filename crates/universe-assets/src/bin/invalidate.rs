use std::path::PathBuf;
use universe_assets::invalidation::run_invalidation;
use universe_core::UniverseError;

/// Runs a live, recorded Asset rebuild/invalidation: seeds a Node with a
/// `current` Asset, advances the authoritative mapping revision, rebuilds a new
/// `current` Asset, and supersedes the old one — then reads the transition back
/// independently and writes the receipt. Re-running is idempotent.
fn main() -> Result<(), UniverseError> {
    let mut args = std::env::args_os().skip(1);
    let artifact_dir = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    std::fs::create_dir_all(&artifact_dir).map_err(|error| UniverseError::Io(error.to_string()))?;
    let store_root = artifact_dir.join("store");

    let receipt = run_invalidation(&store_root)?;
    let bytes = serde_json::to_vec_pretty(&receipt)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(artifact_dir.join("invalidation-receipt.json"), bytes)
        .map_err(|error| UniverseError::Io(error.to_string()))?;

    println!(
        "invalidation change={} newly_committed={} trigger={:?} before(current={},stale={}) after(current={},stale={}) transitioned={} node_preserved={}",
        receipt.change_id,
        receipt.newly_committed,
        receipt.trigger,
        receipt.before.current.len(),
        receipt.before.stale.len(),
        receipt.after.current.len(),
        receipt.after.stale.len(),
        receipt.transitioned_to_stale.len(),
        receipt.node_preserved
    );
    Ok(())
}

fn usage() -> UniverseError {
    UniverseError::Validation("usage: invalidate <artifact-dir>".into())
}
