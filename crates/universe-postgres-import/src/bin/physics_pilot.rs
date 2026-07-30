use std::path::PathBuf;
use universe_core::UniverseError;
use universe_postgres_import::physics_pilot::{load_manifest, run_physics_pilot};

/// Imports source physical profiles as INERT `imported_physics_profile` Nodes,
/// adapted to the ontology schema through one approved, source-graph-scoped,
/// idempotent ChangeSet — never binding them to the live physics simulation —
/// then reads the result back independently and writes the receipt as evidence.
fn main() -> Result<(), UniverseError> {
    let mut args = std::env::args_os().skip(1);
    let manifest_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    std::fs::create_dir_all(&artifact_dir).map_err(|error| UniverseError::Io(error.to_string()))?;
    let store_root = artifact_dir.join("store");

    let manifest = load_manifest(&manifest_path)?;
    let evidence = run_physics_pilot(&manifest, &store_root)?;
    let bytes = serde_json::to_vec_pretty(&evidence)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(artifact_dir.join("physics-pilot-evidence.json"), bytes)
        .map_err(|error| UniverseError::Io(error.to_string()))?;

    println!(
        "physics-pilot change={} profiles={} adapted_inert={} compatibility={} unresolved={} quarantined={} members={} quarantined_from_barrier={} activated=0 materialized=0 refusals={} provenance={}",
        evidence.change_id,
        evidence.total_profiles,
        evidence.adapted_inert,
        evidence.compatibility,
        evidence.unresolved,
        evidence.quarantined,
        evidence.changeset_members,
        evidence.barrier_quarantined,
        evidence.activation_refusals,
        evidence.provenance_complete
    );
    Ok(())
}

fn usage() -> UniverseError {
    UniverseError::Validation("usage: physics_pilot <manifest.json> <artifact-dir>".into())
}
