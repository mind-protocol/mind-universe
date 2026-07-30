//! Executable proof that a graph-stored ontology/physics mapping closes only
//! after independent measurements, and opens again under counterevidence.

use crate::E2eError;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use universe_core::{EntityKey, RelationKey, Tick};
use universe_physics::{AtomBond, AtomDynamics, AtomInjection, AtomRun, AtomSpec, BondPolarity};
use universe_store::{
    apply_event, load_seed, ContentRef, EntityRecord, EventRecord, RelationRecord,
    UniverseMutation, UniverseSnapshot, UniverseStore,
};

const EXPERIMENT: EntityKey = EntityKey(0x104);
const CLOSURE: EntityKey = EntityKey(0x10b);
const CONTRADICTION: EntityKey = EntityKey(0x10c);
const RECEIPT: EntityKey = EntityKey(0x1f0);
const RECEIPT_RELATION: RelationKey = RelationKey(0x2f0);
const LOOP_RECEIPT: EntityKey = EntityKey(0x1f1);
const LOOP_RECEIPT_RELATION: RelationKey = RelationKey(0x2f1);
const LOOP_EXPERIMENT: EntityKey = EntityKey(0x31c);
const OBJECTIVE_GAP: EntityKey = EntityKey(0x303);
const DECISION: EntityKey = EntityKey(0x304);
const OPTION_A: EntityKey = EntityKey(0x305);
const OPTION_B: EntityKey = EntityKey(0x306);
const EFFECT_INTENT: EntityKey = EntityKey(0x312);
const EPISTEMIC_HEALTH: EntityKey = EntityKey(0x315);
const OPERATIONAL_HEALTH: EntityKey = EntityKey(0x316);
const OUTCOME_HEALTH: EntityKey = EntityKey(0x317);
const LOOP_HEALTH: EntityKey = EntityKey(0x318);
const MECHANISM_A_REINFORCED: EntityKey = EntityKey(0x319);

