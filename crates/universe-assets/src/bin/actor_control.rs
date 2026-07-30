use std::path::PathBuf;
use universe_assets::actor_control::{materialize, ActorControlBounds};
use universe_core::UniverseError;

/// Materializes the graph-native actor-control bounds authority from the declared
/// fixture, reads it back independently (proving byte parity with the renderer
/// fixture and that exactly one Actor is bound), and writes the receipt as
/// evidence. Re-running is idempotent.
fn main() -> Result<(), UniverseError> {
    let mut args = std::env::args_os().skip(1);
    let bounds_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    std::fs::create_dir_all(&artifact_dir).map_err(|error| UniverseError::Io(error.to_string()))?;
    let store_root = artifact_dir.join("store");

    let bounds = ActorControlBounds::load(&bounds_path)?;
    let receipt = materialize(&store_root, &bounds)?;

    let bytes = serde_json::to_vec_pretty(&receipt)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(
        artifact_dir.join("actor-control-bounds-receipt.json"),
        bytes,
    )
    .map_err(|error| UniverseError::Io(error.to_string()))?;

    println!(
        "actor-control authority={} bounds={} actor={} gate={} newly_committed={} parity={} nodes_preserved={} bindings={}",
        receipt.authority_id,
        receipt.bounds_id,
        receipt.bound_actor,
        receipt.gate_rule,
        receipt.newly_committed,
        receipt.catalog_parity,
        receipt.nodes_preserved,
        receipt.bindings
    );
    Ok(())
}

fn usage() -> UniverseError {
    UniverseError::Validation("usage: actor_control <bounds.json> <artifact-dir>".into())
}
