use std::path::PathBuf;
use universe_assets::visual::{materialize, VisualCatalog, VisualPolicy};
use universe_core::UniverseError;

/// Materializes the graph-native visual embodiment mapping authority from the
/// declared catalog + projection policy, reads it back independently (proving
/// parity with the renderer fixture and the epistemic-honesty invariant), and
/// writes the receipt as evidence. Re-running is idempotent.
fn main() -> Result<(), UniverseError> {
    let mut args = std::env::args_os().skip(1);
    let catalog_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let policy_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    std::fs::create_dir_all(&artifact_dir).map_err(|error| UniverseError::Io(error.to_string()))?;
    let store_root = artifact_dir.join("store");

    let catalog = VisualCatalog::load(&catalog_path)?;
    let policy = VisualPolicy::load(&policy_path)?;
    let receipt = materialize(&store_root, &catalog, &policy)?;

    let bytes = serde_json::to_vec_pretty(&receipt)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(artifact_dir.join("visual-mapping-receipt.json"), bytes)
        .map_err(|error| UniverseError::Io(error.to_string()))?;

    println!(
        "visual authority={} mapping={} newly_committed={} parity={} nodes_preserved={} bindings={} resolutions={} honest={}",
        receipt.authority_id,
        receipt.mapping_id,
        receipt.newly_committed,
        receipt.catalog_parity,
        receipt.nodes_preserved,
        receipt.bindings,
        receipt.resolutions_checked,
        receipt.honesty_invariant_held
    );
    Ok(())
}

fn usage() -> UniverseError {
    UniverseError::Validation("usage: visual <catalog.json> <policy.json> <artifact-dir>".into())
}
