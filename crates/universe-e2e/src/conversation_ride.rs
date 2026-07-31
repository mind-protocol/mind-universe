//! The board rides a real CONVERSATION branch with measured-semantic energy.
//!
//! Where `canonical_ride`/`neighborhood_arc` ride the canonical type-ontology
//! (whose 784 relations collapse to ~14 sentences — flat terrain), this rides a
//! conversation branch: a parent message and its candidate replies. Each
//! reply-pair is measured through the SAME shared [`SeedContext`]
//! (`measured:semantic_v0`), so distinct replies get distinct energy — the
//! instance-level resolution the canonical registry lacks. Keys are synthetic
//! (conversation messages are not in the store); the ride is over AtomDynamics.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use universe_core::EntityKey;
use universe_embeddings::EmbeddingProvider;
use universe_physics::BondPolarity;

use crate::canonical_seed_energy::SeedContext;
use crate::magic_object::{
    Activation, Gesture, GradientPolicy, MagicObject, PartBond, PartNode, Role,
};
use crate::E2eError;

const HERE_KEY: u128 = 0x0001_0000;
const REPLY_BASE: u128 = 0x0001_0001;
const BOND_BASE: u128 = 0x0002_0000;
const SPACE_KEY: u128 = u128::MAX;
const CARVE_KEY: u128 = u128::MAX - 1;
const CARVE_BOND_KEY: u128 = u128::MAX - 2;

#[derive(Clone, Debug, Serialize)]
pub struct ConversationRide {
    pub epistemic_status: String,
    pub model: String,
    pub start: EntityKey,
    pub replies: usize,
    pub support_candidates: usize,
    pub inhibit_replies: usize,
    /// Distinct measured energies across all replies — the rideable resolution.
    pub distinct_energies: usize,
    pub attractor: Option<EntityKey>,
    pub attractor_energy: u64,
    pub carve_target: Option<EntityKey>,
    pub carve_attractor: Option<EntityKey>,
    pub carve_redirected: bool,
    pub energy_conserved: bool,
    pub quiescent: bool,
    pub resolution_note: String,
}

fn attractor_of(
    activation: &Activation,
    candidates: &BTreeSet<EntityKey>,
) -> Option<(EntityKey, u64)> {
    let fired: BTreeSet<EntityKey> = activation.fired.iter().copied().collect();
    candidates
        .iter()
        .filter(|candidate| fired.contains(candidate))
        .filter_map(|candidate| activation.support.get(candidate).map(|s| (*candidate, *s)))
        .max_by(|left, right| left.1.cmp(&right.1).then(right.0 .0.cmp(&left.0 .0)))
}

