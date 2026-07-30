use std::path::PathBuf;
use universe_core::UniverseError;
use universe_postgres_import::code_pilot::{load_manifest, receipt_content, run_code_pilot};

/// Runs the bounded inert code-Node import pilot: it imports source
/// `code_definition` and related code symbols as inert, non-executable Nodes with
/// full provenance, quarantines each from activation via the approved import
/// ChangeSet, and writes measured evidence plus the adaptation receipt. It
/// imports no code payload and makes nothing runnable. Re-running is idempotent.
fn main() -> Result<(), UniverseError> {
    let mut args = std::env::args_os().skip(1);
    let manifest_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    std::fs::create_dir_all(&artifact_dir).map_err(|error| UniverseError::Io(error.to_string()))?;

    let manifest = load_manifest(manifest_path)?;
    let evidence = run_code_pilot(&manifest, artifact_dir.join("store"))?;

    let evidence_bytes = serde_json::to_vec_pretty(&evidence)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(
        artifact_dir.join("code-pilot-evidence.json"),
        evidence_bytes,
    )
    .map_err(|error| UniverseError::Io(error.to_string()))?;

    // The receipt is deterministic; write the exact bytes that were stored so the
    // artifact can be independently compared against the store.
    let receipt_bytes = serde_json::to_vec_pretty(&receipt_content(&manifest))
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(artifact_dir.join("code-pilot-receipt.json"), receipt_bytes)
        .map_err(|error| UniverseError::Io(error.to_string()))?;

    println!(
        "code-pilot change={} batch={} nodes={} imported_inert={} quarantined={} members={} provenance={} refused={} executable={} dispatchable={} activated={}",
        evidence.change_id,
        evidence.import_batch,
        evidence.total_nodes,
        evidence.imported_inert,
        evidence.quarantined_from_activation,
        evidence.changeset_members,
        evidence.provenance_complete,
        evidence.activation_attempts_refused,
        evidence.executable_count,
        evidence.dispatchable_count,
        evidence.activated_count
    );
    Ok(())
}

fn usage() -> UniverseError {
    UniverseError::Validation("usage: code_pilot <manifest.json> <artifact-dir>".into())
}
