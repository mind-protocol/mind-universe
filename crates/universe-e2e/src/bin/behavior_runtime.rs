use std::path::PathBuf;
use universe_e2e::behavior_runtime::{default_genesis_path, run, BehaviorRuntimeConfig};

fn main() {
    let artifact_root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/behavior-runtime"));
    match run(&BehaviorRuntimeConfig {
        artifact_root,
        genesis_path: default_genesis_path(),
    }) {
        Ok(manifest) => println!(
            "behavior_runtime_closed correlation={} revision={} snapshot_hash={}",
            manifest.correlation, manifest.final_revision.0, manifest.final_snapshot_hash
        ),
        Err(error) => {
            eprintln!("behavior_runtime_failed: {error:?}");
            std::process::exit(1);
        }
    }
}
