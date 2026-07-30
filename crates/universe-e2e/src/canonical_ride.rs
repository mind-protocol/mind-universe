//! Heuristic, DERIVED ride over the REAL canonical ontology store, routed
//! through the [`crate::magic_object`] blueprint (one path for fixtures and real
//! data). It derives a bounded neighborhood into a decorated `space`: supporting
//! predicates become downhill bonds, oppositional predicates become inhibition
//! (roughness), and a carve gesture re-chooses the attractor.
//!
//! Every value here is **DERIVED and NOT MEASURED**. The canonical
//! `physical_profile`s are spatial and `prototype_not_calibrated`; the atom
//! energy is invented from `polarity[0]`. The epistemic tag rides on the result
//! so no consumer mistakes it for evidence.

use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use universe_core::EntityKey;
use universe_physics::BondPolarity;
use universe_store::{
    load_seed,
    ontology::{OntologyLoadBudget, OntologyRegistry},
    UniverseSnapshot, UniverseStore,
};

use crate::magic_object::{
    Activation, Gesture, GradientPolicy, MagicObject, PartBond, PartNode, Role,
};
use crate::E2eError;

/// Derived energy = round(|polarity[0]| * ENERGY_SCALE). Not a measurement.
const ENERGY_SCALE: f64 = 100.0;
/// Uniform derived firing threshold for every downhill candidate.
const CANDIDATE_THRESHOLD: u64 = 50;
/// Oppositional predicates deform into inhibition (roughness), not descent.
const OPPOSITIONAL: [&str; 7] = [
    "CONTRADICTS",
    "BLOCKS",
    "INHIBITS",
    "DECREASES_PROPENSITY",
    "WEAKENS",
    "MITIGATES",
    "PRESSURES",
];

// Synthetic keys for the object's own nodes, far from real entity keys.
const SPACE_KEY: u128 = u128::MAX;
const CARVE_KEY: u128 = u128::MAX - 1;
const CARVE_BOND_KEY: u128 = u128::MAX - 2;

#[derive(Clone, Debug, Serialize)]
pub struct DerivedRide {
    /// Iron tag: nothing in this record is measured.
    pub epistemic_status: String,
    pub object: String,
    pub start: EntityKey,
    pub start_symbol: String,
    pub support_candidates: usize,
    pub inhibit_bonds: usize,
    pub roughened_candidates: usize,
    pub unprofiled_skipped: usize,
    pub attractor: Option<EntityKey>,
    pub attractor_predicate: Option<String>,
    pub attractor_support: u64,
    pub carve_target: Option<EntityKey>,
    pub carve_attractor: Option<EntityKey>,
    pub carve_redirected: bool,
    pub energy_conserved: bool,
    pub quiescent: bool,
}

fn predicate_name(snapshot: &UniverseSnapshot, predicate: u32) -> Option<&str> {
    snapshot.symbols.get(predicate as usize).map(String::as_str)
}

fn derive_energy(profile: &serde_json::Value) -> Option<u64> {
    let forward = profile.get("polarity")?.as_array()?.first()?.as_f64()?;
    let energy = (forward.abs() * ENERGY_SCALE).round() as u64;
    (energy > 0).then_some(energy)
}

