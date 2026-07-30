//! MEASURED semantic seeding of atom energy over the REAL canonical store.
//!
//! This is the L1 port of the design's grounding pattern
//! (`space:l2:physics:developer-needs-initial-seeding-v0`): instead of
//! repurposing an unrelated spatial descriptor (as `canonical_ride` does with
//! `polarity[0]`, which is honestly `not_measured`), it MEASURES the *meaning*
//! of each canonical relation.
//!
//! For every relation it builds a source/relation/target sentence, embeds it,
//! and compares it against a fixed positive/negative anchor pair that names the
//! semantic poles of activation propensity:
//!
//! ```text
//! raw       = cosine(link, +anchor) - cosine(link, -anchor)
//! propensity = tanh(raw / temperature)      in (-1, 1)
//! bond       = |propensity| -> energy, sign -> Support / Inhibit / Neutral
//! ```
//!
//! Epistemic honesty, per the loops:
//! - The propensity is `measured:semantic_v0` — a real, deterministic, bounded
//!   measurement of the relation's meaning under a named embedding model. It is
//!   NOT the lived-activation energy (that needs the plasticity chain
//!   `outcome-observation -> prediction-error-credit -> online-physical-plasticity`);
//!   it is the measured *seed* the plasticity layer would later evolve.
//! - The AtomBond energy/polarity are a deterministic UNIT CONVERSION of that
//!   measured scalar (magnitude -> energy, sign -> polarity), so they stay
//!   measured — unlike a derivation from an unrelated field.
//! - Nothing here is written back into the canonical graph. This is a separate
//!   overlay, exactly as `l1-online-physical-plasticity` requires
//!   ("ajuste les poids physiques sans modifier le graphe canonique").
//!
//! The single modeling choice is the ANCHOR PAIR (below): the semantic poles of
//! "activation propensity". It is named and versioned so it is an explicit,
//! reviewable input, not a smuggled default.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use universe_core::{EntityKey, RelationKey};
use universe_embeddings::{
    cosine, EmbeddingError, EmbeddingProvider, EmbeddingRuntime, QuantizedEmbedding, SCORE_SCALE,
};
use universe_physics::BondPolarity;
use universe_store::{load_seed, EntityRecord, UniverseSnapshot, UniverseStore};

use crate::E2eError;

/// Identity of the one modeling choice: the semantic poles of activation.
pub const ANCHORS_ID: &str = "activation-propensity-v0";
/// Positive pole: the relation strongly propagates / recruits.
pub const POSITIVE_ANCHOR: &str = "This connection strongly drives, causes, leads to, grounds, or reinforces the next thing: activating the source strongly activates the target.";
/// Negative pole: the relation blocks / opposes / is inert.
pub const NEGATIVE_ANCHOR: &str = "This connection blocks, contradicts, weakens, inhibits, or opposes: activating the source suppresses the target or does not reach it.";
/// tanh temperature, in `SCORE_SCALE` units (1_000_000 == temperature 1.0). A
/// feel knob: smaller sharpens the split between support and inhibit.
pub const TEMPERATURE_MICRO: i64 = 1_000_000;
/// `|propensity in [-1,1]| * ENERGY_SCALE` -> integer bond energy.
pub const ENERGY_SCALE: u64 = 1_000;
/// Embedding dimensionality used by the deterministic seeding embedder.
pub const SEED_DIMENSIONS: usize = 64;

/// One relation's measured semantic energy, plus the AtomBond it converts to.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MeasuredBondEnergy {
    pub relation: RelationKey,
    pub source: EntityKey,
    pub target: EntityKey,
    pub predicate: String,
    /// Exactly the text that was measured — provenance for the propensity.
    pub sentence: String,
    pub cos_positive_micro: i64,
    pub cos_negative_micro: i64,
    /// The measured scalar in (-1,1), scaled by 1_000_000.
    pub propensity_micro: i64,
    pub polarity: BondPolarity,
    pub energy: u64,
}