#[derive(Clone, Debug)]
pub struct CovalidityConfig {
    pub seed_path: PathBuf,
    pub store_root: PathBuf,
    pub artifact_root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReasoningAudit {
    pub all_bonds_justified: bool,
    pub provenance_and_scope_are_non_energizing: bool,
    pub contradiction_is_inhibitory: bool,
    pub ontology_branch_has_two_measured_supports: bool,
    pub physics_branch_has_three_measured_supports: bool,
    pub conclusions_are_context_bounded: bool,
}

impl ReasoningAudit {
    fn valid(&self) -> bool {
        self.all_bonds_justified
            && self.provenance_and_scope_are_non_energizing
            && self.contradiction_is_inhibitory
            && self.ontology_branch_has_two_measured_supports
            && self.physics_branch_has_three_measured_supports
            && self.conclusions_are_context_bounded
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CovalidityReceipt {
    pub scope: String,
    pub universal_claim: bool,
    pub reasoning: ReasoningAudit,
    pub store_roundtrip_observed: bool,
    pub deterministic_trace_observed: bool,
    pub energy_conservation_observed: bool,
    pub closure_activated: bool,
    pub contradiction_blocks_closure: bool,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LoopLoopReceipt {
    pub scope: String,
    pub objective_gap_activated: bool,
    pub option_a_eligible: bool,
    pub option_b_rejected: bool,
    pub decision_activated: bool,
    pub effect_intent_activated: bool,
    pub effect_content: ContentRef,
    pub effect_store_readback_observed: bool,
    pub effect_receipt_observed: bool,
    pub epistemic_health_activated: bool,
    pub operational_health_activated: bool,
    pub outcome_health_activated: bool,
    pub loop_health_activated: bool,
    pub mechanism_a_reinforced: bool,
    pub missing_receipt_blocks_loop_health: bool,
    pub contradiction_blocks_decision: bool,
    pub energy_conserved: bool,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LoopLoopVerification {
    pub decision_injections: Vec<AtomInjection>,
    pub decision_run: AtomRun,
    pub outcome_injections: Vec<AtomInjection>,
    pub completion_run: AtomRun,
    pub missing_receipt_run: AtomRun,
    pub contradicted_decision_run: AtomRun,
    pub receipt: LoopLoopReceipt,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CovalidityManifest {
    pub seed_path: PathBuf,
    pub store_root: PathBuf,
    pub snapshot_hash: String,
    pub entity_count: usize,
    pub relation_count: usize,
    pub verified_content_records: usize,
    pub initial_run: AtomRun,
    pub comparison_run: AtomRun,
    pub measurement_injections: Vec<AtomInjection>,
    pub measured_run: AtomRun,
    pub counterfactual_run: AtomRun,
    pub receipt: CovalidityReceipt,
    pub receipt_content: ContentRef,
    pub loop_loop: LoopLoopVerification,
    pub loop_receipt_content: ContentRef,
    pub final_revision: u64,
    pub manifest_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
struct StoredAtom {
    kind: String,
    name: String,
    #[serde(default)]
    measurement_binding: Option<String>,
    physics: StoredAtomPhysics,
}

#[derive(Clone, Debug, Deserialize)]
struct StoredAtomPhysics {
    threshold: u64,
    #[serde(default)]
    seed_energy: u64,
    #[serde(default)]
    required_supports: Vec<RelationKey>,
    #[serde(default)]
    inhibition_threshold: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
struct StoredBond {
    kind: String,
    justification: String,
    logic: StoredBondLogic,
    physics: StoredBondPhysics,
}

#[derive(Clone, Debug, Deserialize)]
struct StoredBondLogic {
    role: String,
}

#[derive(Clone, Debug, Deserialize)]
struct StoredBondPhysics {
    polarity: BondPolarity,
    energy: u64,
}

struct LoadedCluster {
    dynamics: AtomDynamics,
    measurement_bindings: BTreeMap<String, EntityKey>,
    audit: ReasoningAudit,
}

pub fn run(config: &CovalidityConfig) -> Result<CovalidityManifest, E2eError> {
    fs::create_dir_all(&config.artifact_root).map_err(|error| E2eError::Io(error.to_string()))?;
    let seed = load_seed(&config.seed_path)?;
    let store = UniverseStore::open(&config.store_root)?;
    let installed = store.install_seed(&seed)?;

    let independent_store = UniverseStore::open(&config.store_root)?;
    let independent = independent_store.load_snapshot()?;
    let store_roundtrip_observed =
        installed == independent && installed.canonical_hash()? == independent.canonical_hash()?;
    let verified_content_records = verify_all_content(&independent_store, &independent)?;

    let mut primary = load_cluster(&independent_store, &independent)?;
    let mut comparison = load_cluster(&independent_store, &independent)?;
    let initial_run = primary.dynamics.run_until_quiescent(32)?;
    let comparison_run = comparison.dynamics.run_until_quiescent(32)?;
    let deterministic_trace_observed = initial_run == comparison_run;
    let energy_conservation_observed =
        initial_run.energy_conserved && comparison_run.energy_conserved;

    let mut measurement_injections = Vec::new();
    if store_roundtrip_observed {
        measurement_injections.push(inject_measurement(&mut primary, "store_roundtrip", 200)?);
    }
    if deterministic_trace_observed {
        measurement_injections.push(inject_measurement(
            &mut primary,
            "deterministic_trace",
            100,
        )?);
    }
    if energy_conservation_observed {
        measurement_injections.push(inject_measurement(
            &mut primary,
            "energy_conservation",
            200,
        )?);
    }
    let measured_run = primary.dynamics.run_until_quiescent(32)?;
    let closure_activated =
        primary.dynamics.fired(CLOSURE) && measured_run.energy_conserved && measured_run.quiescent;

    let mut counterfactual = load_cluster(&independent_store, &independent)?;
    counterfactual.dynamics.run_until_quiescent(32)?;
    inject_measurement(&mut counterfactual, "store_roundtrip", 200)?;
    inject_measurement(&mut counterfactual, "deterministic_trace", 100)?;
    inject_measurement(&mut counterfactual, "energy_conservation", 200)?;
    counterfactual
        .dynamics
        .inject(CONTRADICTION, 1, "counterfactual:active_contradiction")?;
    let counterfactual_run = counterfactual.dynamics.run_until_quiescent(32)?;
    let contradiction_blocks_closure = !counterfactual.dynamics.fired(CLOSURE)
        && counterfactual_run.energy_conserved
        && counterfactual_run.quiescent;

    let reasoning = primary.audit.clone();
    let passed = reasoning.valid()
        && store_roundtrip_observed
        && deterministic_trace_observed
        && energy_conservation_observed
        && closure_activated
        && contradiction_blocks_closure;
    let receipt = CovalidityReceipt {
        scope: "ontology-physics-covalidity fixture v0".into(),
        universal_claim: false,
        reasoning,
        store_roundtrip_observed,
        deterministic_trace_observed,
        energy_conservation_observed,
        closure_activated,
        contradiction_blocks_closure,
        status: if passed {
            "validated_for_fixture"
        } else {
            "not_validated"
        }
        .into(),
    };
    let loop_loop = verify_loop_loop(&mut primary, &independent_store, &independent)?;

    let receipt_content = store.append_content(
        &serde_json::to_value(&receipt).map_err(|error| E2eError::Contract(error.to_string()))?,
    )?;
    let loop_receipt_content = store.append_content(
        &serde_json::to_value(&loop_loop.receipt)
            .map_err(|error| E2eError::Contract(error.to_string()))?,
    )?;
    let mut published = independent;
    publish_receipt(
        &store,
        &mut published,
        ReceiptPublication {
            entity: RECEIPT,
            relation: RECEIPT_RELATION,
            source: EXPERIMENT,
            entity_idempotency: "ontology-physics-covalidity-receipt-v0",
            relation_idempotency: "ontology-physics-covalidity-receipt-link-v0",
            justification:
                "The executed ontology/physics protocol produced this independently read receipt.",
        },
        receipt_content.clone(),
    )?;
    publish_receipt(
        &store,
        &mut published,
        ReceiptPublication {
            entity: LOOP_RECEIPT,
            relation: LOOP_RECEIPT_RELATION,
            source: LOOP_EXPERIMENT,
            entity_idempotency: "loop-loop-receipt-v0",
            relation_idempotency: "loop-loop-receipt-link-v0",
            justification:
                "The executed Loop Loop produced this independently read and falsified receipt.",
        },
        loop_receipt_content.clone(),
    )?;

    let final_store = UniverseStore::open(&config.store_root)?;
    let final_snapshot = final_store.replay(final_store.load_snapshot()?)?;
    let readback: CovalidityReceipt = read_receipt(&final_store, &final_snapshot, RECEIPT)?;
    let loop_readback: LoopLoopReceipt = read_receipt(&final_store, &final_snapshot, LOOP_RECEIPT)?;
    if readback != receipt
        || loop_readback != loop_loop.receipt
        || !final_snapshot
            .relations
            .iter()
            .any(|relation| relation.key == RECEIPT_RELATION)
        || !final_snapshot
            .relations
            .iter()
            .any(|relation| relation.key == LOOP_RECEIPT_RELATION)
    {
        return Err(E2eError::Contract(
            "independent receipt readback does not match both publications".into(),
        ));
    }

    let manifest_path = config.artifact_root.join("covalidity-manifest.json");
    let manifest = CovalidityManifest {
        seed_path: config.seed_path.clone(),
        store_root: config.store_root.clone(),
        snapshot_hash: installed.canonical_hash()?,
        entity_count: installed.entities.len(),
        relation_count: installed.relations.len(),
        verified_content_records,
        initial_run,
        comparison_run,
        measurement_injections,
        measured_run,
        counterfactual_run,
        receipt,
        receipt_content,
        loop_loop,
        loop_receipt_content,
        final_revision: final_snapshot.revision.0,
        manifest_path: manifest_path.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| E2eError::Contract(error.to_string()))?;
    fs::write(&manifest_path, bytes).map_err(|error| E2eError::Io(error.to_string()))?;
    Ok(manifest)
}

fn verify_all_content(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
) -> Result<usize, E2eError> {
    let contents = snapshot
        .entities
        .iter()
        .filter_map(|entity| entity.content.as_ref())
        .chain(
            snapshot
                .relations
                .iter()
                .filter_map(|relation| relation.content.as_ref()),
        );
    let mut count = 0;
    for content in contents {
        store.read_content(content)?;
        count += 1;
    }
    Ok(count)
}

fn load_cluster(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
) -> Result<LoadedCluster, E2eError> {
    let mut atoms = Vec::new();
    let mut names = BTreeMap::new();
    let mut measurement_bindings = BTreeMap::new();
    for entity in &snapshot.entities {
        let content = entity
            .content
            .as_ref()
            .ok_or_else(|| E2eError::Contract("cluster Atom has no content".into()))?;
        let stored: StoredAtom = serde_json::from_value(store.read_content(content)?)
            .map_err(|error| E2eError::Contract(error.to_string()))?;
        if stored.kind != "atom" {
            return Err(E2eError::Contract("entity content is not an Atom".into()));
        }
        if names.insert(stored.name.clone(), entity.key).is_some() {
            return Err(E2eError::Contract("duplicate Atom name".into()));
        }
        if let Some(binding) = stored.measurement_binding {
            if measurement_bindings.insert(binding, entity.key).is_some() {
                return Err(E2eError::Contract("duplicate measurement binding".into()));
            }
        }
        atoms.push(AtomSpec {
            key: entity.key,
            threshold: stored.physics.threshold,
            seed_energy: stored.physics.seed_energy,
            required_supports: stored.physics.required_supports,
            inhibition_threshold: stored.physics.inhibition_threshold,
        });
    }

    let neutral_predicates = BTreeSet::from([
        "DERIVED_FROM",
        "TESTS",
        "PRODUCES",
        "MEASURES",
        "MEASURED_BY",
        "APPLIES_IN",
        "OPTION_FOR",
        "IMPLEMENTS",
        "ADDRESSES",
    ]);
    let mut bonds = Vec::new();
    let mut all_bonds_justified = true;
    let mut provenance_and_scope_are_non_energizing = true;
    let mut contradiction_is_inhibitory = false;
    let mut ontology_measured_supports = 0;
    let mut physics_measured_supports = 0;
    let mut contextualized = BTreeSet::new();
    for relation in &snapshot.relations {
        let content = relation
            .content
            .as_ref()
            .ok_or_else(|| E2eError::Contract("cluster Bond has no content".into()))?;
        let stored: StoredBond = serde_json::from_value(store.read_content(content)?)
            .map_err(|error| E2eError::Contract(error.to_string()))?;
        if stored.kind != "bond" {
            return Err(E2eError::Contract("relation content is not a Bond".into()));
        }
        all_bonds_justified &= !stored.justification.trim().is_empty();
        let predicate = snapshot
            .symbols
            .get(relation.predicate as usize)
            .ok_or_else(|| E2eError::Contract("missing relation predicate".into()))?;
        if neutral_predicates.contains(predicate.as_str()) {
            provenance_and_scope_are_non_energizing &=
                stored.physics.polarity == BondPolarity::Neutral && stored.physics.energy == 0;
        }
        if predicate == "CONTRADICTS" {
            contradiction_is_inhibitory |= stored.physics.polarity == BondPolarity::Inhibit
                && stored.physics.energy > 0
                && relation.target == CLOSURE;
        }
        if stored.logic.role == "measured_support" {
            if relation.target == EntityKey(0x109) {
                ontology_measured_supports += 1;
            }
            if relation.target == EntityKey(0x10a) {
                physics_measured_supports += 1;
            }
        }
        if predicate == "APPLIES_IN" {
            contextualized.insert(relation.source);
        }
        bonds.push(AtomBond {
            key: relation.key,
            source: relation.source,
            target: relation.target,
            polarity: stored.physics.polarity,
            energy: stored.physics.energy,
        });
    }

    let audit = ReasoningAudit {
        all_bonds_justified,
        provenance_and_scope_are_non_energizing,
        contradiction_is_inhibitory,
        ontology_branch_has_two_measured_supports: ontology_measured_supports >= 2,
        physics_branch_has_three_measured_supports: physics_measured_supports >= 3,
        conclusions_are_context_bounded: [EntityKey(0x109), EntityKey(0x10a), CLOSURE]
            .into_iter()
            .all(|entity| contextualized.contains(&entity)),
    };
    Ok(LoadedCluster {
        dynamics: AtomDynamics::new(atoms, bonds)?,
        measurement_bindings,
        audit,
    })
}

fn inject_measurement(
    cluster: &mut LoadedCluster,
    binding: &str,
    energy: u64,
) -> Result<AtomInjection, E2eError> {
    let atom = cluster
        .measurement_bindings
        .get(binding)
        .copied()
        .ok_or_else(|| E2eError::Contract(format!("missing measurement binding {binding}")))?;
    Ok(cluster
        .dynamics
        .inject(atom, energy, format!("measured:{binding}"))?)
}

fn verify_loop_loop(
    primary: &mut LoadedCluster,
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
) -> Result<LoopLoopVerification, E2eError> {
    let decision_injections = inject_decision_inputs(primary)?;
    let decision_run = primary.dynamics.run_until_quiescent(64)?;

    if !primary.dynamics.fired(EFFECT_INTENT) {
        return Err(E2eError::Contract(
            "Loop Loop effect cannot run before its intent fires".into(),
        ));
    }
    let effect_payload = serde_json::json!({
        "kind": "loop_fixture_effect",
        "mechanism": "mechanism_a_verified_execution",
        "objective": "loop_health_objective",
        "result": "durably_written"
    });
    let effect_content = store.append_content(&effect_payload)?;
    let effect_store_readback_observed = store.read_content(&effect_content)? == effect_payload;
    let mut outcome_injections = Vec::new();
    if effect_store_readback_observed {
        outcome_injections.push(inject_measurement(primary, "effect_receipt", 100)?);
        outcome_injections.push(inject_measurement(primary, "positive_outcome", 100)?);
    }
    let completion_run = primary.dynamics.run_until_quiescent(64)?;
    let effect_receipt = measurement_atom(primary, "effect_receipt")?;

    let mut missing_receipt = prepare_covalid_cluster(store, snapshot)?;
    inject_decision_inputs(&mut missing_receipt)?;
    missing_receipt.dynamics.run_until_quiescent(64)?;
    inject_measurement(&mut missing_receipt, "positive_outcome", 100)?;
    let missing_receipt_run = missing_receipt.dynamics.run_until_quiescent(64)?;
    let missing_receipt_blocks_loop_health = !missing_receipt.dynamics.fired(OPERATIONAL_HEALTH)
        && !missing_receipt.dynamics.fired(LOOP_HEALTH)
        && !missing_receipt.dynamics.fired(MECHANISM_A_REINFORCED);

    let mut contradicted = prepare_covalid_cluster(store, snapshot)?;
    inject_decision_inputs(&mut contradicted)?;
    inject_measurement(&mut contradicted, "decision_contradiction", 1)?;
    let contradicted_decision_run = contradicted.dynamics.run_until_quiescent(64)?;
    let contradiction_blocks_decision =
        !contradicted.dynamics.fired(DECISION) && !contradicted.dynamics.fired(EFFECT_INTENT);

    let energy_conserved = decision_run.energy_conserved
        && completion_run.energy_conserved
        && missing_receipt_run.energy_conserved
        && contradicted_decision_run.energy_conserved;
    let facts = LoopLoopReceipt {
        scope: "loop-loop fixture v0".into(),
        objective_gap_activated: primary.dynamics.fired(OBJECTIVE_GAP),
        option_a_eligible: primary.dynamics.fired(OPTION_A),
        option_b_rejected: !primary.dynamics.fired(OPTION_B),
        decision_activated: primary.dynamics.fired(DECISION),
        effect_intent_activated: primary.dynamics.fired(EFFECT_INTENT),
        effect_content,
        effect_store_readback_observed,
        effect_receipt_observed: primary.dynamics.fired(effect_receipt),
        epistemic_health_activated: primary.dynamics.fired(EPISTEMIC_HEALTH),
        operational_health_activated: primary.dynamics.fired(OPERATIONAL_HEALTH),
        outcome_health_activated: primary.dynamics.fired(OUTCOME_HEALTH),
        loop_health_activated: primary.dynamics.fired(LOOP_HEALTH),
        mechanism_a_reinforced: primary.dynamics.fired(MECHANISM_A_REINFORCED),
        missing_receipt_blocks_loop_health,
        contradiction_blocks_decision,
        energy_conserved,
        status: String::new(),
    };
    let passed = facts.objective_gap_activated
        && facts.option_a_eligible
        && facts.option_b_rejected
        && facts.decision_activated
        && facts.effect_intent_activated
        && facts.effect_store_readback_observed
        && facts.effect_receipt_observed
        && facts.epistemic_health_activated
        && facts.operational_health_activated
        && facts.outcome_health_activated
        && facts.loop_health_activated
        && facts.mechanism_a_reinforced
        && facts.missing_receipt_blocks_loop_health
        && facts.contradiction_blocks_decision
        && facts.energy_conserved;
    let receipt = LoopLoopReceipt {
        status: if passed {
            "validated_for_fixture"
        } else {
            "not_validated"
        }
        .into(),
        ..facts
    };
    Ok(LoopLoopVerification {
        decision_injections,
        decision_run,
        outcome_injections,
        completion_run,
        missing_receipt_run,
        contradicted_decision_run,
        receipt,
    })
}

fn prepare_covalid_cluster(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
) -> Result<LoadedCluster, E2eError> {
    let mut cluster = load_cluster(store, snapshot)?;
    cluster.dynamics.run_until_quiescent(64)?;
    inject_measurement(&mut cluster, "store_roundtrip", 200)?;
    inject_measurement(&mut cluster, "deterministic_trace", 100)?;
    inject_measurement(&mut cluster, "energy_conservation", 200)?;
    cluster.dynamics.run_until_quiescent(64)?;
    if !cluster.dynamics.fired(CLOSURE) {
        return Err(E2eError::Contract(
            "Loop Loop cannot start before co-validity closes".into(),
        ));
    }
    Ok(cluster)
}

fn inject_decision_inputs(cluster: &mut LoadedCluster) -> Result<Vec<AtomInjection>, E2eError> {
    [
        ("current_loop_state", 200),
        ("mechanism_a_evidence", 100),
        ("mechanism_a_feasible", 100),
        ("mechanism_a_safe", 100),
        ("mechanism_b_evidence", 100),
        ("mechanism_b_feasible", 100),
        ("mechanism_b_unsafe", 1),
        ("loop_context_match", 200),
        ("capability_available", 100),
    ]
    .into_iter()
    .map(|(binding, energy)| inject_measurement(cluster, binding, energy))
    .collect()
}

fn measurement_atom(cluster: &LoadedCluster, binding: &str) -> Result<EntityKey, E2eError> {
    cluster
        .measurement_bindings
        .get(binding)
        .copied()
        .ok_or_else(|| E2eError::Contract(format!("missing measurement binding {binding}")))
}

struct ReceiptPublication<'a> {
    entity: EntityKey,
    relation: RelationKey,
    source: EntityKey,
    entity_idempotency: &'a str,
    relation_idempotency: &'a str,
    justification: &'a str,
}

fn publish_receipt(
    store: &UniverseStore,
    snapshot: &mut UniverseSnapshot,
    publication: ReceiptPublication<'_>,
    content: ContentRef,
) -> Result<(), E2eError> {
    let observation_symbol = symbol_id(snapshot, "observation")?;
    let produces_symbol = symbol_id(snapshot, "PRODUCES")?;
    let entity_event = EventRecord::new(
        snapshot.universe,
        snapshot.revision,
        Tick(snapshot.tick.0 + 1),
        publication.entity_idempotency,
        UniverseMutation::PutEntity {
            entity: EntityRecord {
                key: publication.entity,
                generation: 0,
                symbol: observation_symbol,
                content: Some(content),
            },
        },
    )?;
    store.append_event(&entity_event)?;
    apply_event(snapshot, &entity_event)?;

    let relation_content = store.append_content(&serde_json::json!({
        "kind": "bond",
        "justification": publication.justification,
        "logic": {"role": "result_provenance"},
        "physics": {"polarity": "neutral", "energy": 0}
    }))?;
    let relation_event = EventRecord::new(
        snapshot.universe,
        snapshot.revision,
        Tick(snapshot.tick.0 + 1),
        publication.relation_idempotency,
        UniverseMutation::PutRelation {
            relation: RelationRecord {
                key: publication.relation,
                generation: 0,
                source: publication.source,
                target: publication.entity,
                predicate: produces_symbol,
                content: Some(relation_content),
            },
        },
    )?;
    store.append_event(&relation_event)?;
    apply_event(snapshot, &relation_event)?;
    Ok(())
}

fn read_receipt<T: for<'de> Deserialize<'de>>(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    entity: EntityKey,
) -> Result<T, E2eError> {
    let content = snapshot
        .entities
        .iter()
        .find(|candidate| candidate.key == entity)
        .and_then(|candidate| candidate.content.as_ref())
        .ok_or_else(|| E2eError::Contract(format!("receipt {entity} was not replayed")))?;
    serde_json::from_value(store.read_content(content)?)
        .map_err(|error| E2eError::Contract(error.to_string()))
}

fn symbol_id(snapshot: &UniverseSnapshot, symbol: &str) -> Result<u32, E2eError> {
    snapshot
        .symbols
        .iter()
        .position(|candidate| candidate == symbol)
        .and_then(|index| u32::try_from(index).ok())
        .ok_or_else(|| E2eError::Contract(format!("missing symbol {symbol}")))
}

pub fn default_config(repository: &Path, artifact_root: PathBuf) -> CovalidityConfig {
    CovalidityConfig {
        seed_path: repository.join("fixtures/atoms/ontology-physics-covalidity.json"),
        store_root: artifact_root.join("store"),
        artifact_root,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_store_trace_measurement_and_counterexample_close_the_fixture() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let temp = tempfile::tempdir().unwrap();
        let manifest = run(&default_config(&repository, temp.path().join("proof"))).unwrap();
        assert_eq!(manifest.receipt.status, "validated_for_fixture");
        assert!(!manifest.receipt.universal_claim);
        assert!(manifest.receipt.closure_activated);
        assert!(manifest.receipt.contradiction_blocks_closure);
        assert_eq!(manifest.loop_loop.receipt.status, "validated_for_fixture");
        assert!(manifest.loop_loop.receipt.loop_health_activated);
        assert!(
            manifest
                .loop_loop
                .receipt
                .missing_receipt_blocks_loop_health
        );
        assert!(manifest.loop_loop.receipt.contradiction_blocks_decision);
        assert_eq!(manifest.final_revision, 4);
        assert_eq!(
            manifest.verified_content_records,
            manifest.entity_count + manifest.relation_count
        );
    }
}
