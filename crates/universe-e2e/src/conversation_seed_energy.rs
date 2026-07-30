//! MEASURED semantic seeding of atom energy over real CONVERSATION content.
//!
//! Same pipeline as [`crate::canonical_seed_energy`] (embed a sentence, compare
//! against the `activation-propensity-v0` anchor pair, `tanh` into a bounded
//! propensity, unit-convert to an AtomBond) — but the relations come from
//! conversation **reply-pairs** (`parent message -> child message`) instead of
//! canonical type edges.
//!
//! Why: the canonical type-ontology collapses 784 relations to ~14 distinct
//! sentences, so its rideable terrain is flat. Lived conversation text does not
//! collapse — measured on a real export, 17k messages give ~11.7k distinct
//! reply-pairs. This seeder turns that rich text into a measured energy overlay
//! the board can actually ride.
//!
//! The heavy lifting (anchors, `tanh`, bond conversion, epistemic tag) is shared
//! through [`SeedContext`], so canonical and conversation seeds can never drift.
//! Input is already-parsed conversation JSON (ChatGPT/OpenAI export shape), so
//! the caller owns the source: a tiny synthetic fixture in tests, the real
//! export in a local bin. No conversation text is embedded in this file.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use universe_embeddings::EmbeddingProvider;
use universe_physics::BondPolarity;

use crate::canonical_seed_energy::{SeedContext, ANCHORS_ID};
use crate::E2eError;

/// One measured reply-pair. `sentence` is the exact text measured (provenance).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConversationBond {
    pub conversation_id: String,
    pub parent_role: String,
    pub child_role: String,
    pub sentence: String,
    pub cos_positive_micro: i64,
    pub cos_negative_micro: i64,
    pub propensity_micro: i64,
    pub polarity: BondPolarity,
    pub energy: u64,
}

/// The measured energy overlay over conversation reply-pairs. Separate from any
/// canonical store; carries model + anchor provenance and the `measured:semantic_v0`
/// tag so it is never mistaken for lived-activation energy or canonical truth.
#[derive(Clone, Debug, Serialize)]
pub struct ConversationSeedEnergy {
    pub epistemic_status: String,
    pub model: String,
    pub model_revision: String,
    pub anchors_id: String,
    pub conversations: usize,
    pub reply_pairs_measured: usize,
    pub support_bonds: usize,
    pub inhibit_bonds: usize,
    pub neutral_bonds: usize,
    /// Distinct propensity values — the rideable terrain's resolution.
    pub distinct_propensities: usize,
    /// Distinct reply-pair sentences — the upper bound on resolution.
    pub distinct_sentences: usize,
    /// Retained sample of bonds (bounded by `max_bonds_retained`; may be empty
    /// when a caller wants counts only and no text held).
    pub bonds: Vec<ConversationBond>,
}

struct ReplyPair {
    parent_role: String,
    parent_text: String,
    child_role: String,
    child_text: String,
}

