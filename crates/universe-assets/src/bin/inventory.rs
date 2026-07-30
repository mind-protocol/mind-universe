use std::path::PathBuf;
use universe_assets::inventory::{load_inventory_manifest, run_inventory};
use universe_core::UniverseError;

fn main() -> Result<(), UniverseError> {
    let mut args = std::env::args_os().skip(1);
    let manifest_path = args.next().map(PathBuf::from).ok_or_else(|| {
        UniverseError::Validation(
            "usage: inventory <inventory-manifest.json> <artifact-dir>".into(),
        )
    })?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or_else(|| {
        UniverseError::Validation(
            "usage: inventory <inventory-manifest.json> <artifact-dir>".into(),
        )
    })?;
    if args.next().is_some() {
        return Err(UniverseError::Validation(
            "usage: inventory <inventory-manifest.json> <artifact-dir>".into(),
        ));
    }
    std::fs::create_dir_all(&artifact_dir).map_err(|error| UniverseError::Io(error.to_string()))?;
    let manifest = load_inventory_manifest(manifest_path)?;
    let evidence = run_inventory(&manifest, artifact_dir.join("store"))?;
    let bytes = serde_json::to_vec_pretty(&evidence)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(artifact_dir.join("evidence.json"), bytes)
        .map_err(|error| UniverseError::Io(error.to_string()))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&evidence)
            .map_err(|error| UniverseError::CorruptContent(error.to_string()))?
    );
    Ok(())
}