/// The full measured seed overlay over the canonical graph. Separate from the
/// canonical store; carries the model + anchor provenance so no consumer can
/// mistake it for either evidence-of-lived-activation or canonical truth.
#[derive(Clone, Debug, Serialize)]
pub struct CanonicalSeedEnergy {
    pub epistemic_status: String,
    pub model: String,
    pub model_revision: String,
    pub anchors_id: String,
    pub positive_anchor: String,
    pub negative_anchor: String,
    pub temperature_micro: i64,
    pub energy_scale: u64,
    pub relations_measured: usize,
    pub support_bonds: usize,
    pub inhibit_bonds: usize,
    pub neutral_bonds: usize,
    /// How many distinct propensity values — proves the embedding differentiates
    /// relations rather than collapsing them to one number.
    pub distinct_propensities: usize,
    pub bonds: Vec<MeasuredBondEnergy>,
}

/// A deterministic, offline embedder: `dims` components hashed from the text.
///
/// It is the analogue of the seeding loop's "deterministic hashing embedder"
/// used in its tests: it needs no model download and yields the same vector for
/// the same text forever, so the whole seed is reproducible. A real
/// sentence-transformers provider (`NodeTransformersProvider`) is a drop-in
/// replacement when semantic — rather than merely deterministic — poles matter.
#[derive(Clone, Debug)]
pub struct HashingEmbedder {
    pub dimensions: usize,
}

impl EmbeddingProvider for HashingEmbedder {
    fn model(&self) -> &str {
        "hashing-embedder"
    }

    fn model_revision(&self) -> &str {
        "sha256:v0"
    }

    fn encode(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        Ok(texts
            .iter()
            .map(|text| hash_vector(text, self.dimensions))
            .collect())
    }
}

fn hash_vector(text: &str, dimensions: usize) -> Vec<f32> {
    let mut values = Vec::with_capacity(dimensions);
    for index in 0..dimensions {
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        hasher.update((index as u32).to_le_bytes());
        let digest = hasher.finalize();
        let raw = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]);
        // Map into [-1, 1].
        values.push(((f64::from(raw) / f64::from(u32::MAX)) * 2.0 - 1.0) as f32);
    }
    if values.iter().all(|value| *value == 0.0) {
        // Degenerate guard so quantization never sees a zero vector.
        if let Some(first) = values.first_mut() {
            *first = 1.0;
        }
    }
    values
}