/// Measure a parent message's candidate replies and ride them as a descent: the
/// strongest-driving reply is the attractor, a contradicting reply roughens.
pub fn ride_conversation_branch<P: EmbeddingProvider>(
    parent: &str,
    replies: &[String],
    provider: P,
    dimensions: usize,
) -> Result<ConversationRide, E2eError> {
    let mut context = SeedContext::new(provider, dimensions)?;
    let model = context.model.clone();
    let sentences: Vec<String> = replies
        .iter()
        .map(|reply| format!("Message: {parent}. Reply: {reply}."))
        .collect();
    let scalars = context.measure_batch(&sentences)?;

    let here = EntityKey(HERE_KEY);
    let mut support_energy: BTreeMap<EntityKey, u64> = BTreeMap::new();
    let mut inhibited: BTreeSet<EntityKey> = BTreeSet::new();
    let mut target_keys: BTreeSet<EntityKey> = BTreeSet::new();
    let mut bonds: Vec<PartBond> = Vec::new();
    let mut start_pay = 0u64;
    let mut inhibit_replies = 0usize;
    let mut distinct = BTreeSet::new();
    for (index, scalar) in scalars.iter().enumerate() {
        distinct.insert(scalar.energy);
        let target = EntityKey(REPLY_BASE + index as u128);
        let bond_key = BOND_BASE + index as u128;
        match scalar.polarity {
            BondPolarity::Neutral => {}
            BondPolarity::Support => {
                bonds.push(PartBond {
                    key: bond_key,
                    source: here.0,
                    target: target.0,
                    polarity: BondPolarity::Support,
                    energy: scalar.energy,
                });
                target_keys.insert(target);
                *support_energy.entry(target).or_default() += scalar.energy;
                start_pay = start_pay.saturating_add(scalar.energy);
            }
            BondPolarity::Inhibit => {
                bonds.push(PartBond {
                    key: bond_key,
                    source: here.0,
                    target: target.0,
                    polarity: BondPolarity::Inhibit,
                    energy: scalar.energy,
                });
                target_keys.insert(target);
                inhibited.insert(target);
                start_pay = start_pay.saturating_add(scalar.energy);
                inhibit_replies += 1;
            }
        }
    }

    let support_set: BTreeSet<EntityKey> = support_energy.keys().copied().collect();

    let mut nodes = vec![PartNode {
        key: here.0,
        role: Role::Moment,
        function: "here".into(),
        binding: None,
        threshold: 1,
        seed_energy: start_pay,
        required_supports: Vec::new(),
        inhibition_threshold: None,
    }];
    for target in &target_keys {
        nodes.push(PartNode {
            key: target.0,
            role: Role::Moment,
            function: if support_energy.contains_key(target) {
                "candidate"
            } else {
                "blocked"
            }
            .into(),
            binding: None,
            threshold: 1,
            seed_energy: 0,
            required_supports: Vec::new(),
            inhibition_threshold: Some(1),
        });
    }

    let mut ranked: Vec<(EntityKey, u64)> = support_energy.iter().map(|(k, v)| (*k, *v)).collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then(left.0 .0.cmp(&right.0 .0)));
    let carve_target = ranked.get(1).map(|(key, _)| *key);
    let boost = ranked.first().map(|(_, energy)| *energy).unwrap_or(0).max(1);
    if let Some(target) = carve_target {
        nodes.push(PartNode {
            key: CARVE_KEY,
            role: Role::Thing,
            function: "carve_gate".into(),
            binding: Some("carve".into()),
            threshold: 1,
            seed_energy: 0,
            required_supports: Vec::new(),
            inhibition_threshold: None,
        });
        bonds.push(PartBond {
            key: CARVE_BOND_KEY,
            source: CARVE_KEY,
            target: target.0,
            polarity: BondPolarity::Support,
            energy: boost,
        });
    }

    let object = MagicObject::from_parts(
        "conversation-ride".into(),
        SPACE_KEY,
        Some(parent.chars().take(40).collect()),
        GradientPolicy::default(),
        nodes,
        bonds,
    )?;

    let default = object.wield(&[])?;
    let default_attractor = attractor_of(&default, &support_set);
    let carve = if carve_target.is_some() {
        object.wield(&[Gesture {
            binding: "carve",
            energy: boost,
        }])?
    } else {
        default.clone()
    };
    let carve_attractor = attractor_of(&carve, &support_set).map(|(key, _)| key);
    let carve_redirected = carve_target.is_some()
        && default_attractor.map(|(key, _)| key) != carve_attractor
        && carve_attractor == carve_target;

    Ok(ConversationRide {
        epistemic_status: "measured:semantic_v0".into(),
        model,
        start: here,
        replies: replies.len(),
        support_candidates: support_set.len(),
        inhibit_replies,
        distinct_energies: distinct.len(),
        attractor: default_attractor.map(|(key, _)| key),
        attractor_energy: default_attractor.map(|(_, energy)| energy).unwrap_or(0),
        carve_target,
        carve_attractor,
        carve_redirected,
        energy_conserved: default.energy_conserved,
        quiescent: default.quiescent,
        resolution_note:
            "instance-level: distinct replies get distinct measured energy (unlike the canonical registry's type-level collapse). Semantic fidelity needs a real embedder; hashing embedder is deterministic-but-not-semantic."
                .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical_seed_energy::{HashingEmbedder, SEED_DIMENSIONS};

    fn branch() -> (String, Vec<String>) {
        (
            "How should the board seed energy from real conversations?".into(),
            vec![
                "Measure each reply-pair's meaning and convert magnitude to energy.".into(),
                "That contradicts the plan; embeddings are too slow and should be dropped.".into(),
                "Recency alone drives the seed; the strongest continuation wins.".into(),
                "Block any unmeasured transfer at the membrane before it renders.".into(),
                "Grounding the profile in the graph keeps provenance measured.".into(),
            ],
        )
    }

    #[test]
    fn board_rides_a_measured_conversation_branch() {
        let (parent, replies) = branch();
        let ride = ride_conversation_branch(
            &parent,
            &replies,
            HashingEmbedder {
                dimensions: SEED_DIMENSIONS,
            },
            SEED_DIMENSIONS,
        )
        .unwrap();
        println!("{ride:#?}");

        assert_eq!(ride.epistemic_status, "measured:semantic_v0");
        assert_eq!(ride.replies, 5);
        // The point: distinct replies differentiate — a rich, non-collapsed terrain.
        assert!(
            ride.distinct_energies >= 3,
            "conversation replies must differentiate, got {} distinct",
            ride.distinct_energies
        );
        assert!(ride.energy_conserved && ride.quiescent);

        // Same input + embedder -> same ride.
        let (parent2, replies2) = branch();
        let ride2 = ride_conversation_branch(
            &parent2,
            &replies2,
            HashingEmbedder {
                dimensions: SEED_DIMENSIONS,
            },
            SEED_DIMENSIONS,
        )
        .unwrap();
        assert_eq!(ride.attractor, ride2.attractor);
        assert_eq!(ride.distinct_energies, ride2.distinct_energies);
    }
}
