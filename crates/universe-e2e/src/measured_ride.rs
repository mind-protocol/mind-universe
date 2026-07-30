//! The board rides a real canonical NEIGHBORHOOD with MEASURED-SEMANTIC energy.
//!
//! It connects [`crate::canonical_seed_energy`] (which measures each relation's
//! meaning via embeddings against a named anchor pair, tagged
//! `measured:semantic_v0`) to the [`crate::magic_object`] blueprint: the
//! neighborhood of the busiest node becomes a decorated `space` the board carves.
//!
//! Provenance ladder this sits on:
//! - `canonical_ride` derived: energy = a formula over a spatial field. not_measured.
//! - `canonical_ride` measured-authored: energy authored in graph + hash. measured provenance.
//! - `behavior_ride` executed: ONE bond compiled + executed. measured + executed.
//! - **here**: energy = a real measurement of each relation's MEANING, over a
//!   whole neighborhood. Rides `AtomDynamics` (not an executed BehaviorBond) —
//!   riding N EXECUTED bonds is the remaining fidelity arc.

use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use universe_core::EntityKey;
use universe_physics::BondPolarity;

use crate::canonical_seed_energy::{seed_energy_from_canonical, HashingEmbedder, SEED_DIMENSIONS};
use crate::magic_object::{
    Activation, Gesture, GradientPolicy, MagicObject, PartBond, PartNode, Role,
};
use crate::E2eError;

const SPACE_KEY: u128 = u128::MAX;
const CARVE_KEY: u128 = u128::MAX - 1;
const CARVE_BOND_KEY: u128 = u128::MAX - 2;
/// Measured energies live in [0, ENERGY_SCALE]; any supported candidate fires,
/// and the attractor is the steepest (highest measured energy).
const CANDIDATE_THRESHOLD: u64 = 1;