/// Count an entity's profiled outgoing relations (support or inhibit) so we can
/// pick the richest neighborhood to ride. Deterministic; ties go to lower key.
fn choose_start(snapshot: &UniverseSnapshot, registry: &OntologyRegistry) -> Option<EntityKey> {
    let mut best: Option<(usize, EntityKey)> = None;
    for entity in &snapshot.entities {
        let count = snapshot
            .relations
            .iter()
            .filter(|relation| relation.source == entity.key && relation.target != entity.key)
            .filter(|relation| {
                predicate_name(snapshot, relation.predicate).is_some_and(|name| {
                    OPPOSITIONAL.contains(&name)
                        || registry
                            .physical_profiles
                            .get(name)
                            .and_then(|profile| derive_energy(&profile.profile))
                            .is_some()
                })
            })
            .count();
        if count == 0 {
            continue;
        }
        best = match best {
            Some((best_count, best_key))
                if best_count > count || (best_count == count && best_key <= entity.key) =>
            {
                Some((best_count, best_key))
            }
            _ => Some((count, entity.key)),
        };
    }
    best.map(|(_, key)| key)
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

/// Load the canonical store, derive a rideable `space` from a real neighborhood,
/// wield it (default glide + carve), and read back the attractors. DERIVED.
pub fn derive_ride_from_canonical(
    repository: &Path,
    store_root: &Path,
) -> Result<DerivedRide, E2eError> {
    let seed = load_seed(repository.join("fixtures/ontology/canonical-ontology.json"))?;
    let store = UniverseStore::open(store_root)?;
    let snapshot = if store_root.join("snapshot.json").exists() {
        store.replay(store.load_snapshot()?)?
    } else {
        store.install_seed(&seed)?
    };
    let registry = OntologyRegistry::load(&store, &snapshot, OntologyLoadBudget::default())?;

    let start = choose_start(&snapshot, &registry).ok_or_else(|| {
        E2eError::Contract("no canonical entity has a profiled outgoing relation".into())
    })?;

    let mut support_energy: BTreeMap<EntityKey, u64> = BTreeMap::new();
    let mut support_predicate: BTreeMap<EntityKey, String> = BTreeMap::new();
    let mut inhibited: BTreeSet<EntityKey> = BTreeSet::new();
    let mut target_keys: BTreeSet<EntityKey> = BTreeSet::new();
    let mut bonds: Vec<PartBond> = Vec::new();
    let mut start_pay = 0u64;
    let mut inhibit_bonds = 0usize;
    let mut unprofiled_skipped = 0usize;
    for relation in &snapshot.relations {
        if relation.source != start || relation.target == start {
            continue;
        }
        let Some(name) = predicate_name(&snapshot, relation.predicate) else {
            continue;
        };
        let profiled = registry
            .physical_profiles
            .get(name)
            .and_then(|profile| derive_energy(&profile.profile));
        if OPPOSITIONAL.contains(&name) {
            let energy = profiled.unwrap_or(1);
            bonds.push(PartBond {
                key: relation.key.0,
                source: start.0,
                target: relation.target.0,
                polarity: BondPolarity::Inhibit,
                energy,
            });
            target_keys.insert(relation.target);
            inhibited.insert(relation.target);
            start_pay = start_pay.saturating_add(energy);
            inhibit_bonds += 1;
        } else if let Some(energy) = profiled {
            bonds.push(PartBond {
                key: relation.key.0,
                source: start.0,
                target: relation.target.0,
                polarity: BondPolarity::Support,
                energy,
            });
            target_keys.insert(relation.target);
            *support_energy.entry(relation.target).or_default() += energy;
            support_predicate
                .entry(relation.target)
                .or_insert_with(|| name.to_owned());
            start_pay = start_pay.saturating_add(energy);
        } else {
            unprofiled_skipped += 1;
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
        let is_support = support_energy.contains_key(target);
        nodes.push(PartNode {
            key: target.0,
            role: Role::Moment,
            function: if is_support { "candidate" } else { "blocked" }.into(),
            binding: None,
            threshold: CANDIDATE_THRESHOLD,
            seed_energy: 0,
            required_supports: Vec::new(),
            inhibition_threshold: Some(1),
        });
    }

    // Carving: boost the second-strongest downhill candidate enough to overtake
    // the first. It re-chooses "down" without touching the object's structure.
    let mut ranked: Vec<(EntityKey, u64)> = support_energy.iter().map(|(k, v)| (*k, *v)).collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then(left.0 .0.cmp(&right.0 .0)));
    let carve_target = ranked.get(1).map(|(key, _)| *key);
    let boost = ranked.first().map(|(_, energy)| *energy).unwrap_or(0);
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

    let start_symbol = snapshot
        .entities
        .iter()
        .find(|entity| entity.key == start)
        .and_then(|entity| snapshot.symbols.get(entity.symbol as usize))
        .cloned()
        .unwrap_or_default();

    let policy = GradientPolicy {
        energizes: ["LEADS_TO", "CAUSES", "DEFINES", "GROUNDS"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        inhibits: OPPOSITIONAL.iter().map(|s| s.to_string()).collect(),
        fork_predicate: Some("QUESTIONS".into()),
        answer_predicate: Some("ANSWERS".into()),
    };
    let object = MagicObject::from_parts(
        format!("canonical-ride-{}", start.0),
        SPACE_KEY,
        Some(start_symbol.clone()),
        policy,
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

    Ok(DerivedRide {
        epistemic_status: "derived_uncalibrated / not_measured".into(),
        object: object.name,
        start,
        start_symbol,
        support_candidates: support_set.len(),
        inhibit_bonds,
        roughened_candidates,
        unprofiled_skipped,
        attractor: default_attractor.map(|(key, _)| key),
        attractor_predicate: default_attractor
            .and_then(|(key, _)| support_predicate.get(&key).cloned()),
        attractor_support: default_attractor.map(|(_, support)| support).unwrap_or(0),
        carve_target,
        carve_attractor,
        carve_redirected,
        energy_conserved: default.energy_conserved,
        quiescent: default.quiescent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn board_rides_a_real_canonical_neighborhood_through_the_blueprint() {
        let temp = tempfile::tempdir().unwrap();
        let ride = derive_ride_from_canonical(&repository(), &temp.path().join("store")).unwrap();
        println!("{ride:#?}");

        assert!(ride.support_candidates >= 2, "need a real choice to carve");
        assert!(ride.attractor.is_some());
        assert!(ride.energy_conserved && ride.quiescent);
        assert_eq!(ride.epistemic_status, "derived_uncalibrated / not_measured");

        // Carving re-chooses the attractor on real data.
        assert!(ride.carve_redirected);
        assert_ne!(ride.attractor, ride.carve_attractor);

        // Same store, same derivation -> same descent.
        let temp2 = tempfile::tempdir().unwrap();
        let ride2 = derive_ride_from_canonical(&repository(), &temp2.path().join("store")).unwrap();
        assert_eq!(ride.start, ride2.start);
        assert_eq!(ride.attractor, ride2.attractor);
        assert_eq!(ride.carve_attractor, ride2.carve_attractor);
    }
}
