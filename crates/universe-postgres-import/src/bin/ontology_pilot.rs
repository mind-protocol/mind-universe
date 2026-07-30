use std::path::PathBuf;
use universe_core::UniverseError;
use universe_postgres_import::ontology_pilot::{load_manifest, run_ontology_pilot};

/// Runs the bounded ontology-adaptation pilot over a declared manifest and
/// writes its measured evidence. Activation is authorized only by the manifest's
/// approved, scoped, revision-pinned ChangeSet; re-running is idempotent.
fn main() -> Result<(), UniverseError> {
    let mut args = std::env::args_os().skip(1);
    let manifest_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    std::fs::create_dir_all(&artifact_dir).map_err(|error| UniverseError::Io(error.to_string()))?;

    let manifest = load_manifest(manifest_path)?;
    let evidence = run_ontology_pilot(&manifest, artifact_dir.join("store"))?;
    let bytes = serde_json::to_vec_pretty(&evidence)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(artifact_dir.join("ontology-pilot-evidence.json"), bytes)
        .map_err(|error| UniverseError::Io(error.to_string()))?;

    println!(
        "ontology-pilot change={} activated={} compatibility={} unresolved={} quarantined={} members={} code_activated={}",
        evidence.change_id,
        evidence.activated,
        evidence.compatibility,
        evidence.unresolved,
        evidence.quarantined,
        evidence.changeset_members,
        evidence.code_bindings_activated
    );
    Ok(())
}

fn usage() -> UniverseError {
    UniverseError::Validation("usage: ontology_pilot <manifest.json> <artifact-dir>".into())
}