#[derive(Clone, Debug, Serialize)]
pub struct MeasuredRide {
    pub epistemic_status: String,
    pub model: String,
    pub anchors_id: String,
    pub start: EntityKey,
    pub support_candidates: usize,
    pub inhibit_bonds: usize,
    pub roughened_candidates: usize,
    pub attractor: Option<EntityKey>,
    pub attractor_predicate: Option<String>,
    pub attractor_energy: u64,
    pub carve_target: Option<EntityKey>,
    pub carve_attractor: Option<EntityKey>,
    pub carve_redirected: bool,
    pub energy_conserved: bool,
    pub quiescent: bool,
    /// Honest ceiling on the current corpus.
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

/// Measure the canonical graph's semantic energy, then ride the busiest node's
/// neighborhood through the blueprint. The energy is `measured:semantic_v0`.
pub fn ride_measured_semantic(
    repository: &Path,
    store_root: &Path,
) -> Result<MeasuredRide, E2eError> {
    let seed = seed_energy_from_canonical(
        repository,
        store_root,
        HashingEmbedder {
            dimensions: SEED_DIMENSIONS,
        },
        SEED_DIMENSIONS,
    )?;

    // Ride the busiest source (most non-neutral measured bonds). Deterministic.
    let mut counts: BTreeMap<EntityKey, usize> = BTreeMap::new();
    for bond in &seed.bonds {
        if bond.polarity != BondPolarity::Neutral {
            *counts.entry(bond.source).or_default() += 1;
        }
    }
    let start = counts
        .iter()
        .max_by(|left, right| left.1.cmp(right.1).then(right.0 .0.cmp(&left.0 .0)))
        .map(|(key, _)| *key)
        .ok_or_else(|| E2eError::Contract("no non-neutral measured bond to ride".into()))?;

    let mut support_energy: BTreeMap<EntityKey, u64> = BTreeMap::new();
    let mut support_predicate: BTreeMap<EntityKey, String> = BTreeMap::new();
    let mut inhibited: BTreeSet<EntityKey> = BTreeSet::new();
    let mut target_keys: BTreeSet<EntityKey> = BTreeSet::new();
    let mut bonds: Vec<PartBond> = Vec::new();
    let mut start_pay = 0u64;
    let mut inhibit_bonds = 0usize;
    for bond in seed
        .bonds
        .iter()
        .filter(|bond| bond.source == start && bond.target != start)
    {
        match bond.polarity {
            BondPolarity::Neutral => continue,
            BondPolarity::Inhibit => {
                bonds.push(PartBond {
                    key: bond.relation.0,
                    source: start.0,
                    target: bond.target.0,
                    polarity: BondPolarity::Inhibit,
                    energy: bond.energy,
                });
                target_keys.insert(bond.target);
                inhibited.insert(bond.target);
                start_pay = start_pay.saturating_add(bond.energy);
                inhibit_bonds += 1;
            }
            BondPolarity::Support => {
                bonds.push(PartBond {
                    key: bond.relation.0,
                    source: start.0,
                    target: bond.target.0,
                    polarity: BondPolarity::Support,
                    energy: bond.energy,
                });
                target_keys.insert(bond.target);
                *support_energy.entry(bond.target).or_default() += bond.energy;
                support_predicate
                    .entry(bond.target)
                    .or_insert_with(|| bond.predicate.clone());
                start_pay = start_pay.saturating_add(bond.energy);
            }
        }
    }

    let support_set: BTreeSet<EntityKey> = support_energy.keys().copied().collect();
    let roughened_candidates = support_set.intersection(&inhibited).count();

    let mut nodes = vec![PartNode {
        key: start.0,
        role: Role::Moment,
        function: "here".into(),
        binding: None,
        threshold: 1,
        seed_energy: start_pay,
        required_supports: Vec::new(),
        inhibition_threshold: None,
    }];
    for target in &target_keys {
        let is_support_target = support_energy.contains_key(target);
        nodes.push(PartNode {
            key: target.0,
            role: Role::Moment,
            function: if is_support_target { "candidate" } else { "blocked" }.into(),
            binding: None,
            threshold: CANDIDATE_THRESHOLD,
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
        format!("measured-ride-{}", start.0),
        SPACE_KEY,
        None,
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

    Ok(MeasuredRide {
        epistemic_status: seed.epistemic_status,
        model: seed.model,
        anchors_id: seed.anchors_id,
        start,
        support_candidates: support_set.len(),
        inhibit_bonds,
        roughened_candidates,
        attractor: default_attractor.map(|(key, _)| key),
        attractor_predicate: default_attractor
            .and_then(|(key, _)| support_predicate.get(&key).cloned()),
        attractor_energy: default_attractor.map(|(_, energy)| energy).unwrap_or(0),
        carve_target,
        carve_attractor,
        carve_redirected,
        energy_conserved: default.energy_conserved,
        quiescent: default.quiescent,
        resolution_note:
            "type-level on the canonical registry (measured energy collapses to per-predicate sentence); an instance corpus would differentiate richly"
                .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn board_rides_a_measured_semantic_neighborhood() {
        let temp = tempfile::tempdir().unwrap();
        let ride = ride_measured_semantic(&repository(), &temp.path().join("store")).unwrap();
        println!("{ride:#?}");

        assert_eq!(ride.epistemic_status, "measured:semantic_v0");
        assert!(ride.support_candidates >= 1, "need a real neighborhood to ride");
        assert!(ride.attractor.is_some());
        assert!(ride.energy_conserved && ride.quiescent);

        // Same store + same embedder -> same ride.
        let temp2 = tempfile::tempdir().unwrap();
        let ride2 = ride_measured_semantic(&repository(), &temp2.path().join("store")).unwrap();
        assert_eq!(ride.start, ride2.start);
        assert_eq!(ride.attractor, ride2.attractor);
        assert_eq!(ride.support_candidates, ride2.support_candidates);
    }
}
