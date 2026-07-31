//! A ToolkitResolver: run a construct FROM ITS GRAPH PROJECTION.
//!
//! `house_alarm_fire` proves Rung 1 (approach -> the construct wakes -> it
//! notifies) but assembles the alarm's atom circuit BY HAND as hardcoded
//! constants. This module closes that gap: it reads the AUTHORED atom circuit
//! from a construct's graph projection (the `alarm_atom_circuit` block of
//! `fixtures/ontology/lumina-prime-house-alarm-v0.json`) and materializes the
//! exact runtime inputs `Supervisor::run_physics_deposit_phase` consumes — so
//! the SAME Rung-1 outcome is produced from the authored circuit, not from
//! hand-built constants.
//!
//! It is pure runtime materialization: it reads authored data and allocates
//! runtime structs. It commits NOTHING, mutates no store, and mints zero new
//! canonical symbols. The physics-firing semantics live entirely in the real
//! `universe_physics` / `universe_supervisor` primitives, which this module only
//! feeds.
//!
//! The transform (authored circuit -> runtime inputs):
//!   1. Assign every atom string-key a stable, injective `EntityKey` and every
//!      bond string-key a stable `RelationKey`, both in first-seen order.
//!   2. The `deposit_bond` is REMOVED from the conduction graph and becomes a
//!      `PhysicsEventDeposit{trigger, target, weight}` — the `event -> +energy`
//!      edge. It never remains a conducting bond in either cluster.
//!   3. Atoms split into two clusters: the CONSTRUCT cluster is `{trigger_atom}`
//!      plus every atom reachable FORWARD from it over non-deposit bonds; the
//!      SENSOR cluster is every other atom. A bond joins a cluster only when both
//!      its endpoints are in that cluster (and it is not the deposit bond).
//!   4. `external_measured_injections{atom:energy}` set that atom's `seed_energy`
//!      in whichever cluster it lands in.
//!   5. Each atom's `required_supports` are kept as authored, minus any entry
//!      equal to the deposit-bond name (once the deposit is an injection it is no
//!      longer a conducting support).
//!   6. `effect_bindings[]` become `PhysicsEffectBinding`s carrying the CANDIDATE
//!      `EffectIntent` each emitter atom proposes.
//!
//! HONEST BOUNDARY (identical to `house_alarm_fire`): the authored circuit lists
//! `physics_intersection_event`'s seed as an `external_measured_injection` ONLY
//! to make the circuit self-consistent for local checks. On a live world that
//! energy MUST arrive from the real physics step via the physics-event ->
//! atom-deposit bridge, not from a hand seed. Materializing it as a seed proves
//! the circuit + resolver shape, NOT a real entry.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use universe_capabilities::EffectIntent;
use universe_core::{EntityKey, RelationKey, Tick};
use universe_physics::{AtomBond, AtomSpec, BondPolarity, LocalAtomCluster, PhysicsEventDeposit};
use universe_supervisor::PhysicsEffectBinding;

use crate::E2eError;

/// One authored atom of the circuit. Extra descriptive fields on the JSON node
/// (e.g. `roleAxis`) are ignored — only the firing physics is materialized.
#[derive(Clone, Debug, Deserialize)]
pub struct CircuitAtom {
    pub key: String,
    pub threshold: u64,
    #[serde(default)]
    pub seed_energy: u64,
    #[serde(default)]
    pub required_supports: Vec<String>,
    #[serde(default)]
    pub inhibition_threshold: Option<u64>,
}

/// One authored bond of the circuit. `energy_status` and other provenance fields
/// are ignored here; only the conduction physics is materialized.
#[derive(Clone, Debug, Deserialize)]
pub struct CircuitBond {
    pub key: String,
    pub source: String,
    pub target: String,
    pub polarity: String,
    pub energy: u64,
}

/// One authored emitter -> CANDIDATE effect binding.
#[derive(Clone, Debug, Deserialize)]
pub struct CircuitEffectBinding {
    pub emitter_atom: String,
    pub capability: String,
    pub idempotency_key: String,
    pub message: String,
    pub deadline_tick: u64,
    #[serde(default)]
    pub causal_ancestry: Vec<String>,
}

/// The authored `alarm_atom_circuit` block, deserialized verbatim. This is the
/// construct's graph projection — the resolver's only input.
#[derive(Clone, Debug, Deserialize)]
pub struct AlarmAtomCircuit {
    pub atoms: Vec<CircuitAtom>,
    pub bonds: Vec<CircuitBond>,
    pub deposit_bond: String,
    pub trigger_atom: String,
    #[serde(default)]
    pub effect_bindings: Vec<CircuitEffectBinding>,
    #[serde(default)]
    pub external_measured_injections: BTreeMap<String, u64>,
}

