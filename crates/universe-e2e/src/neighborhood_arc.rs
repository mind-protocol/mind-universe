//! The full arc: the board rides a canonical NEIGHBORHOOD as N EXECUTED
//! BehaviorBonds.
//!
//! It authors one BehaviorBond per selected canonical relation on top of the
//! shared behavior-bond authority scaffold (reused from the testkit), with each
//! bond's energy taken from the MEASURED-SEMANTIC seed
//! ([`crate::canonical_seed_energy`]). Every bond is then projected, materialized,
//! compiled, and EXECUTED through the real pipeline
//! ([`crate::behavior_runtime::build_projection`] + `execute_runtime_bond_artifact`),
//! and the board reads the N execution receipts as glide steps. This is the
//! generalization of `behavior_ride` (one bond) to a whole neighborhood: the
//! energy is `measured:semantic_v0`, the physics is executed and hash-verified.

use serde::Serialize;
use std::{collections::BTreeMap, path::Path};

use universe_compiler::{
    compile_materialized_behavior, materialize_behavior_bond, BehaviorMaterializationStatus,
};
use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseId};
use universe_physics::{AtomConvergence, BondPolarity};
use universe_query::{read_local_binding_subgraph, QueryBudget, QueryOrigin};
use universe_store::{
    ontology::{OntologyLoadBudget, OntologyRegistry},
    AdjacencyOverlayBudget, EntityRecord, RelationRecord, UniverseStore,
};
use universe_supervisor::{execute_runtime_bond_artifact, RuntimeBondExecutionReceipt};
use universe_testkit::{
    create_behavior_bond_authority_store, BehaviorBindingRelationKeys, BehaviorBondAuthorityKeys,
    BEHAVIOR_BOND_AUTHORITY_KEYS as SCAFFOLD,
};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

use crate::behavior_runtime::{build_projection, verify_complete_binding_query};
use crate::canonical_seed_energy::{seed_energy_from_canonical, HashingEmbedder, SEED_DIMENSIONS};
use crate::E2eError;

// Authority hashes copied verbatim from the shared ChangeSet (they hash the
// ChangeSet/ontology/mapping, not the bond, so every bond reuses them). The
// validator only checks their hex format, and every bond points at ChangeSet
// 0x3000, so this is honest reuse, not fabrication.
const CHANGE_SET_HASH: &str = "038797d60c21f0d1a5431ddb48d15fd631cb75989265bba5797654d562acf25e";
const MAPPING_HASH: &str = "be0d2d851112ac6f3025d80772d35189e3462e9d4b95a2aeb9afd31966ee5c29";
const ONTOLOGY_HASH: &str = "4c0ec977f1f81bcd2749c53c3dc8c3a9e7c217405a0a5e93d5d104a07783827d";

const BOND_ENTITY_BASE: u128 = 0x0005_0000;
const BOND_RELATION_BASE: u128 = 0x0005_0000;
const RELATIONS_PER_BOND: u128 = 16;