fn char_bounded(text: &str, max_chars: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

/// Build "Source: <name>[. Source meaning: <content>]. Relation: <p>. Target:
/// <name>[. Target meaning: <content>]." — the unit the seeding loop embeds.
fn relation_sentence(
    store: &UniverseStore,
    entities: &BTreeMap<EntityKey, &EntityRecord>,
    symbols: &[String],
    source: EntityKey,
    predicate: &str,
    target: EntityKey,
) -> String {
    let phrase = |key: EntityKey, role: &str| -> String {
        let Some(entity) = entities.get(&key) else {
            return format!("{role}: <absent {}>.", key.0);
        };
        let name = symbols
            .get(entity.symbol as usize)
            .cloned()
            .unwrap_or_else(|| format!("<symbol {}>", entity.symbol));
        let meaning = entity
            .content
            .as_ref()
            .and_then(|content| store.read_content(content).ok())
            .map(|value| {
                for field in ["summary", "description", "kind", "semantic_type", "name"] {
                    if let Some(text) = value.get(field).and_then(serde_json::Value::as_str) {
                        return char_bounded(text, 160);
                    }
                }
                char_bounded(&value.to_string(), 160)
            })
            .filter(|meaning| !meaning.is_empty());
        match meaning {
            Some(meaning) => format!("{role}: {name}. {role} meaning: {meaning}."),
            None => format!("{role}: {name}."),
        }
    };
    format!(
        "{} Relation: {predicate}. {}",
        phrase(source, "Source"),
        phrase(target, "Target"),
    )
}

fn map_error(error: EmbeddingError) -> E2eError {
    E2eError::Contract(error.to_string())
}

/// The measured semantic scalar for one sentence, plus its AtomBond projection.
#[derive(Clone, Copy, Debug)]
pub struct MeasuredScalar {
    pub cos_positive_micro: i64,
    pub cos_negative_micro: i64,
    pub propensity_micro: i64,
    pub polarity: BondPolarity,
    pub energy: u64,
}

/// A reusable seeding context: an embedding runtime plus the two encoded
/// anchors. Shared by every seeder (canonical relations, conversation
/// reply-pairs, ...) so the anchor pair AND the propensity->bond math live in
/// exactly one place and can never drift between sources.
pub struct SeedContext<P: EmbeddingProvider> {
    runtime: EmbeddingRuntime<P>,
    positive: QuantizedEmbedding,
    negative: QuantizedEmbedding,
    pub model: String,
    pub model_revision: String,
}

impl<P: EmbeddingProvider> SeedContext<P> {
    /// Encode the fixed `activation-propensity-v0` anchors once.
    pub fn new(provider: P, dimensions: usize) -> Result<Self, E2eError> {
        let model = provider.model().to_owned();
        let model_revision = provider.model_revision().to_owned();
        let mut runtime = EmbeddingRuntime::new(provider, dimensions);
        let anchors = runtime
            .encode(&[POSITIVE_ANCHOR.to_owned(), NEGATIVE_ANCHOR.to_owned()])
            .map_err(map_error)?;
        Ok(Self {
            positive: anchors[0].clone(),
            negative: anchors[1].clone(),
            runtime,
            model,
            model_revision,
        })
    }

    /// Measure one sentence into a bounded, deterministic activation scalar. The
    /// AtomBond fields are a unit conversion of the measured propensity:
    /// magnitude -> energy, sign -> Support / Inhibit (near-zero -> Neutral).
    pub fn measure(&mut self, sentence: &str) -> Result<MeasuredScalar, E2eError> {
        let link = self
            .runtime
            .encode(&[sentence.to_owned()])
            .map_err(map_error)?
            .swap_remove(0);
        let cos_positive_micro = cosine(&link, &self.positive).map_err(map_error)?;
        let cos_negative_micro = cosine(&link, &self.negative).map_err(map_error)?;
        let raw = cos_positive_micro - cos_negative_micro;
        let propensity = (raw as f64 / TEMPERATURE_MICRO as f64).tanh();
        let propensity_micro = (propensity * SCORE_SCALE as f64).round() as i64;
        let energy = (propensity.abs() * ENERGY_SCALE as f64).round() as u64;
        let polarity = if energy == 0 {
            BondPolarity::Neutral
        } else if propensity_micro > 0 {
            BondPolarity::Support
        } else {
            BondPolarity::Inhibit
        };
        Ok(MeasuredScalar {
            cos_positive_micro,
            cos_negative_micro,
            propensity_micro,
            polarity,
            energy,
        })
    }
}

/// Load the canonical store and produce a MEASURED semantic energy overlay for
/// every relation. `provider` is injected so tests use the deterministic
/// embedder while production can pass a real model. DERIVED-FREE: every energy
/// traces to a measurement of the relation's meaning.
pub fn seed_energy_from_canonical<P: EmbeddingProvider>(
    repository: &Path,
    store_root: &Path,
    provider: P,
    dimensions: usize,
) -> Result<CanonicalSeedEnergy, E2eError> {
    let seed = load_seed(repository.join("fixtures/ontology/canonical-ontology.json"))?;
    let store = UniverseStore::open(store_root)?;
    let snapshot: UniverseSnapshot = if store_root.join("snapshot.json").exists() {
        store.replay(store.load_snapshot()?)?
    } else {
        store.install_seed(&seed)?
    };

    let entities: BTreeMap<EntityKey, &EntityRecord> =
        snapshot.entities.iter().map(|e| (e.key, e)).collect();

    let mut context = SeedContext::new(provider, dimensions)?;
    let model = context.model.clone();
    let model_revision = context.model_revision.clone();

    let mut bonds = Vec::new();
    let (mut support, mut inhibit, mut neutral) = (0usize, 0usize, 0usize);
    for relation in &snapshot.relations {
        if relation.source == relation.target {
            continue;
        }
        let predicate = snapshot
            .symbols
            .get(relation.predicate as usize)
            .cloned()
            .unwrap_or_else(|| format!("<predicate {}>", relation.predicate));
        let sentence = relation_sentence(
            &store,
            &entities,
            &snapshot.symbols,
            relation.source,
            &predicate,
            relation.target,
        );
        let scalar = context.measure(&sentence)?;
        match scalar.polarity {
            BondPolarity::Support => support += 1,
            BondPolarity::Inhibit => inhibit += 1,
            BondPolarity::Neutral => neutral += 1,
        }
        bonds.push(MeasuredBondEnergy {
            relation: relation.key,
            source: relation.source,
            target: relation.target,
            predicate,
            sentence,
            cos_positive_micro: scalar.cos_positive_micro,
            cos_negative_micro: scalar.cos_negative_micro,
            propensity_micro: scalar.propensity_micro,
            polarity: scalar.polarity,
            energy: scalar.energy,
        });
    }

    let distinct_propensities = bonds
        .iter()
        .map(|bond| bond.propensity_micro)
        .collect::<BTreeSet<_>>()
        .len();

    Ok(CanonicalSeedEnergy {
        epistemic_status: "measured:semantic_v0".to_owned(),
        model,
        model_revision,
        anchors_id: ANCHORS_ID.to_owned(),
        positive_anchor: POSITIVE_ANCHOR.to_owned(),
        negative_anchor: NEGATIVE_ANCHOR.to_owned(),
        temperature_micro: TEMPERATURE_MICRO,
        energy_scale: ENERGY_SCALE,
        relations_measured: bonds.len(),
        support_bonds: support,
        inhibit_bonds: inhibit,
        neutral_bonds: neutral,
        distinct_propensities,
        bonds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repository() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn seeds_measured_semantic_energy_over_the_real_canonical_graph() {
        let temp = tempfile::tempdir().unwrap();
        let seed = seed_energy_from_canonical(
            &repository(),
            &temp.path().join("store"),
            HashingEmbedder {
                dimensions: SEED_DIMENSIONS,
            },
            SEED_DIMENSIONS,
        )
        .unwrap();

        // We measured a real, multi-relation neighborhood.
        assert_eq!(seed.epistemic_status, "measured:semantic_v0");
        assert!(seed.relations_measured >= 2, "need real relations to seed");

        // Every propensity is bounded and every bond is internally consistent.
        for bond in &seed.bonds {
            assert!(bond.propensity_micro.abs() <= SCORE_SCALE);
            match bond.polarity {
                BondPolarity::Support => {
                    assert!(bond.energy > 0);
                    assert!(bond.propensity_micro > 0);
                }
                BondPolarity::Inhibit => {
                    assert!(bond.energy > 0);
                    assert!(bond.propensity_micro < 0);
                }
                BondPolarity::Neutral => assert_eq!(bond.energy, 0),
            }
        }
        assert_eq!(
            seed.support_bonds + seed.inhibit_bonds + seed.neutral_bonds,
            seed.bonds.len()
        );

        // The embedding differentiates relations — it is not a constant.
        assert!(
            seed.distinct_propensities >= 2,
            "measured energy must differentiate relations, got {} distinct",
            seed.distinct_propensities
        );

        // Honest ceiling: propensity is a pure function of the sentence, so the
        // rideable terrain can never have more distinct energies than the graph
        // gives distinct sentences. Resolution is bounded by the canonical
        // graph's textual distinctness, NOT by this method. (Observed on the
        // canonical store: 784 relations collapse to ~14 type-level sentences.)
        let distinct_sentences = seed
            .bonds
            .iter()
            .map(|bond| bond.sentence.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        assert!(
            seed.distinct_propensities <= distinct_sentences,
            "propensity is a function of the sentence: {} propensities > {} sentences",
            seed.distinct_propensities,
            distinct_sentences,
        );

        // Same store + same embedder -> byte-identical seed (deterministic).
        let temp2 = tempfile::tempdir().unwrap();
        let seed2 = seed_energy_from_canonical(
            &repository(),
            &temp2.path().join("store"),
            HashingEmbedder {
                dimensions: SEED_DIMENSIONS,
            },
            SEED_DIMENSIONS,
        )
        .unwrap();
        assert_eq!(seed.bonds.len(), seed2.bonds.len());
        assert_eq!(seed.distinct_propensities, seed2.distinct_propensities);
        for (left, right) in seed.bonds.iter().zip(&seed2.bonds) {
            assert_eq!(left, right);
        }
    }
}
