//! Ride over the REAL canonical ontology store, routed through the
//! [`crate::magic_object`] blueprint (one path for fixtures and real data).
//!
//! Two provenances for the atom energy, both honest about what they are:
//!
//! - **derived** ([`derive_ride_from_canonical`]): energy = round(|polarity[0]|
//!   * 100), a Rust formula over the spatial `physical_profile`. Tagged
//!   `derived_uncalibrated / not_measured`. This can never be streamed as a felt
//!   glide — `universe-protocol` rejects non-`Measured` energy transfers.
//! - **measured-authored** ([`derive_ride_from_canonical_measured`]): energy is
//!   authored as integers in a graph overlay, committed, and read back with its
//!   content hash. Tagged `measured_authored / uncalibrated`: the PROVENANCE is
//!   now measured (graph authority + verified hash — what the membrane requires),
//!   while the MAGNITUDE is still authored, not fitted-to-data. True calibration
//!   is blocked until an observable exists.
//!
//! In both, support-vs-inhibit is the SIGN of `polarity[0]` — a read (measured)
//! graph property, never a hardcoded name list.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use universe_core::{EntityKey, Tick};
use universe_physics::BondPolarity;
use universe_store::{
    load_seed,
    ontology::{OntologyLoadBudget, OntologyRegistry},
    EntityRecord, UniverseSnapshot, UniverseStore,
};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

use crate::magic_object::{
    Activation, Gesture, GradientPolicy, MagicObject, PartBond, PartNode, Role,
};
use crate::E2eError;

/// Derived energy = round(|polarity[0]| * ENERGY_SCALE). Not a measurement.
const ENERGY_SCALE: f64 = 100.0;
/// Uniform derived firing threshold for every downhill candidate.
const CANDIDATE_THRESHOLD: u64 = 50;

// Synthetic keys for the object's own nodes, far from real entity keys.
const SPACE_KEY: u128 = u128::MAX;
const CARVE_KEY: u128 = u128::MAX - 1;
const CARVE_BOND_KEY: u128 = u128::MAX - 2;