fn bounded(text: &str, max_chars: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

/// Extract `(parent message -> child message)` reply-pairs from one conversation
/// in the OpenAI export shape: `mapping[node] = { message, parent, children }`,
/// `message.content = { content_type: "text", parts: [..] }`. Deterministic:
/// nodes are visited in sorted key order.
fn reply_pairs(conversation: &Value, max_chars: usize) -> Vec<ReplyPair> {
    let Some(mapping) = conversation.get("mapping").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut node_ids: Vec<&String> = mapping.keys().collect();
    node_ids.sort();

    let text_of = |node: &Value| -> Option<(String, String)> {
        let message = node.get("message")?;
        if message.is_null() {
            return None;
        }
        let content = message.get("content")?;
        if content.get("content_type").and_then(Value::as_str) != Some("text") {
            return None;
        }
        let role = message
            .get("author")
            .and_then(|author| author.get("role"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let text = content
            .get("parts")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("");
        let text = bounded(text.trim(), max_chars);
        (!text.is_empty()).then_some((role, text))
    };

    let mut resolved: std::collections::BTreeMap<&str, (String, String)> =
        std::collections::BTreeMap::new();
    for id in &node_ids {
        if let Some(pair) = text_of(&mapping[*id]) {
            resolved.insert(id.as_str(), pair);
        }
    }

    let mut pairs = Vec::new();
    for id in &node_ids {
        let Some((child_role, child_text)) = resolved.get(id.as_str()) else {
            continue;
        };
        let Some(parent) = mapping[*id].get("parent").and_then(Value::as_str) else {
            continue;
        };
        if let Some((parent_role, parent_text)) = resolved.get(parent) {
            pairs.push(ReplyPair {
                parent_role: parent_role.clone(),
                parent_text: parent_text.clone(),
                child_role: child_role.clone(),
                child_text: child_text.clone(),
            });
        }
    }
    pairs
}

/// Seed a measured energy overlay from parsed conversations. `max_bonds_retained`
/// bounds how many bond records (with text) are kept in the result — pass 0 for
/// counts-only (no conversation text retained), a small N to keep a sample.
pub fn seed_energy_from_conversations<P: EmbeddingProvider>(
    conversations: &[Value],
    provider: P,
    dimensions: usize,
    max_chars_per_message: usize,
    max_bonds_retained: usize,
) -> Result<ConversationSeedEnergy, E2eError> {
    let mut context = SeedContext::new(provider, dimensions)?;
    let model = context.model.clone();
    let model_revision = context.model_revision.clone();

    // Gather every reply-pair first, then measure the whole batch in one encode
    // call — so a real (subprocess) embedder loads its model once, not per pair.
    struct Pending {
        conversation_id: String,
        parent_role: String,
        child_role: String,
        sentence: String,
    }
    let mut pending: Vec<Pending> = Vec::new();
    for conversation in conversations {
        let conversation_id = conversation
            .get("conversation_id")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>")
            .to_owned();
        for pair in reply_pairs(conversation, max_chars_per_message) {
            pending.push(Pending {
                conversation_id: conversation_id.clone(),
                sentence: format!(
                    "Message ({}): {}. Reply ({}): {}.",
                    pair.parent_role, pair.parent_text, pair.child_role, pair.child_text
                ),
                parent_role: pair.parent_role,
                child_role: pair.child_role,
            });
        }
    }
    let sentences: Vec<String> = pending.iter().map(|item| item.sentence.clone()).collect();
    let scalars = context.measure_batch(&sentences)?;

    let mut bonds = Vec::new();
    let (mut support, mut inhibit, mut neutral) = (0usize, 0usize, 0usize);
    let mut distinct_propensities = BTreeSet::new();
    let mut distinct_sentences = BTreeSet::new();
    let reply_pairs_measured = pending.len();

    for (item, scalar) in pending.into_iter().zip(scalars) {
        distinct_propensities.insert(scalar.propensity_micro);
        distinct_sentences.insert(item.sentence.clone());
        match scalar.polarity {
            BondPolarity::Support => support += 1,
            BondPolarity::Inhibit => inhibit += 1,
            BondPolarity::Neutral => neutral += 1,
        }
        if bonds.len() < max_bonds_retained {
            bonds.push(ConversationBond {
                conversation_id: item.conversation_id,
                parent_role: item.parent_role,
                child_role: item.child_role,
                sentence: item.sentence,
                cos_positive_micro: scalar.cos_positive_micro,
                cos_negative_micro: scalar.cos_negative_micro,
                propensity_micro: scalar.propensity_micro,
                polarity: scalar.polarity,
                energy: scalar.energy,
            });
        }
    }

    Ok(ConversationSeedEnergy {
        epistemic_status: "measured:semantic_v0".to_owned(),
        model,
        model_revision,
        anchors_id: ANCHORS_ID.to_owned(),
        conversations: conversations.len(),
        reply_pairs_measured,
        support_bonds: support,
        inhibit_bonds: inhibit,
        neutral_bonds: neutral,
        distinct_propensities: distinct_propensities.len(),
        distinct_sentences: distinct_sentences.len(),
        bonds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical_seed_energy::{HashingEmbedder, SEED_DIMENSIONS};
    use serde_json::json;
    use universe_embeddings::SCORE_SCALE;

    /// A synthetic conversation in the OpenAI export shape. Text is invented —
    /// no real conversation content lives in the repo.
    fn conversation(id: &str, nodes: &[(&str, Option<&str>, &str, &str)]) -> Value {
        let mut mapping = serde_json::Map::new();
        for (node_id, parent, role, text) in nodes {
            mapping.insert(
                (*node_id).to_owned(),
                json!({
                    "id": node_id,
                    "parent": parent,
                    "message": {
                        "author": { "role": role },
                        "content": { "content_type": "text", "parts": [text] }
                    }
                }),
            );
        }
        json!({ "conversation_id": id, "mapping": mapping })
    }

    fn corpus() -> Vec<Value> {
        vec![
            conversation(
                "conv-a",
                &[
                    (
                        "n0",
                        None,
                        "user",
                        "How does the membrane admit a stimulus?",
                    ),
                    (
                        "n1",
                        Some("n0"),
                        "assistant",
                        "It gates energy through a bounded frontier.",
                    ),
                    (
                        "n2",
                        Some("n1"),
                        "user",
                        "And what blocks a contradiction from propagating?",
                    ),
                    (
                        "n3",
                        Some("n2"),
                        "assistant",
                        "Inhibitory bonds raise the firing threshold.",
                    ),
                ],
            ),
            conversation(
                "conv-b",
                &[
                    (
                        "m0",
                        None,
                        "user",
                        "Seed the board energy from real recency signal.",
                    ),
                    (
                        "m1",
                        Some("m0"),
                        "assistant",
                        "Recency maps to seed energy; provenance stays measured.",
                    ),
                ],
            ),
        ]
    }

    #[test]
    fn seeds_measured_energy_from_conversation_reply_pairs() {
        let convos = corpus();
        let seed = seed_energy_from_conversations(
            &convos,
            HashingEmbedder {
                dimensions: SEED_DIMENSIONS,
            },
            SEED_DIMENSIONS,
            400,
            16,
        )
        .unwrap();

        assert_eq!(seed.epistemic_status, "measured:semantic_v0");
        assert_eq!(seed.conversations, 2);
        // conv-a has 3 reply-pairs, conv-b has 1 -> 4 total.
        assert_eq!(seed.reply_pairs_measured, 4);
        assert_eq!(
            seed.support_bonds + seed.inhibit_bonds + seed.neutral_bonds,
            seed.reply_pairs_measured
        );

        // The whole point: lived text differentiates, unlike the canonical
        // graph's 14-bucket collapse. Every distinct sentence here is distinct.
        assert!(
            seed.distinct_propensities >= 3,
            "conversation text must yield a rich terrain, got {} distinct",
            seed.distinct_propensities
        );
        assert!(seed.distinct_propensities <= seed.distinct_sentences);

        for bond in &seed.bonds {
            assert!(bond.propensity_micro.abs() <= SCORE_SCALE);
            match bond.polarity {
                BondPolarity::Support => {
                    assert!(bond.energy > 0 && bond.propensity_micro > 0)
                }
                BondPolarity::Inhibit => {
                    assert!(bond.energy > 0 && bond.propensity_micro < 0)
                }
                BondPolarity::Neutral => assert_eq!(bond.energy, 0),
            }
        }

        // Deterministic: same corpus + embedder -> identical seed.
        let seed2 = seed_energy_from_conversations(
            &corpus(),
            HashingEmbedder {
                dimensions: SEED_DIMENSIONS,
            },
            SEED_DIMENSIONS,
            400,
            16,
        )
        .unwrap();
        assert_eq!(seed.bonds, seed2.bonds);
        assert_eq!(seed.distinct_propensities, seed2.distinct_propensities);
    }

    #[test]
    fn counts_only_retains_no_conversation_text() {
        let seed = seed_energy_from_conversations(
            &corpus(),
            HashingEmbedder {
                dimensions: SEED_DIMENSIONS,
            },
            SEED_DIMENSIONS,
            400,
            0,
        )
        .unwrap();
        // Counts are produced, but no bond text is retained (privacy-safe path).
        assert_eq!(seed.reply_pairs_measured, 4);
        assert!(seed.bonds.is_empty());
    }
}
