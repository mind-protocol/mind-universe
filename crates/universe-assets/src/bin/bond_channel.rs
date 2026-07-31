use std::path::PathBuf;
use universe_assets::bond_channel::{materialize, BondChannelGrammar};
use universe_core::UniverseError;

/// Materializes the bond-channel grammar (ALIGN.md §2) as a content-addressed
/// Asset, reads it back independently (byte-parity + the honesty/membrane
/// invariants), and writes the receipt as evidence. Re-running is idempotent.
fn main() -> Result<(), UniverseError> {
    let mut args = std::env::args_os().skip(1);
    let grammar_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let artifact_dir = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    std::fs::create_dir_all(&artifact_dir).map_err(|error| UniverseError::Io(error.to_string()))?;

    let grammar = BondChannelGrammar::load(&grammar_path)?;
    let receipt = materialize(artifact_dir.join("store"), &grammar)?;
    let bytes = serde_json::to_vec_pretty(&receipt)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    std::fs::write(artifact_dir.join("bond-channel-receipt.json"), bytes)
        .map_err(|error| UniverseError::Io(error.to_string()))?;

    println!(
        "bond-channel grammar={} newly_committed={} parity={} static={} dynamic={} energy_requires_measured={}",
        receipt.grammar_id,
        receipt.newly_committed,
        receipt.parity,
        receipt.static_channels,
        receipt.dynamic_channels,
        receipt.energy_requires_measured
    );
    Ok(())
}

fn usage() -> UniverseError {
    UniverseError::Validation("usage: bond_channel <grammar.json> <artifact-dir>".into())
}
