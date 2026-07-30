//! Run the conversation energy seeder over real exported conversations and
//! print MEASURED terrain counts. Counts-only: no conversation text is retained
//! or written anywhere. Input is one or more extracted `conversations-*.json`
//! files (OpenAI export shape).
//!
//! usage: conversation_seed_energy <conversations-000.json> [more.json ...]

use serde_json::Value;
use std::{env, error::Error, fs, path::PathBuf};
use universe_e2e::canonical_seed_energy::{HashingEmbedder, SEED_DIMENSIONS};
use universe_e2e::conversation_seed_energy::{
    seed_energy_from_conversations, ConversationSeedEnergy,
};
use universe_e2e::E2eError;
use universe_embeddings::NodeTransformersProvider;

const MAX_CHARS_PER_MESSAGE: usize = 500;

/// Build the real sentence-transformers provider iff the project's embedding env
/// is configured (module dir with `@xenova/transformers`, a local model cache,
/// and a pinned revision). Otherwise `None` -> deterministic hashing fallback.
/// Mirrors `examples/live_graph_ir.rs`.
fn real_provider() -> Option<(NodeTransformersProvider, usize)> {
    let module_dir = env::var("EMBEDDING_MODULE_DIR").ok()?;
    let cache_dir = env::var("EMBEDDING_CACHE_DIR").ok()?;
    let model_revision = env::var("EMBEDDING_MODEL_REVISION").ok()?;
    let model =
        env::var("EMBEDDING_MODEL").unwrap_or_else(|_| "Xenova/multilingual-e5-small".to_owned());
    let dimensions = env::var("EMBEDDING_DIM")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(384);
    let allow_remote = matches!(
        env::var("EMBEDDING_ALLOW_REMOTE").as_deref(),
        Ok("1" | "true")
    );
    Some((
        NodeTransformersProvider {
            node_executable: PathBuf::from("node"),
            module_dir: PathBuf::from(module_dir),
            cache_dir: PathBuf::from(cache_dir),
            model,
            model_revision,
            prefix: "query: ".to_owned(),
            allow_remote,
        },
        dimensions,
    ))
}

fn main() -> Result<(), Box<dyn Error>> {
    let paths: Vec<PathBuf> = env::args_os().skip(1).map(PathBuf::from).collect();
    if paths.is_empty() {
        eprintln!("usage: conversation_seed_energy <conversations-*.json> [more.json ...]");
        eprintln!("  real embedder: set EMBEDDING_MODULE_DIR, EMBEDDING_CACHE_DIR, EMBEDDING_MODEL_REVISION");
        eprintln!("  otherwise: deterministic hashing embedder (differentiation real, polarity NOT semantic)");
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

    // Counts-only by default (no text retained). EMBEDDING_SAMPLE_BONDS=N keeps
    // and prints N (sentence, polarity) samples — use ONLY on non-private data.
    let retained: usize = env::var("EMBEDDING_SAMPLE_BONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);

    let run =
        |seed: Result<ConversationSeedEnergy, E2eError>| seed.map_err(|error| format!("{error:?}"));
    let seed = match real_provider() {
        Some((provider, dimensions)) => {
            eprintln!(
                "embedder=REAL model={} dims={} allow_remote={}",
                provider.model, dimensions, provider.allow_remote
            );
            run(seed_energy_from_conversations(
                &conversations,
                provider,
                dimensions,
                MAX_CHARS_PER_MESSAGE,
                retained,
            ))?
        }
        None => {
            eprintln!("embedder=HASHING (deterministic; differentiation real, polarity NOT semantic — set EMBEDDING_MODULE_DIR/CACHE_DIR/MODEL_REVISION for the real model)");
            run(seed_energy_from_conversations(
                &conversations,
                HashingEmbedder {
                    dimensions: SEED_DIMENSIONS,
                },
                SEED_DIMENSIONS,
                MAX_CHARS_PER_MESSAGE,
                retained,
            ))?
        }
    };

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