#[derive(Clone, Debug, Serialize)]
pub struct DerivedRide {
    /// Provenance/calibration tag for the energy magnitudes.
    pub epistemic_status: String,
    /// predicate -> content hash of its authored energy profile (measured mode
    /// only; empty when energy is derived in code).
    pub energy_provenance: BTreeMap<String, String>,
    /// Measured, not derived: "roughened" iff a support candidate is also
    /// inhibited; otherwise "known_absent: ...". A zero here is measured absence.
    pub roughness_status: String,
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

#[derive(Debug, Deserialize)]
struct EnergyOverlay {
    overlay: String,
    profile_id: String,
    status: String,
    profiles: Vec<AuthoredEnergy>,
}

#[derive(Debug, Deserialize)]
struct AuthoredEnergy {
    predicate: String,
    transfer_energy: u64,
}

fn predicate_name(snapshot: &UniverseSnapshot, predicate: u32) -> Option<&str> {
    snapshot.symbols.get(predicate as usize).map(String::as_str)
}

fn forward_polarity(registry: &OntologyRegistry, name: &str) -> Option<f64> {
    registry
        .physical_profiles
        .get(name)?
        .profile
        .get("polarity")?
        .as_array()?
        .first()?
        .as_f64()
}

/// Support-vs-inhibit is the SIGN of forward polarity — a read graph property.
fn classify_sign(registry: &OntologyRegistry, name: &str) -> Option<BondPolarity> {
    match forward_polarity(registry, name)? {
        forward if forward < 0.0 => Some(BondPolarity::Inhibit),
        forward if forward > 0.0 => Some(BondPolarity::Support),
        _ => None,
    }
}

/// Derive a bond (polarity from sign, magnitude from |polarity|*100). The
/// magnitude is derived-in-code and NOT measured.
fn derive_bond(registry: &OntologyRegistry, name: &str) -> Option<(BondPolarity, u64)> {
    let forward = forward_polarity(registry, name)?;
    let energy = (forward.abs() * ENERGY_SCALE).round() as u64;
    if energy == 0 {
        return None;
    }
    let polarity = if forward < 0.0 {
        BondPolarity::Inhibit
    } else {
        BondPolarity::Support
    };
    Some((polarity, energy))
}

fn is_support(registry: &OntologyRegistry, name: &str) -> bool {
    matches!(classify_sign(registry, name), Some(BondPolarity::Support))
}

fn is_inhibitory(registry: &OntologyRegistry, name: &str) -> bool {
    matches!(classify_sign(registry, name), Some(BondPolarity::Inhibit))
}

fn load_canonical(
    repository: &Path,
    store_root: &Path,
) -> Result<(UniverseSnapshot, OntologyRegistry), E2eError> {
    let seed = load_seed(repository.join("fixtures/ontology/canonical-ontology.json"))?;
    let store = UniverseStore::open(store_root)?;
    let snapshot = if store_root.join("snapshot.json").exists() {
        store.replay(store.load_snapshot()?)?
    } else {
        store.install_seed(&seed)?
    };
    let registry = OntologyRegistry::load(&store, &snapshot, OntologyLoadBudget::default())?;
    Ok((snapshot, registry))
}

fn next_key(snapshot: &UniverseSnapshot) -> u128 {
    snapshot
        .entities
        .iter()
        .map(|entity| entity.key.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

/// Pick the entity with the most profiled outgoing relations — the richest
/// neighborhood to ride. Deterministic; ties go to lower key.
fn choose_start(snapshot: &UniverseSnapshot, registry: &OntologyRegistry) -> Option<EntityKey> {
    let mut best: Option<(usize, EntityKey)> = None;
    for entity in &snapshot.entities {
        let count = snapshot
            .relations
            .iter()
            .filter(|relation| relation.source == entity.key && relation.target != entity.key)
            .filter(|relation| {
                predicate_name(snapshot, relation.predicate)
                    .is_some_and(|name| classify_sign(registry, name).is_some())
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

/// Pick the most conflictual neighborhood (support and inhibition into the same
/// targets). Returns None when no such tension exists — the honest answer here.
fn choose_conflict_start(
    snapshot: &UniverseSnapshot,
    registry: &OntologyRegistry,
) -> Option<EntityKey> {
    let mut best: Option<(usize, usize, EntityKey)> = None;
    for entity in &snapshot.entities {
        let mut supported = BTreeSet::new();
        let mut inhibited = BTreeSet::new();
        for relation in &snapshot.relations {
            if relation.source != entity.key || relation.target == entity.key {
                continue;
            }
            let Some(name) = predicate_name(snapshot, relation.predicate) else {
                continue;
            };
            if is_support(registry, name) {
                supported.insert(relation.target);
            } else if is_inhibitory(registry, name) {
                inhibited.insert(relation.target);
            }
        }
        let roughened = supported.intersection(&inhibited).count();
        if roughened == 0 {
            continue;
        }
        let fan_out = supported.len();
        best = match best {
            Some((best_rough, best_fan, best_key))
                if (best_rough, best_fan) > (roughened, fan_out)
                    || ((best_rough, best_fan) == (roughened, fan_out)
                        && best_key <= entity.key) =>
            {
                Some((best_rough, best_fan, best_key))
            }
            _ => Some((roughened, fan_out, entity.key)),
        };
    }
    best.map(|(_, _, key)| key)
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

/// Derive a rideable `space` from the neighborhood of `start`, sourcing each
/// bond's energy from `energy_for`, and read back the glide + carve + roughness.
fn build_ride(
    snapshot: &UniverseSnapshot,
    start: EntityKey,
    energy_for: &dyn Fn(&str) -> Option<(BondPolarity, u64)>,
    epistemic_status: String,
    energy_provenance: BTreeMap<String, String>,
) -> Result<DerivedRide, E2eError> {
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
        let Some(name) = predicate_name(snapshot, relation.predicate) else {
            continue;
        };
        match energy_for(name) {
            Some((BondPolarity::Inhibit, energy)) => {
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
            }
            Some((_, energy)) => {
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
            }
            None => unprofiled_skipped += 1,
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
            function: if is_support_target {
                "candidate"
            } else {
                "blocked"
            }
            .into(),
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
        inhibits: ["CONTRADICTS", "BLOCKS", "INHIBITS"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
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

    let roughness_status = if roughened_candidates > 0 {
        "roughened".to_owned()
    } else {
        "known_absent: no oppositional relation in neighborhood".to_owned()
    };

    Ok(DerivedRide {
        epistemic_status,
        energy_provenance,
        roughness_status,
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

/// Ride the richest profiled neighborhood with DERIVED (in-code) energy.
pub fn derive_ride_from_canonical(
    repository: &Path,
    store_root: &Path,
) -> Result<DerivedRide, E2eError> {
    let (snapshot, registry) = load_canonical(repository, store_root)?;
    let start = choose_start(&snapshot, &registry).ok_or_else(|| {
        E2eError::Contract("no canonical entity has a profiled outgoing relation".into())
    })?;
    build_ride(
        &snapshot,
        start,
        &|name| derive_bond(&registry, name),
        "derived_uncalibrated / not_measured".into(),
        BTreeMap::new(),
    )
}

/// Ride the neighborhood of a caller-chosen start with DERIVED energy.
pub fn derive_ride_from_canonical_at(
    repository: &Path,
    store_root: &Path,
    start: EntityKey,
) -> Result<DerivedRide, E2eError> {
    let (snapshot, registry) = load_canonical(repository, store_root)?;
    build_ride(
        &snapshot,
        start,
        &|name| derive_bond(&registry, name),
        "derived_uncalibrated / not_measured".into(),
        BTreeMap::new(),
    )
}

/// Ride the most conflictual neighborhood, or `Ok(None)` when the graph has no
/// oppositional tension — a measured absence, never a fabricated ride.
pub fn derive_ride_maximizing_conflict(
    repository: &Path,
    store_root: &Path,
) -> Result<Option<DerivedRide>, E2eError> {
    let (snapshot, registry) = load_canonical(repository, store_root)?;
    match choose_conflict_start(&snapshot, &registry) {
        Some(start) => build_ride(
            &snapshot,
            start,
            &|name| derive_bond(&registry, name),
            "derived_uncalibrated / not_measured".into(),
            BTreeMap::new(),
        )
        .map(Some),
        None => Ok(None),
    }
}

/// Ride with MEASURED-PROVENANCE energy: author integer energies into a graph
/// overlay, commit them, read them back with content hashes, and source each
/// bond from the graph — not from an in-code formula. Magnitudes are authored,
/// not fitted (uncalibrated); their provenance is measured.
pub fn derive_ride_from_canonical_measured(
    repository: &Path,
    store_root: &Path,
) -> Result<DerivedRide, E2eError> {
    let seed = load_seed(repository.join("fixtures/ontology/canonical-ontology.json"))?;
    let store = UniverseStore::open(store_root)?;
    let mut snapshot = if store_root.join("snapshot.json").exists() {
        store.replay(store.load_snapshot()?)?
    } else {
        store.install_seed(&seed)?
    };

    let overlay: EnergyOverlay = serde_json::from_slice(
        &fs::read(repository.join("fixtures/ontology/canonical-energy-overlay-v0.json"))
            .map_err(|error| E2eError::Io(error.to_string()))?,
    )
    .map_err(|error| E2eError::Contract(error.to_string()))?;

    // Author + commit the energy overlay once (idempotent by event key).
    let idempotency_key = format!("{}:authored", overlay.overlay);
    if !snapshot.event_keys.contains(&idempotency_key) {
        let plan = snapshot.plan_symbol_interning(&["authored_energy_profile".to_owned()])?;
        let symbol = *plan
            .assignments
            .get("authored_energy_profile")
            .ok_or_else(|| E2eError::Contract("energy profile symbol was not planned".into()))?;
        let mut commands = Vec::new();
        if !plan.additions.is_empty() {
            commands.push(UniverseCommand::InternSymbols {
                symbols: plan.additions.clone(),
            });
        }
        let mut next = next_key(&snapshot);
        for profile in &overlay.profiles {
            let content = store.append_content(&json!({
                "kind": "authored_energy_profile",
                "predicate": profile.predicate,
                "transfer_energy": profile.transfer_energy,
                "profile_id": overlay.profile_id,
                "status": overlay.status,
            }))?;
            commands.push(UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key: EntityKey(next),
                    generation: 0,
                    symbol,
                    content: Some(content),
                },
            });
            next += 1;
        }
        let transaction = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: snapshot.revision,
                idempotency_key,
                causal_ancestry: vec![overlay.overlay.clone()],
                commands,
            },
        )?;
        let commit_tick = Tick(snapshot.tick.0 + 1);
        transaction.commit(&store, &mut snapshot, commit_tick)?;
    }

    // Independent readback: hydrate the authored energies + record their content
    // hashes. This is what makes the provenance MEASURED.
    let readback = store.replay(store.load_snapshot()?)?;
    let registry = OntologyRegistry::load(&store, &readback, OntologyLoadBudget::default())?;
    let symbol = readback
        .symbol_id("authored_energy_profile")
        .ok_or_else(|| {
            E2eError::Contract("authored_energy_profile symbol missing after commit".into())
        })?;
    let mut authored: BTreeMap<String, (u64, String)> = BTreeMap::new();
    for entity in readback
        .entities
        .iter()
        .filter(|entity| entity.symbol == symbol)
    {
        let content_ref = entity
            .content
            .as_ref()
            .ok_or_else(|| E2eError::Contract("authored energy profile has no content".into()))?;
        let value = store.read_content(content_ref)?;
        let predicate = value["predicate"]
            .as_str()
            .ok_or_else(|| E2eError::Contract("authored energy profile has no predicate".into()))?
            .to_owned();
        let energy = value["transfer_energy"].as_u64().ok_or_else(|| {
            E2eError::Contract("authored energy profile has no transfer_energy".into())
        })?;
        authored.insert(predicate, (energy, content_ref.sha256.clone()));
    }
    if authored.is_empty() {
        return Err(E2eError::Contract(
            "no authored energy profile was read back".into(),
        ));
    }

    let start = choose_start(&readback, &registry).ok_or_else(|| {
        E2eError::Contract("no canonical entity has a profiled outgoing relation".into())
    })?;
    let provenance: BTreeMap<String, String> = authored
        .iter()
        .map(|(predicate, (_, hash))| (predicate.clone(), hash.clone()))
        .collect();
    build_ride(
        &readback,
        start,
        &|name| {
            classify_sign(&registry, name)
                .and_then(|pol| authored.get(name).map(|(e, _)| (pol, *e)))
        },
        "measured_authored / uncalibrated".into(),
        provenance,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn is_content_hash(hash: &str) -> bool {
        hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
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
        assert!(ride.energy_provenance.is_empty());

        assert!(ride.carve_redirected);
        assert_ne!(ride.attractor, ride.carve_attractor);

        let temp2 = tempfile::tempdir().unwrap();
        let ride2 = derive_ride_from_canonical(&repository(), &temp2.path().join("store")).unwrap();
        assert_eq!(ride.start, ride2.start);
        assert_eq!(ride.attractor, ride2.attractor);
        assert_eq!(ride.carve_attractor, ride2.carve_attractor);
    }

    #[test]
    fn conflict_absence_is_measured_not_assumed() {
        let temp = tempfile::tempdir().unwrap();
        let conflict =
            derive_ride_maximizing_conflict(&repository(), &temp.path().join("store")).unwrap();
        assert!(conflict.is_none(), "no oppositional tension exists to ride");

        let temp2 = tempfile::tempdir().unwrap();
        let ride = derive_ride_from_canonical(&repository(), &temp2.path().join("store")).unwrap();
        assert_eq!(ride.inhibit_bonds, 0);
        assert_eq!(ride.roughened_candidates, 0);
        assert!(ride.roughness_status.starts_with("known_absent"));
    }

    #[test]
    fn roughness_bites_when_opposition_is_present() {
        // SYNTHETIC, not canonical: a hand-built neighborhood where one target
        // gets both support and inhibition. Proves the bite on AtomDynamics
        // without pretending the canonical corpus contains opposition.
        let nodes = vec![
            PartNode {
                key: 1,
                role: Role::Moment,
                function: "here".into(),
                binding: None,
                threshold: 1,
                seed_energy: 210,
                required_supports: Vec::new(),
                inhibition_threshold: None,
            },
            PartNode {
                key: 2,
                role: Role::Moment,
                function: "candidate".into(),
                binding: None,
                threshold: CANDIDATE_THRESHOLD,
                seed_energy: 0,
                required_supports: Vec::new(),
                inhibition_threshold: Some(1),
            },
            PartNode {
                key: 3,
                role: Role::Moment,
                function: "candidate".into(),
                binding: None,
                threshold: CANDIDATE_THRESHOLD,
                seed_energy: 0,
                required_supports: Vec::new(),
                inhibition_threshold: Some(1),
            },
        ];
        let bonds = vec![
            PartBond {
                key: 10,
                source: 1,
                target: 2,
                polarity: BondPolarity::Support,
                energy: 100,
            },
            PartBond {
                key: 11,
                source: 1,
                target: 3,
                polarity: BondPolarity::Support,
                energy: 100,
            },
            PartBond {
                key: 12,
                source: 1,
                target: 2,
                polarity: BondPolarity::Inhibit,
                energy: 10,
            },
        ];
        let object = MagicObject::from_parts(
            "synthetic-roughness".into(),
            SPACE_KEY,
            None,
            GradientPolicy::default(),
            nodes,
            bonds,
        )
        .unwrap();
        let activation = object.wield(&[]).unwrap();
        let fired: BTreeSet<EntityKey> = activation.fired.iter().copied().collect();

        assert!(fired.contains(&EntityKey(3)), "un-inhibited sibling fires");
        assert!(
            !fired.contains(&EntityKey(2)),
            "roughened candidate is blocked by inhibition, not starvation"
        );
        assert!(activation.energy_conserved && activation.quiescent);
    }

    #[test]
    fn measured_provenance_energy_reads_back_from_the_graph() {
        let temp = tempfile::tempdir().unwrap();
        let ride =
            derive_ride_from_canonical_measured(&repository(), &temp.path().join("store")).unwrap();
        println!("{ride:#?}");

        // The energy provenance is now measured: every magnitude is read back
        // from a committed graph node with a verified content hash.
        assert_eq!(ride.epistemic_status, "measured_authored / uncalibrated");
        assert!(!ride.energy_provenance.is_empty());
        assert!(ride
            .energy_provenance
            .values()
            .all(|hash| is_content_hash(hash)));
        assert!(ride.attractor.is_some());
        assert!(ride.energy_conserved && ride.quiescent);

        // Only provenance changed: authored magnitudes equal the derived ones, so
        // the same neighborhood yields the same attractor.
        let temp2 = tempfile::tempdir().unwrap();
        let derived =
            derive_ride_from_canonical(&repository(), &temp2.path().join("store")).unwrap();
        assert_eq!(ride.start, derived.start);
        assert_eq!(ride.attractor, derived.attractor);
        assert_eq!(ride.support_candidates, derived.support_candidates);
    }
}