/// The runtime inputs `Supervisor::run_physics_deposit_phase` consumes, resolved
/// from the authored circuit. The two key maps are retained so a caller can name
/// atoms/bonds by their authored string keys when asserting on the outcome.
#[derive(Clone, Debug)]
pub struct ResolvedConstruct {
    pub sensor_cluster: LocalAtomCluster,
    pub deposit_bindings: Vec<PhysicsEventDeposit>,
    pub construct_cluster: LocalAtomCluster,
    pub effect_bindings: Vec<PhysicsEffectBinding>,
    pub atom_keys: BTreeMap<String, EntityKey>,
    pub bond_keys: BTreeMap<String, RelationKey>,
}

/// "support" -> Support, "inhibit" -> Inhibit, "neutral" -> Neutral. Anything
/// else is a hard error — a polarity is never invented.
fn parse_polarity(raw: &str) -> Result<BondPolarity, E2eError> {
    match raw {
        "support" => Ok(BondPolarity::Support),
        "inhibit" => Ok(BondPolarity::Inhibit),
        "neutral" => Ok(BondPolarity::Neutral),
        other => Err(E2eError::Contract(format!(
            "unknown bond polarity: {other:?} (expected support|inhibit|neutral)"
        ))),
    }
}

/// Read the AUTHORED atom circuit and materialize the exact runtime inputs
/// `Supervisor::run_physics_deposit_phase` consumes.
///
/// Pure materialization: no store, no snapshot, no commit, no new symbol. The
/// caller supplies the execution budget to the supervisor separately (graph
/// authority is never buried in the resolver).
pub fn resolve_construct(circuit: &AlarmAtomCircuit) -> Result<ResolvedConstruct, E2eError> {
    // (1) Assign stable, injective keys in first-seen order.
    let mut atom_keys: BTreeMap<String, EntityKey> = BTreeMap::new();
    for (index, atom) in circuit.atoms.iter().enumerate() {
        let key = EntityKey((index as u128) + 1);
        if atom_keys.insert(atom.key.clone(), key).is_some() {
            return Err(E2eError::Contract(format!(
                "duplicate atom key in circuit: {}",
                atom.key
            )));
        }
    }
    let mut bond_keys: BTreeMap<String, RelationKey> = BTreeMap::new();
    for (index, bond) in circuit.bonds.iter().enumerate() {
        let key = RelationKey((index as u128) + 1);
        if bond_keys.insert(bond.key.clone(), key).is_some() {
            return Err(E2eError::Contract(format!(
                "duplicate bond key in circuit: {}",
                bond.key
            )));
        }
    }

    let entity_of = |name: &str| -> Result<EntityKey, E2eError> {
        atom_keys
            .get(name)
            .copied()
            .ok_or_else(|| E2eError::Contract(format!("bond/binding names unknown atom: {name}")))
    };
    let bond_of = |name: &str| -> Result<RelationKey, E2eError> {
        bond_keys
            .get(name)
            .copied()
            .ok_or_else(|| E2eError::Contract(format!("required support names unknown bond: {name}")))
    };

    // (2) Turn the deposit bond into the `event -> +energy` edge and REMOVE it
    // from the conduction graph.
    let deposit_bond = circuit
        .bonds
        .iter()
        .find(|bond| bond.key == circuit.deposit_bond)
        .ok_or_else(|| {
            E2eError::Contract(format!(
                "deposit_bond names no bond: {}",
                circuit.deposit_bond
            ))
        })?;
    let deposit = PhysicsEventDeposit {
        trigger: entity_of(&deposit_bond.source)?,
        target: entity_of(&deposit_bond.target)?,
        weight: deposit_bond.energy,
    };

    // (3) Split atoms: construct = {trigger_atom} + everything reachable FORWARD
    // over non-deposit bonds; sensor = the rest.
    if !atom_keys.contains_key(&circuit.trigger_atom) {
        return Err(E2eError::Contract(format!(
            "trigger_atom names no atom: {}",
            circuit.trigger_atom
        )));
    }
    let mut forward: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for bond in &circuit.bonds {
        if bond.key == circuit.deposit_bond {
            continue; // the deposit edge does not conduct — it is an injection.
        }
        forward
            .entry(bond.source.as_str())
            .or_default()
            .push(bond.target.as_str());
    }
    let mut construct_names: BTreeSet<String> = BTreeSet::new();
    let mut frontier = vec![circuit.trigger_atom.as_str()];
    construct_names.insert(circuit.trigger_atom.clone());
    while let Some(node) = frontier.pop() {
        for &next in forward.get(node).into_iter().flatten() {
            if construct_names.insert(next.to_owned()) {
                frontier.push(next);
            }
        }
    }

    // (4) Which atoms carry an external measured injection.
    for name in circuit.external_measured_injections.keys() {
        if !atom_keys.contains_key(name) {
            return Err(E2eError::Contract(format!(
                "external_measured_injections names unknown atom: {name}"
            )));
        }
    }

    // Build the two atom vectors, preserving first-seen order within each cluster.
    let mut sensor_atoms: Vec<AtomSpec> = Vec::new();
    let mut construct_atoms: Vec<AtomSpec> = Vec::new();
    for atom in &circuit.atoms {
        // (4) seed application: an external measured injection overrides the seed.
        let seed_energy = circuit
            .external_measured_injections
            .get(&atom.key)
            .copied()
            .unwrap_or(atom.seed_energy);
        // (5) drop the deposit-bond name from required supports; map the rest.
        let required_supports = atom
            .required_supports
            .iter()
            .filter(|name| **name != circuit.deposit_bond)
            .map(|name| bond_of(name))
            .collect::<Result<Vec<_>, _>>()?;
        let spec = AtomSpec {
            key: entity_of(&atom.key)?,
            threshold: atom.threshold,
            seed_energy,
            required_supports,
            inhibition_threshold: atom.inhibition_threshold,
        };
        if construct_names.contains(&atom.key) {
            construct_atoms.push(spec);
        } else {
            sensor_atoms.push(spec);
        }
    }

    // Bonds: a bond joins a cluster only when BOTH endpoints are in it and it is
    // not the deposit bond. A spanning non-deposit bond joins neither.
    let mut sensor_bonds: Vec<AtomBond> = Vec::new();
    let mut construct_bonds: Vec<AtomBond> = Vec::new();
    for bond in &circuit.bonds {
        if bond.key == circuit.deposit_bond {
            continue;
        }
        let source_in_construct = construct_names.contains(&bond.source);
        let target_in_construct = construct_names.contains(&bond.target);
        let materialized = AtomBond {
            key: bond_of(&bond.key)?,
            source: entity_of(&bond.source)?,
            target: entity_of(&bond.target)?,
            polarity: parse_polarity(&bond.polarity)?,
            energy: bond.energy,
        };
        match (source_in_construct, target_in_construct) {
            (true, true) => construct_bonds.push(materialized),
            (false, false) => sensor_bonds.push(materialized),
            _ => {} // spanning non-deposit bond: joins neither cluster.
        }
    }

    // (6) Effect bindings: emitter atom -> CANDIDATE EffectIntent.
    let effect_bindings = circuit
        .effect_bindings
        .iter()
        .map(|binding| {
            Ok(PhysicsEffectBinding {
                atom: entity_of(&binding.emitter_atom)?,
                candidate: EffectIntent {
                    capability: binding.capability.clone(),
                    idempotency_key: binding.idempotency_key.clone(),
                    payload: binding.message.as_bytes().to_vec(),
                    deadline_tick: Tick(binding.deadline_tick),
                    causal_ancestry: binding.causal_ancestry.clone(),
                },
            })
        })
        .collect::<Result<Vec<_>, E2eError>>()?;

    Ok(ResolvedConstruct {
        sensor_cluster: LocalAtomCluster {
            atoms: sensor_atoms,
            bonds: sensor_bonds,
            injections: Vec::new(),
        },
        deposit_bindings: vec![deposit],
        construct_cluster: LocalAtomCluster {
            atoms: construct_atoms,
            bonds: construct_bonds,
            injections: Vec::new(),
        },
        effect_bindings,
        atom_keys,
        bond_keys,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny synthetic 4-atom circuit: two sensor atoms feed a gate, the gate's
    /// outgoing bond is the DEPOSIT onto the trigger, and the trigger conducts to
    /// the emitter. It exercises every branch of the transform.
    ///
    ///   sensor_a --a_to_gate--> sensor_gate --deposit--> trig --trig_to_emit--> emit
    ///
    /// The gate is the deposit source; the deposit crosses into the construct
    /// cluster {trig, emit}. The sensor cluster is {sensor_a, sensor_gate}.
    fn synthetic() -> AlarmAtomCircuit {
        AlarmAtomCircuit {
            atoms: vec![
                CircuitAtom {
                    key: "sensor_a".into(),
                    threshold: 100,
                    seed_energy: 0,
                    required_supports: vec![],
                    inhibition_threshold: None,
                },
                CircuitAtom {
                    key: "sensor_gate".into(),
                    threshold: 100,
                    seed_energy: 0,
                    required_supports: vec!["a_to_gate".into()],
                    inhibition_threshold: None,
                },
                CircuitAtom {
                    key: "trig".into(),
                    threshold: 100,
                    seed_energy: 0,
                    // Names the deposit bond: it MUST be dropped once the deposit
                    // becomes an injection rather than a conducting support.
                    required_supports: vec!["deposit".into()],
                    inhibition_threshold: None,
                },
                CircuitAtom {
                    key: "emit".into(),
                    threshold: 100,
                    seed_energy: 0,
                    required_supports: vec![],
                    inhibition_threshold: None,
                },
            ],
            bonds: vec![
                CircuitBond {
                    key: "a_to_gate".into(),
                    source: "sensor_a".into(),
                    target: "sensor_gate".into(),
                    polarity: "support".into(),
                    energy: 100,
                },
                CircuitBond {
                    key: "deposit".into(),
                    source: "sensor_gate".into(),
                    target: "trig".into(),
                    polarity: "support".into(),
                    energy: 100,
                },
                CircuitBond {
                    key: "trig_to_emit".into(),
                    source: "trig".into(),
                    target: "emit".into(),
                    polarity: "support".into(),
                    energy: 100,
                },
            ],
            deposit_bond: "deposit".into(),
            trigger_atom: "trig".into(),
            effect_bindings: vec![CircuitEffectBinding {
                emitter_atom: "emit".into(),
                capability: "safe.notify".into(),
                idempotency_key: "synthetic:notify".into(),
                message: "hi".into(),
                deadline_tick: 500,
                causal_ancestry: vec!["synthetic:cause".into()],
            }],
            external_measured_injections: BTreeMap::from([("sensor_a".into(), 100)]),
        }
    }

    fn atom(resolved: &ResolvedConstruct, name: &str) -> EntityKey {
        *resolved.atom_keys.get(name).unwrap()
    }

    #[test]
    fn resolves_split_deposit_seed_dropped_support_and_effect() {
        let resolved = resolve_construct(&synthetic()).unwrap();

        // The split: sensor = {sensor_a, sensor_gate}; construct = {trig, emit}.
        let sensor: BTreeSet<EntityKey> =
            resolved.sensor_cluster.atoms.iter().map(|a| a.key).collect();
        let construct: BTreeSet<EntityKey> = resolved
            .construct_cluster
            .atoms
            .iter()
            .map(|a| a.key)
            .collect();
        assert_eq!(
            sensor,
            BTreeSet::from([atom(&resolved, "sensor_a"), atom(&resolved, "sensor_gate")])
        );
        assert_eq!(
            construct,
            BTreeSet::from([atom(&resolved, "trig"), atom(&resolved, "emit")])
        );

        // The deposit bond conducts in NEITHER cluster.
        let deposit_key = *resolved.bond_keys.get("deposit").unwrap();
        assert!(resolved
            .sensor_cluster
            .bonds
            .iter()
            .chain(&resolved.construct_cluster.bonds)
            .all(|bond| bond.key != deposit_key));

        // Exactly one PhysicsEventDeposit{trigger, target, weight}.
        assert_eq!(resolved.deposit_bindings.len(), 1);
        assert_eq!(
            resolved.deposit_bindings[0],
            PhysicsEventDeposit {
                trigger: atom(&resolved, "sensor_gate"),
                target: atom(&resolved, "trig"),
                weight: 100,
            }
        );

        // Seed application: the external measured injection landed on sensor_a.
        let sensor_a_spec = resolved
            .sensor_cluster
            .atoms
            .iter()
            .find(|a| a.key == atom(&resolved, "sensor_a"))
            .unwrap();
        assert_eq!(sensor_a_spec.seed_energy, 100);

        // Deposit-bond dropped from required_supports: trig had ["deposit"] and is
        // now empty; sensor_gate keeps its real support bond.
        let trig_spec = resolved
            .construct_cluster
            .atoms
            .iter()
            .find(|a| a.key == atom(&resolved, "trig"))
            .unwrap();
        assert!(trig_spec.required_supports.is_empty());
        let gate_spec = resolved
            .sensor_cluster
            .atoms
            .iter()
            .find(|a| a.key == atom(&resolved, "sensor_gate"))
            .unwrap();
        assert_eq!(
            gate_spec.required_supports,
            vec![*resolved.bond_keys.get("a_to_gate").unwrap()]
        );

        // The effect binding is on the emitter, carrying the candidate intent.
        assert_eq!(resolved.effect_bindings.len(), 1);
        assert_eq!(resolved.effect_bindings[0].atom, atom(&resolved, "emit"));
        assert_eq!(resolved.effect_bindings[0].candidate.capability, "safe.notify");
        assert_eq!(resolved.effect_bindings[0].candidate.payload, b"hi".to_vec());
    }
}
