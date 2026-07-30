//! Run the conversation energy seeder over real exported conversations and
//! print MEASURED terrain counts. Counts-only: no conversation text is retained
//! or written anywhere. Input is one or more extracted `conversations-*.json`
//! files (OpenAI export shape).
//!
//! usage: conversation_seed_energy <conversations-000.json> [more.json ...]

use serde_json::Value;
use std::{env, error::Error, fs, path::PathBuf};
use universe_e2e::canonical_seed_energy::{HashingEmbedder, SEED_DIMENSIONS};
use universe_e2e::conversation_seed_energy::seed_energy_from_conversations;

const MAX_CHARS_PER_MESSAGE: usize = 500;

fn main() -> Result<(), Box<dyn Error>> {
    let paths: Vec<PathBuf> = env::args_os().skip(1).map(PathBuf::from).collect();
    if paths.is_empty() {
        eprintln!("usage: conversation_seed_energy <conversations-*.json> [more.json ...]");
        std::process::exit(2);
    }

    let mut conversations: Vec<Value> = Vec::new();
    for path in &paths {
        let value: Value = serde_json::from_slice(&fs::read(path)?)?;
        match value {
            Value::Array(items) => conversations.extend(items),
            other => conversations.push(other),
        }
    }

    let seed = seed_energy_from_conversations(
        &conversations,
        HashingEmbedder {
            dimensions: SEED_DIMENSIONS,
        },
        SEED_DIMENSIONS,
        MAX_CHARS_PER_MESSAGE,
        0, // counts-only: retain no conversation text
    )
    .map_err(|error| format!("{error:?}"))?;

    println!(
        "conversation_seed epistemic={} model={} anchors={}",
        seed.epistemic_status, seed.model, seed.anchors_id
    );
    println!(
        "  conversations={} reply_pairs={} support={} inhibit={} neutral={}",
        seed.conversations,
        seed.reply_pairs_measured,
        seed.support_bonds,
        seed.inhibit_bonds,
        seed.neutral_bonds
    );
    println!(
        "  distinct_propensities={} distinct_sentences={}",
        seed.distinct_propensities, seed.distinct_sentences
    );
    Ok(())
}
