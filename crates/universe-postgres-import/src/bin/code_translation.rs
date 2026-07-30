use std::path::PathBuf;
use universe_core::UniverseError;
use universe_postgres_import::code_translation::{load_manifest, run_translation};

/// Translates the reconciliation candidate into a real Graph-IR CodeDefinition,
/// stores it as graph data, then compiles and shadow-executes it on the
/// fuel-bounded VM (mutation-free), comparing against the declared contract.
/// It activates nothing and applies no proposal.
fn main() -> Result<(), UniverseError> {
    let mut args = std::env::args_os().skip(1);
    let manifest_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    std::fs::create_dir_all(&artifact_dir).map_err(|error| UniverseError::Io(error.to_string()))?;

    let manifest = load_manifest(manifest_path)?;
    let evidence = run_translation(&manifest, artifact_dir.join("store"))?;
    let bytes = serde_json::to_vec_pretty(&evidence)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(artifact_dir.join("code-translation-evidence.json"), bytes)
        .map_err(|error| UniverseError::Io(error.to_string()))?;

    println!(
        "code-translation batch={} compiled={} shadow_executed={} deterministic={} equivalent={} fuel_used={} proposals={} state={} activated={}",
        evidence.batch_id,
        evidence.compiled,
        evidence.shadow_executed,
        evidence.deterministic,
        evidence.equivalent,
        evidence.fuel_used,
        evidence.proposal_count,
        evidence.state_reached,
        evidence.activated
    );
    Ok(())
}

fn usage() -> UniverseError {
    UniverseError::Validation("usage: code_translation <manifest.json> <artifact-dir>".into())
}
