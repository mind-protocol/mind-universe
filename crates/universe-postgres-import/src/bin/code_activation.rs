use std::path::PathBuf;
use universe_core::UniverseError;
use universe_postgres_import::code_activation::run_activation;
use universe_postgres_import::code_translation::load_manifest;

/// Activates the shadow-compared reconciliation candidate for LATER execution by
/// committing an approved ChangeSet that pins an enabled TriggerSubscription to
/// the compiled CodeDefinition. It gates on the full evidence chain and fires
/// nothing.
fn main() -> Result<(), UniverseError> {
    let mut args = std::env::args_os().skip(1);
    let manifest_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    std::fs::create_dir_all(&artifact_dir).map_err(|error| UniverseError::Io(error.to_string()))?;

    let manifest = load_manifest(manifest_path)?;
    let evidence = run_activation(&manifest, artifact_dir.join("store"))?;
    let bytes = serde_json::to_vec_pretty(&evidence)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(artifact_dir.join("code-activation-evidence.json"), bytes)
        .map_err(|error| UniverseError::Io(error.to_string()))?;

    println!(
        "code-activation change={} activatable={} activated={} subscription_valid={} enabled={} executions_now={} state={}",
        evidence.change_id,
        evidence.activatable,
        evidence.activated,
        evidence.subscription_valid,
        evidence.subscription_enabled,
        evidence.executions_now,
        evidence.state_reached
    );
    Ok(())
}

fn usage() -> UniverseError {
    UniverseError::Validation("usage: code_activation <translation-manifest.json> <artifact-dir>".into())
}