/// One executed bond, read back from its RuntimeBondExecutionReceipt.
#[derive(Clone, Debug, Serialize)]
pub struct ExecutedBondGlide {
    pub bond: EntityKey,
    pub source: EntityKey,
    pub target: EntityKey,
    pub predicate: String,
    pub transfer_energy: u64,
    pub target_fired: bool,
    pub converged: bool,
    pub energy_conserved: bool,
    pub artifact_hash: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct NeighborhoodArc {
    pub epistemic_status: String,
    pub start: EntityKey,
    pub bonds_executed: usize,
    pub all_targets_fired: bool,
    pub all_energy_conserved: bool,
    pub attractor: Option<EntityKey>,
    pub attractor_predicate: Option<String>,
    pub attractor_energy: u64,
    pub glides: Vec<ExecutedBondGlide>,
}

fn entity_hex(key: EntityKey) -> String {
    format!("{:032x}", key.0)
}

struct SelectedBond {
    target: EntityKey,
    predicate: String,
    energy: u64,
}

/// The executed neighborhood plus the raw execution receipts, so downstream
/// consumers (e.g. the desktop stream) can read each bond's measured transfers.
pub struct NeighborhoodExecution {
    pub arc: NeighborhoodArc,
    pub universe: UniverseId,
    pub revision: Revision,
    pub receipts: Vec<RuntimeBondExecutionReceipt>,
}

/// Ride a canonical neighborhood as N executed BehaviorBonds, returning the
/// board readout and the execution receipts. `max_bonds` caps how many of the
/// busiest node's strongest support relations are executed.
pub fn execute_neighborhood(
    repository: &Path,
    artifact_root: &Path,
    max_bonds: usize,
) -> Result<NeighborhoodExecution, E2eError> {
    // 1. Install the shared behavior-bond authority scaffold (revision 1).
    let store_root = artifact_root.join("store");
    create_behavior_bond_authority_store(&store_root)?;
    let store = UniverseStore::open(&store_root)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let registry = OntologyRegistry::load(&store, &snapshot, OntologyLoadBudget::default())?;

    // 2. Measure semantic energy over the canonical graph and pick the busiest
    //    node's strongest support relations.
    let seed = seed_energy_from_canonical(
        repository,
        &artifact_root.join("energy-store"),
        HashingEmbedder {
            dimensions: SEED_DIMENSIONS,
        },
        SEED_DIMENSIONS,
    )?;
    let mut per_source: BTreeMap<EntityKey, usize> = BTreeMap::new();
    for bond in &seed.bonds {
        if bond.polarity == BondPolarity::Support {
            *per_source.entry(bond.source).or_default() += 1;
        }
    }
    let start = per_source
        .iter()
        .max_by(|left, right| left.1.cmp(right.1).then(right.0 .0.cmp(&left.0 .0)))
        .map(|(key, _)| *key)
        .ok_or_else(|| E2eError::Contract("no measured support relation to execute".into()))?;
    let mut selected: Vec<SelectedBond> = seed
        .bonds
        .iter()
        .filter(|bond| {
            bond.source == start
                && bond.target != start
                && bond.polarity == BondPolarity::Support
                && registry.predicate(&bond.predicate).is_some()
        })
        .map(|bond| SelectedBond {
            target: bond.target,
            predicate: bond.predicate.clone(),
            energy: bond.energy.max(1),
        })
        .collect();
    selected.sort_by(|left, right| {
        right
            .energy
            .cmp(&left.energy)
            .then(left.target.0.cmp(&right.target.0))
    });
    selected.truncate(max_bonds.max(1));
    if selected.is_empty() {
        return Err(E2eError::Contract(
            "no executable canonical bond selected".into(),
        ));
    }

    // 3. Author one BehaviorBond per selected relation, committed as one
    //    ChangeSet on top of the scaffold (revision 2).
    let universe_revision = snapshot.revision.0 + 1;
    let symbol = |name: &str| -> Result<u32, E2eError> {
        snapshot
            .symbol_id(name)
            .ok_or_else(|| E2eError::Contract(format!("symbol {name} is not interned")))
    };
    let (bond_sym, mechanism_sym) = (symbol("behavior_bond")?, symbol("mechanism")?);
    let (source_atom_p, target_atom_p) = (symbol("SOURCE_ATOM")?, symbol("TARGET_ATOM")?);
    let (uses_predicate_p, uses_profile_p) = (symbol("USES_PREDICATE")?, symbol("USES_PROFILE")?);
    let (has_logic_role_p, gated_by_p) = (symbol("HAS_LOGIC_ROLE")?, symbol("GATED_BY")?);
    let (serves_objective_p, justified_by_p) =
        (symbol("SERVES_OBJECTIVE")?, symbol("JUSTIFIED_BY")?);
    let (applies_in_p, part_of_p) = (symbol("APPLIES_IN")?, symbol("PART_OF")?);

    // Relations to the ChangeSet and bindings must carry content, exactly like
    // the authority fixture. The content is identical across bonds, so it is
    // appended once and its ContentRef reused (the store dedups by hash).
    let binding_content = store.append_content(&serde_json::json!({
        "kind": "ontology_relation",
        "role": "behavior_binding",
        "justification": "Derived executable binding for a measured-semantic canonical bond."
    }))?;
    let membership_content = store.append_content(&serde_json::json!({
        "kind": "ontology_relation",
        "role": "changeset_membership",
        "justification": "The authored node belongs to the behavior-bond authority ChangeSet."
    }))?;

    let mut entity_commands = Vec::new();
    let mut relation_commands = Vec::new();
    let mut plans = Vec::new();
    for (index, bond) in selected.iter().enumerate() {
        let i = index as u128;
        let bond_key = EntityKey(BOND_ENTITY_BASE + i * 2);
        let profile_key = EntityKey(BOND_ENTITY_BASE + i * 2 + 1);
        let rb = BOND_RELATION_BASE + i * RELATIONS_PER_BOND;
        let predicate_key = registry
            .predicate(&bond.predicate)
            .ok_or_else(|| E2eError::Contract(format!("predicate {} vanished", bond.predicate)))?
            .key;

        // Profile carrying the measured-semantic energy.
        let profile_content = serde_json::json!({
            "kind": "behavior_physical_profile",
            "profile_id": format!("measured-semantic-{index}"),
            "status": "active_overlay",
            "runtime_binding": {
                "kind": "physical_profile",
                "value": {
                    "source_threshold": bond.energy,
                    "source_seed_energy": bond.energy,
                    "source_inhibition_threshold": null,
                    "target_threshold": bond.energy,
                    "target_seed_energy": 0,
                    "target_inhibition_threshold": null,
                    "transfer_energy": bond.energy
                }
            }
        });
        entity_commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: profile_key,
                generation: 0,
                symbol: mechanism_sym,
                content: Some(store.append_content(&profile_content)?),
            },
        });

        // Bond instance reusing the shared ChangeSet authority + budgets.
        let bond_content = serde_json::json!({
            "kind": "behavior_bond_instance",
            "status": "active_overlay",
            "runtime_binding": {
                "kind": "bond",
                "value": {
                    "authority": {
                        "behavior_revision": 1,
                        "change_set": entity_hex(SCAFFOLD.change_set),
                        "change_set_hash": CHANGE_SET_HASH,
                        "context": entity_hex(SCAFFOLD.context),
                        "mapping_hash": MAPPING_HASH,
                        "mapping_revision": 1,
                        "ontology_hash": ONTOLOGY_HASH,
                        "ontology_revision": 1,
                        "universe_revision": universe_revision
                    },
                    "budgets": {
                        "lifetime_ticks": 8,
                        "max_atoms": 16,
                        "max_bonds": 32,
                        "max_steps": 8,
                        "max_total_energy": 1000000,
                        "max_wake_cost": 4
                    }
                }
            }
        });
        entity_commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: bond_key,
                generation: 0,
                symbol: bond_sym,
                content: Some(store.append_content(&bond_content)?),
            },
        });

        // Binding relations (bond -> ...), keyed deterministically, each with
        // behavior-binding content.
        for (offset, predicate, target) in [
            (0u128, source_atom_p, start),
            (1, target_atom_p, bond.target),
            (2, uses_predicate_p, predicate_key),
            (3, uses_profile_p, profile_key),
            (4, has_logic_role_p, SCAFFOLD.support_role),
            (5, gated_by_p, SCAFFOLD.gates[0]),
            (6, gated_by_p, SCAFFOLD.gates[1]),
            (7, serves_objective_p, SCAFFOLD.objective),
            (8, justified_by_p, SCAFFOLD.justification),
            (9, applies_in_p, SCAFFOLD.context),
        ] {
            relation_commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(rb + offset),
                    generation: 0,
                    source: bond_key,
                    target,
                    predicate,
                    content: Some(binding_content.clone()),
                },
            });
        }
        // Changeset membership for the two new nodes.
        for (offset, member) in [(10u128, bond_key), (11, profile_key)] {
            relation_commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(rb + offset),
                    generation: 0,
                    source: member,
                    target: SCAFFOLD.change_set,
                    predicate: part_of_p,
                    content: Some(membership_content.clone()),
                },
            });
        }

        let keys = BehaviorBondAuthorityKeys {
            semantic_predicate: predicate_key,
            behavior_profile: profile_key,
            behavior_bond: bond_key,
            source: start,
            target: bond.target,
            binding_relations: BehaviorBindingRelationKeys {
                source_atom: RelationKey(rb),
                target_atom: RelationKey(rb + 1),
                uses_predicate: RelationKey(rb + 2),
                uses_profile: RelationKey(rb + 3),
                has_logic_role: RelationKey(rb + 4),
                gated_by: [RelationKey(rb + 5), RelationKey(rb + 6)],
                serves_objective: RelationKey(rb + 7),
                justified_by: RelationKey(rb + 8),
                applies_in: RelationKey(rb + 9),
            },
            ..SCAFFOLD
        };
        plans.push((keys, bond.predicate.clone(), bond.energy, bond.target));
    }

    let mut commands = entity_commands;
    commands.extend(relation_commands);
    let transaction = UniverseTransaction::prepare(
        &snapshot,
        UniverseWriteSet {
            base_revision: snapshot.revision,
            idempotency_key: "neighborhood-arc:v0".into(),
            commands,
        },
    )?;
    let commit_tick = Tick(snapshot.tick.0 + 1);
    transaction.commit(&store, &mut snapshot, commit_tick)?;
    let universe = snapshot.universe;
    let revision = snapshot.revision;

    // 4. Project, materialize, compile, and EXECUTE each bond.
    let indexed = store.load_current_overlay_indexed(AdjacencyOverlayBudget::default())?;
    let registry =
        OntologyRegistry::load(&store, indexed.snapshot(), OntologyLoadBudget::default())?;
    let budget = QueryBudget {
        max_entities: 32,
        max_relations: 32,
        max_depth: 1,
    };
    let mut glides = Vec::new();
    let mut receipts = Vec::new();
    for (keys, predicate, energy, target) in plans {
        let query =
            read_local_binding_subgraph(&indexed, QueryOrigin::Entity(keys.behavior_bond), budget);
        verify_complete_binding_query(&query, keys)?;
        let projection =
            build_projection(&store, indexed.snapshot(), &registry, keys, &query, budget)?;
        let materialization = materialize_behavior_bond(&projection);
        if materialization.receipt.status != BehaviorMaterializationStatus::Materialized {
            return Err(E2eError::Contract(format!(
                "bond {} rejected: {:?}",
                keys.behavior_bond, materialization.receipt.validation.issues
            )));
        }
        let compilation = compile_materialized_behavior(&materialization)
            .ok_or_else(|| E2eError::Contract("materialized bond did not compile".into()))?;
        let artifact = compilation
            .artifact
            .as_ref()
            .ok_or_else(|| E2eError::Contract("compiled bond has no artifact".into()))?;
        let execution = execute_runtime_bond_artifact(artifact)?;
        let target_fired = execution
            .physical
            .run
            .steps
            .iter()
            .any(|step| step.fired.contains(&target));
        glides.push(ExecutedBondGlide {
            bond: keys.behavior_bond,
            source: keys.source,
            target,
            predicate,
            transfer_energy: energy,
            target_fired,
            converged: execution.physical.convergence == AtomConvergence::Quiescent,
            energy_conserved: execution.physical.energy.conserved,
            artifact_hash: execution.artifact_hash.clone(),
        });
        receipts.push(execution);
    }

    let attractor = glides
        .iter()
        .filter(|glide| glide.target_fired)
        .max_by(|left, right| {
            left.transfer_energy
                .cmp(&right.transfer_energy)
                .then(right.target.0.cmp(&left.target.0))
        });
    let arc = NeighborhoodArc {
        epistemic_status: "measured:semantic_v0 / executed / hash-verified".into(),
        start,
        bonds_executed: glides.len(),
        all_targets_fired: glides.iter().all(|glide| glide.target_fired),
        all_energy_conserved: glides.iter().all(|glide| glide.energy_conserved),
        attractor: attractor.map(|glide| glide.target),
        attractor_predicate: attractor.map(|glide| glide.predicate.clone()),
        attractor_energy: attractor.map(|glide| glide.transfer_energy).unwrap_or(0),
        glides,
    };
    Ok(NeighborhoodExecution {
        arc,
        universe,
        revision,
        receipts,
    })
}

/// Convenience: run the arc and return only the board readout.
pub fn ride_executed_neighborhood(
    repository: &Path,
    artifact_root: &Path,
    max_bonds: usize,
) -> Result<NeighborhoodArc, E2eError> {
    execute_neighborhood(repository, artifact_root, max_bonds).map(|execution| execution.arc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_content_hash(hash: &str) -> bool {
        hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    }

    #[test]
    fn board_rides_a_canonical_neighborhood_as_executed_bonds() {
        let temp = tempfile::tempdir().unwrap();
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let arc = ride_executed_neighborhood(&repository, temp.path(), 3).unwrap();
        println!("{arc:#?}");

        assert!(arc.bonds_executed >= 2, "the arc must execute N>1 bonds");
        assert!(arc.all_targets_fired, "every executed bond's target fires");
        assert!(arc.all_energy_conserved);
        assert!(arc.attractor.is_some());
        assert!(arc.attractor_energy > 0);
        assert_eq!(
            arc.epistemic_status,
            "measured:semantic_v0 / executed / hash-verified"
        );
        for glide in &arc.glides {
            assert!(is_content_hash(&glide.artifact_hash));
        }
    }
}
