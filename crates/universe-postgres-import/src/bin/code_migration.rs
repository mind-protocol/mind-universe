use std::path::PathBuf;
use universe_core::UniverseError;
use universe_postgres_import::code_migration::{load_manifest, run_code_migration};

/// Runs the bounded code-migration classification pilot: it creates inert
/// LegacyCodeAssets and CodeMigrationTasks, applies the safety gate, and writes
/// measured evidence. It compiles, executes, and activates nothing.
fn main() -> Result<(), UniverseError> {
    let mut args = std::env::args_os().skip(1);
    let manifest_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    std::fs::create_dir_all(&artifact_dir).map_err(|error| UniverseError::Io(error.to_string()))?;

    let manifest = load_manifest(manifest_path)?;
    let evidence = run_code_migration(&manifest, artifact_dir.join("store"))?;
    let bytes = serde_json::to_vec_pretty(&evidence)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(artifact_dir.join("code-migration-evidence.json"), bytes)
        .map_err(|error| UniverseError::Io(error.to_string()))?;

    println!(
        "code-migration batch={} accepted_inert={} rejected={} quarantined={} translated={} migration_required={} executable={} activated={}",
        evidence.batch_id,
        evidence.accepted_inert,
        evidence.rejected,
        evidence.quarantined,
        evidence.translated_candidates,
        evidence.migration_required,
        evidence.executable_count,
        evidence.activated_count
    );
    Ok(())
}

fn usage() -> UniverseError {
    UniverseError::Validation("usage: code_migration <manifest.json> <artifact-dir>".into())
}
