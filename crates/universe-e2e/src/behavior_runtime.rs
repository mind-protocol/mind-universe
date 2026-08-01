//! Real store-to-health BehaviorBond proof.

use crate::E2eError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use universe_compiler::{
    behavior_compilation_receipt_hash, behavior_loop_health_graph_inputs,
    compile_materialized_behavior, decode_behavior_loop_health, materialize_behavior_bond,
    BehaviorCompilationReceipt, BehaviorLoopHealthInput, BehaviorLoopHealthRecord,
    BehaviorMaterializationReceipt, BehaviorMaterializationStatus,
};
use universe_core::{EntityKey, Epistemic, Revision, UniverseError};
use universe_ir::{
    BehaviorGraphProjection, BehaviorGraphRelation, BehaviorLoopClosure, BehaviorPhysicalEvidence,
    BehaviorProjectionReadEvidence, BehaviorReadbackEvidence, BehaviorResolvedContent,
    BehaviorVocabulary, CodeDefinition, OntologyBindingStatus, QuerySpec, ResolvedBehaviorNode,
    Value,
};
use universe_physics::{
    assess_atom_physical_health, AtomPhysicalEnvelopeState, AtomPhysicalHealthCheck,
    AtomPhysicalHealthEvidence, AtomPhysicalHealthObservation,
};
use universe_query::{
    read_local_binding_subgraph, LocalBindingSubgraph, QueryBudget, QueryOrigin, QueryStatus,
};
use universe_store::{AdjacencyOverlayBudget, EntityRecord, UniverseSnapshot, UniverseStore};
use universe_supervisor::{
    execute_runtime_bond_artifact, PhaseHook, RuntimeBondExecutionReceipt, Supervisor, TickPhase,
};
use universe_testkit::{create_behavior_bond_authority_store, BehaviorBondAuthorityKeys};
use universe_transactions::{
    CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet,
};
use universe_vm::{execute_program, ExecutionLimits, ExecutionReceipt, VmHost};

const MAX_CONTENT_BYTES: u32 = 64 * 1024;
const QUERY_TIMEOUT_TICKS: u32 = 8;

#[derive(Clone, Debug)]
pub struct BehaviorRuntimeConfig {
    pub artifact_root: PathBuf,
    pub genesis_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BehaviorRuntimeManifest {
    pub correlation: String,
    pub store_root: PathBuf,
    pub authority_revision: Revision,
    pub authority_hash: String,
    pub adjacency: StoreAdjacencyEvidence,
    pub query: LocalBindingSubgraph,
    pub materialization: BehaviorMaterializationReceipt,
    pub compilation: BehaviorCompilationReceipt,
    pub execution: RuntimeBondExecutionReceipt,
    pub physical_health: AtomPhysicalHealthEvidence,
    pub health_code_entity: EntityKey,
    pub health_execution: ExecutionReceipt,
    pub loop_health: BehaviorLoopHealthRecord,
    pub receipt_commit: CommitReceipt,
    pub health_commit: CommitReceipt,
    pub final_revision: Revision,
    pub final_snapshot_hash: String,
    pub receipt_content_hashes: BTreeMap<String, String>,
    pub independent_readback: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoreAdjacencyEvidence {
    pub base_revision: Revision,
    pub current_revision: Revision,
    pub base_snapshot_hash: String,
    pub current_snapshot_hash: String,
    pub added_entities: usize,
    pub relation_additions: usize,
    pub changed_relations: usize,
    pub tombstones: usize,
    pub touched_entities: usize,
    pub events_applied: usize,
    pub compacted_snapshot_hash: String,
    pub compaction_equivalent: bool,
}

#[derive(Default)]
struct NoopHook;

impl PhaseHook for NoopHook {
    fn run(
        &mut self,
        _phase: TickPhase,
        _snapshot: &UniverseSnapshot,
    ) -> Result<(), UniverseError> {
        Ok(())
    }
}

#[derive(Default)]
struct HealthVmHost;

impl VmHost for HealthVmHost {
    fn is_cancelled(&self) -> bool {
        false
    }

    fn capabilities(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn open_query(
        &mut self,
        _spec: &QuerySpec,
        _origin: &Value,
        _selector: &Value,
    ) -> Result<Value, String> {
        Err("Behavior health CodeDefinition cannot query".into())
    }

    fn await_query(&mut self, _handle: &Value) -> Result<Value, String> {
        Err("Behavior health CodeDefinition cannot await queries".into())
    }

    fn follow_one(&mut self, _source: &Value, _predicate: &Value) -> Result<Value, String> {
        Err("Behavior health CodeDefinition cannot traverse".into())
    }

    fn entity_symbol(&mut self, _entity: &Value) -> Result<Value, String> {
        Err("Behavior health CodeDefinition cannot inspect symbols".into())
    }

    fn hydrate(&mut self, _selected: &[Value], _max_bytes: u32) -> Result<Vec<Value>, String> {
        Err("Behavior health CodeDefinition cannot hydrate".into())
    }
}

pub fn run(config: &BehaviorRuntimeConfig) -> Result<BehaviorRuntimeManifest, E2eError> {
    let correlation = unique_correlation();
    let run_root = config.artifact_root.join(&correlation);
    let store_root = run_root.join("store");
    fs::create_dir_all(&run_root).map_err(|error| E2eError::Io(error.to_string()))?;

    let install = create_behavior_bond_authority_store(&store_root)?;
    let authority = install.readback;
    let keys = authority.keys;
    let authority_revision = authority.snapshot.revision;
    let authority_hash = authority.registry.authority_hash.clone();
    let query_budget = QueryBudget {
        max_entities: 16,
        max_relations: 16,
        max_depth: 1,
    };
    let store = UniverseStore::open(&store_root)?;
    let indexed = store.load_current_overlay_indexed(AdjacencyOverlayBudget::default())?;
    let current_snapshot_hash = indexed.snapshot().canonical_hash()?;
    if indexed.snapshot().revision != authority_revision
        || current_snapshot_hash != authority.snapshot.canonical_hash()?
    {
        return Err(contract(
            "overlay-indexed Store readback differs from installed graph authority",
        ));
    }
    let compacted = indexed.clone().compact()?;
    let compacted_snapshot_hash = compacted.snapshot().canonical_hash()?;
    if compacted_snapshot_hash != current_snapshot_hash {
        return Err(contract(
            "overlay compaction changed the authoritative snapshot hash",
        ));
    }
    let adjacency = StoreAdjacencyEvidence {
        base_revision: indexed.overlay().base_revision(),
        current_revision: indexed.overlay().current_revision(),
        base_snapshot_hash: indexed.overlay().base_snapshot_hash().to_owned(),
        current_snapshot_hash,
        added_entities: indexed.overlay().added_entity_count(),
        relation_additions: indexed.overlay().relation_addition_count(),
        changed_relations: indexed.overlay().changed_relation_count(),
        tombstones: indexed.overlay().tombstone_count(),
        touched_entities: indexed.overlay().touched_entity_count(),
        events_applied: indexed.overlay().events_applied(),
        compacted_snapshot_hash,
        compaction_equivalent: true,
    };
    let query = read_local_binding_subgraph(
        &indexed,
        QueryOrigin::Entity(keys.behavior_bond),
        query_budget,
    );
    verify_complete_binding_query(&query, keys)?;

    let projection = build_projection(
        &store,
        indexed.snapshot(),
        &authority.registry,
        keys,
        &query,
        query_budget,
    )?;
    let materialization = materialize_behavior_bond(&projection);
    if materialization.receipt.status != BehaviorMaterializationStatus::Materialized {
        return Err(contract(format!(
            "BehaviorBond materialization rejected: {:?}",
            materialization.receipt.validation.issues
        )));
    }
    let compilation = compile_materialized_behavior(&materialization)
        .ok_or_else(|| contract("materialized BehaviorBond did not compile"))?;
    let artifact = compilation
        .artifact
        .as_ref()
        .ok_or_else(|| contract("compiled BehaviorBond has no runtime artifact"))?;
    artifact
        .verify()
        .map_err(|error| contract(error.to_string()))?;
    let execution = execute_runtime_bond_artifact(artifact)?;
    let physical_health =
        assess_atom_physical_health(AtomPhysicalHealthObservation::Executed(&execution.physical));
    let health_code_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/graph-ir/behavior-loop-health.json");
    let health_code: CodeDefinition = serde_json::from_slice(
        &fs::read(&health_code_path).map_err(|error| E2eError::Io(error.to_string()))?,
    )
    .map_err(json_contract)?;
    universe_compiler::validate(&health_code).map_err(|error| {
        contract(format!(
            "Behavior health CodeDefinition is invalid: {error}"
        ))
    })?;

    let compilation_value = serde_json::to_value(&compilation.receipt).map_err(json_contract)?;
    let execution_value = serde_json::to_value(&execution).map_err(json_contract)?;
    let health_code_value = serde_json::to_value(&health_code).map_err(json_contract)?;
    let compilation_content = store.append_content(&compilation_value)?;
    let execution_content = store.append_content(&execution_value)?;
    let health_code_content = store.append_content(&health_code_value)?;
    let expected_compilation_hash = behavior_compilation_receipt_hash(&compilation.receipt);

    let mut supervisor = Supervisor::boot(&store_root, &config.genesis_path)?;
    let receipt_keys = next_entity_keys(supervisor.snapshot(), 5)?;
    let receipt_symbols = vec![
        "BehaviorCompilationReceipt".to_owned(),
        "RuntimeBondExecutionReceipt".to_owned(),
        "BehaviorHealthCodeDefinition".to_owned(),
        "BehaviorHealthExecutionReceipt".to_owned(),
        "BehaviorLoopHealthRecord".to_owned(),
    ];
    let symbol_plan = supervisor
        .snapshot()
        .plan_symbol_interning(&receipt_symbols)?;
    let mut receipt_commands = Vec::new();
    if !symbol_plan.additions.is_empty() {
        receipt_commands.push(UniverseCommand::InternSymbols {
            symbols: symbol_plan.additions,
        });
    }
    receipt_commands.extend([
        UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: receipt_keys[0],
                generation: 0,
                symbol: symbol_plan.assignments[&receipt_symbols[0]],
                content: Some(compilation_content.clone()),
            },
        },
        UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: receipt_keys[1],
                generation: 0,
                symbol: symbol_plan.assignments[&receipt_symbols[1]],
                content: Some(execution_content.clone()),
            },
        },
        UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: receipt_keys[2],
                generation: 0,
                symbol: symbol_plan.assignments[&receipt_symbols[2]],
                content: Some(health_code_content.clone()),
            },
        },
    ]);
    supervisor.enqueue(UniverseTransaction::prepare(
        supervisor.snapshot(),
        UniverseWriteSet {
            base_revision: supervisor.revision(),
            idempotency_key: format!("{correlation}:runtime-receipts"),
            commands: receipt_commands,
        },
    )?);
    let receipt_commit = one_commit(supervisor.advance(&mut NoopHook)?)?;

    let receipt_readback = supervisor.independent_readback()?;
    let readback_compilation: BehaviorCompilationReceipt =
        read_entity_content(&store, &receipt_readback, receipt_keys[0])?;
    let readback_execution: RuntimeBondExecutionReceipt =
        read_entity_content(&store, &receipt_readback, receipt_keys[1])?;
    let readback_health_code: CodeDefinition =
        read_entity_content(&store, &receipt_readback, receipt_keys[2])?;
    if readback_compilation != compilation.receipt
        || readback_execution != execution
        || readback_health_code != health_code
    {
        return Err(contract(
            "independent runtime/code readback differs from committed content",
        ));
    }
    let receipt_readback_hash = receipt_readback.canonical_hash()?;
    let physical = behavior_physical_evidence(
        keys.behavior_bond,
        &artifact.artifact_hash,
        &execution_content.sha256,
        &physical_health,
    );
    let behavior = materialization
        .bond
        .as_ref()
        .ok_or_else(|| contract("materialized BehaviorBond disappeared"))?;
    let readback = BehaviorReadbackEvidence {
        behavior_bond: keys.behavior_bond,
        projection_hash: materialization.receipt.projection_hash.clone(),
        compilation_receipt_hash: expected_compilation_hash.clone(),
        artifact_hash: artifact.artifact_hash.clone(),
        execution_receipt_hash: execution_content.sha256.clone(),
        independent_readback_hash: receipt_readback_hash.clone(),
        content_hashes_verified: true,
        contradictory: behavior.gates.iter().any(|gate| gate.contradictory),
    };
    let health_input = BehaviorLoopHealthInput {
        compilation: Epistemic::Measured(compilation.receipt.clone()),
        physical,
        readback: Epistemic::Measured(readback),
    };
    let health_execution = execute_program(
        &readback_health_code,
        &mut HealthVmHost,
        &behavior_loop_health_graph_inputs(&health_input),
        receipt_readback.revision,
        receipt_readback.tick,
        ExecutionLimits {
            fuel: 64,
            max_proposals: 0,
        },
    )
    .map_err(|error| contract(format!("Behavior health VM execution failed: {error}")))?;
    if !health_execution.proposals.is_empty() {
        return Err(contract(
            "Behavior health CodeDefinition emitted a forbidden write proposal",
        ));
    }
    let graph_health = decode_behavior_loop_health(&health_execution.result)
        .map_err(|error| contract(error.to_string()))?;
    let loop_health = BehaviorLoopHealthRecord {
        behavior_bond: Some(keys.behavior_bond),
        health: graph_health,
        blockers: Vec::new(),
        compilation_receipt_hash: Some(expected_compilation_hash.clone()),
        projection_hash: compilation.receipt.projection_hash.clone(),
        artifact_hash: compilation.receipt.artifact_hash.clone(),
        execution_receipt_hash: Some(execution_content.sha256.clone()),
        independent_readback_hash: Some(receipt_readback_hash),
    };
    if loop_health.health != Epistemic::Measured(BehaviorLoopClosure::Closed) {
        return Err(contract(format!(
            "graph-owned Behavior Loop health did not close: {:?}",
            health_execution.result
        )));
    }

    let health_execution_value = serde_json::to_value(&health_execution).map_err(json_contract)?;
    let health_value = serde_json::to_value(&loop_health).map_err(json_contract)?;
    let health_execution_content = store.append_content(&health_execution_value)?;
    let health_content = store.append_content(&health_value)?;
    supervisor.enqueue(UniverseTransaction::prepare(
        supervisor.snapshot(),
        UniverseWriteSet {
            base_revision: supervisor.revision(),
            idempotency_key: format!("{correlation}:loop-health"),
            commands: vec![
                UniverseCommand::PutEntity {
                    entity: EntityRecord {
                        key: receipt_keys[3],
                        generation: 0,
                        symbol: symbol_plan.assignments[&receipt_symbols[3]],
                        content: Some(health_execution_content.clone()),
                    },
                },
                UniverseCommand::PutEntity {
                    entity: EntityRecord {
                        key: receipt_keys[4],
                        generation: 0,
                        symbol: symbol_plan.assignments[&receipt_symbols[4]],
                        content: Some(health_content.clone()),
                    },
                },
            ],
        },
    )?);
    let health_commit = one_commit(supervisor.advance(&mut NoopHook)?)?;
    let final_readback = supervisor.independent_readback()?;
    let stored_health_execution: ExecutionReceipt =
        read_entity_content(&store, &final_readback, receipt_keys[3])?;
    let stored_health: BehaviorLoopHealthRecord =
        read_entity_content(&store, &final_readback, receipt_keys[4])?;
    if stored_health_execution != health_execution || stored_health != loop_health {
        return Err(contract(
            "independent graph health execution/readback differs from committed content",
        ));
    }
    universe_compiler::RuntimeBondArtifact {
        artifact_hash: readback_execution.artifact_hash.clone(),
        plan: readback_execution.plan.clone(),
    }
    .verify()
    .map_err(|error| contract(error.to_string()))?;

    let manifest = BehaviorRuntimeManifest {
        correlation,
        store_root,
        authority_revision,
        authority_hash,
        adjacency,
        query,
        materialization: materialization.receipt,
        compilation: compilation.receipt,
        execution,
        physical_health,
        health_code_entity: receipt_keys[2],
        health_execution,
        loop_health,
        receipt_commit,
        health_commit,
        final_revision: final_readback.revision,
        final_snapshot_hash: final_readback.canonical_hash()?,
        receipt_content_hashes: BTreeMap::from([
            ("compilation".into(), compilation_content.sha256),
            ("execution".into(), execution_content.sha256),
            ("health_code".into(), health_code_content.sha256),
            ("health_execution".into(), health_execution_content.sha256),
            ("loop_health".into(), health_content.sha256),
        ]),
        independent_readback: true,
    };
    let manifest_path = run_root.join("manifest.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).map_err(json_contract)?,
    )
    .map_err(|error| E2eError::Io(error.to_string()))?;
    let manifest_readback: BehaviorRuntimeManifest = serde_json::from_slice(
        &fs::read(&manifest_path).map_err(|error| E2eError::Io(error.to_string()))?,
    )
    .map_err(json_contract)?;
    if manifest_readback != manifest {
        return Err(contract(
            "artifact manifest readback differs from run result",
        ));
    }
    Ok(manifest)
}

pub fn verify_complete_binding_query(
    query: &LocalBindingSubgraph,
    keys: BehaviorBondAuthorityKeys,
) -> Result<(), E2eError> {
    let expected = BTreeSet::from([
        keys.binding_relations.source_atom,
        keys.binding_relations.target_atom,
        keys.binding_relations.uses_predicate,
        keys.binding_relations.uses_profile,
        keys.binding_relations.has_logic_role,
        keys.binding_relations.gated_by[0],
        keys.binding_relations.gated_by[1],
        keys.binding_relations.serves_objective,
        keys.binding_relations.justified_by,
        keys.binding_relations.applies_in,
    ]);
    let actual = query
        .relations
        .iter()
        .map(|relation| relation.key)
        .collect::<BTreeSet<_>>();
    if query.situation.status != QueryStatus::Complete
        || !query.frontier_entities.is_empty()
        || !expected.is_subset(&actual)
    {
        return Err(contract(format!(
            "local BehaviorBond query is incomplete: status={:?}, frontier={:?}, missing={:?}",
            query.situation.status,
            query.frontier_entities,
            expected.difference(&actual).collect::<Vec<_>>()
        )));
    }
    Ok(())
}

pub fn build_projection(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    registry: &universe_store::ontology::OntologyRegistry,
    keys: BehaviorBondAuthorityKeys,
    query: &LocalBindingSubgraph,
    budget: QueryBudget,
) -> Result<BehaviorGraphProjection, E2eError> {
    let relations = query
        .relations
        .iter()
        .map(|local| {
            let record = snapshot
                .relations
                .iter()
                .find(|record| record.key == local.key)
                .ok_or_else(|| contract(format!("relation {} disappeared", local.key)))?;
            let predicate_name = snapshot
                .symbols
                .get(record.predicate as usize)
                .ok_or_else(|| contract("relation predicate symbol is out of range"))?;
            let predicate = registry
                .predicate(predicate_name)
                .ok_or_else(|| {
                    contract(format!(
                        "active registry cannot resolve predicate {predicate_name}"
                    ))
                })?
                .key;
            Ok(BehaviorGraphRelation {
                relation: record.key,
                source: record.source,
                predicate,
                target: record.target,
            })
        })
        .collect::<Result<Vec<_>, E2eError>>()?;

    let required_contents = [
        keys.behavior_bond,
        keys.semantic_predicate,
        keys.behavior_profile,
        keys.support_role,
        keys.gates[0],
        keys.gates[1],
        keys.objective,
        keys.justification,
    ];
    let mut hydrated_bytes = 0usize;
    let contents = required_contents
        .into_iter()
        .map(|entity| {
            let record = snapshot
                .entities
                .iter()
                .find(|record| record.key == entity)
                .ok_or_else(|| contract(format!("content entity {entity} disappeared")))?;
            let content = record
                .content
                .as_ref()
                .ok_or_else(|| contract(format!("content entity {entity} has no ContentRef")))?;
            let value = store.read_content(content)?;
            hydrated_bytes = hydrated_bytes
                .saturating_add(serde_json::to_vec(&value).map_err(json_contract)?.len());
            let resolved = if entity == keys.semantic_predicate {
                if registry
                    .runtime_diagnostics_for(entity)
                    .iter()
                    .any(|diagnostic| diagnostic.runtime_blocking)
                {
                    BehaviorResolvedContent::Predicate(OntologyBindingStatus::UnresolvedGap)
                } else {
                    BehaviorResolvedContent::Predicate(OntologyBindingStatus::Active)
                }
            } else if entity == keys.objective {
                BehaviorResolvedContent::Objective
            } else if entity == keys.justification {
                BehaviorResolvedContent::Justification
            } else {
                serde_json::from_value(value.get("runtime_binding").cloned().ok_or_else(|| {
                    contract(format!("content entity {entity} has no runtime_binding"))
                })?)
                .map_err(json_contract)?
            };
            Ok(ResolvedBehaviorNode {
                entity,
                content_hash: content.sha256.clone(),
                value: Epistemic::Measured(resolved),
            })
        })
        .collect::<Result<Vec<_>, E2eError>>()?;
    if hydrated_bytes > MAX_CONTENT_BYTES as usize {
        return Err(contract("BehaviorBond hydration exceeded its byte budget"));
    }

    Ok(BehaviorGraphProjection {
        behavior_bond: keys.behavior_bond,
        vocabulary: BehaviorVocabulary {
            source_atom: keys.binding_predicates.source_atom,
            target_atom: keys.binding_predicates.target_atom,
            uses_predicate: keys.binding_predicates.uses_predicate,
            uses_profile: keys.binding_predicates.uses_profile,
            has_logic_role: keys.binding_predicates.has_logic_role,
            gated_by: keys.binding_predicates.gated_by,
            serves_objective: keys.binding_predicates.serves_objective,
            justified_by: keys.binding_predicates.justified_by,
        },
        relations,
        contents,
        local_read: Epistemic::Measured(BehaviorProjectionReadEvidence {
            origin: keys.behavior_bond,
            universe_revision: snapshot.revision,
            max_entities: u32::try_from(budget.max_entities)
                .map_err(|_| contract("query max_entities does not fit u32"))?,
            max_relations: u32::try_from(budget.max_relations)
                .map_err(|_| contract("query max_relations does not fit u32"))?,
            max_content_bytes: MAX_CONTENT_BYTES,
            timeout_ticks: QUERY_TIMEOUT_TICKS,
            complete_for_behavior: true,
            query_receipt_hash: hash_json(query)?,
            independent_readback_hash: snapshot.canonical_hash()?,
            active_registry_hash: registry.authority_hash.clone(),
        }),
    })
}

fn behavior_physical_evidence(
    behavior_bond: EntityKey,
    artifact_hash: &str,
    execution_receipt_hash: &str,
    evidence: &AtomPhysicalHealthEvidence,
) -> Epistemic<BehaviorPhysicalEvidence> {
    let pass = |value: &Epistemic<AtomPhysicalHealthCheck>| {
        matches!(value, Epistemic::Measured(AtomPhysicalHealthCheck::Pass))
    };
    if !matches!(
        evidence.envelope,
        Epistemic::Measured(AtomPhysicalEnvelopeState::WithinEnvelope)
    ) {
        return match &evidence.envelope {
            Epistemic::KnownAbsent => Epistemic::KnownAbsent,
            Epistemic::Unknown => Epistemic::Unknown,
            Epistemic::NotMeasured | Epistemic::Observed(_) => Epistemic::NotMeasured,
            Epistemic::MeasurementFailed { reason } => Epistemic::MeasurementFailed {
                reason: reason.clone(),
            },
            Epistemic::Measured(AtomPhysicalEnvelopeState::OutsideEnvelope) => {
                Epistemic::Measured(BehaviorPhysicalEvidence {
                    behavior_bond,
                    artifact_hash: artifact_hash.into(),
                    execution_receipt_hash: execution_receipt_hash.into(),
                    converged: pass(&evidence.convergence),
                    energy_conserved: pass(&evidence.energy_conservation),
                    contained: pass(&evidence.containment),
                    released: pass(&evidence.release),
                    lifetime_within_limit: pass(&evidence.lifetime),
                })
            }
            Epistemic::Measured(AtomPhysicalEnvelopeState::WithinEnvelope) => unreachable!(),
        };
    }
    Epistemic::Measured(BehaviorPhysicalEvidence {
        behavior_bond,
        artifact_hash: artifact_hash.into(),
        execution_receipt_hash: execution_receipt_hash.into(),
        converged: pass(&evidence.convergence),
        energy_conserved: pass(&evidence.energy_conservation),
        contained: pass(&evidence.containment),
        released: pass(&evidence.release),
        lifetime_within_limit: pass(&evidence.lifetime),
    })
}

fn next_entity_keys(snapshot: &UniverseSnapshot, count: usize) -> Result<Vec<EntityKey>, E2eError> {
    let first = snapshot
        .entities
        .iter()
        .map(|entity| entity.key.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| contract("entity key space exhausted"))?;
    (0..count)
        .map(|offset| {
            first
                .checked_add(offset as u128)
                .map(EntityKey)
                .ok_or_else(|| contract("entity key space exhausted"))
        })
        .collect()
}

fn one_commit(receipts: Vec<CommitReceipt>) -> Result<CommitReceipt, E2eError> {
    if receipts.len() != 1 {
        return Err(contract(format!(
            "expected one commit receipt, observed {}",
            receipts.len()
        )));
    }
    Ok(receipts.into_iter().next().expect("length checked"))
}

fn read_entity_content<T: for<'de> Deserialize<'de>>(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    entity: EntityKey,
) -> Result<T, E2eError> {
    let content = snapshot
        .entities
        .iter()
        .find(|record| record.key == entity)
        .and_then(|record| record.content.as_ref())
        .ok_or_else(|| contract(format!("receipt entity {entity} has no content")))?;
    serde_json::from_value(store.read_content(content)?).map_err(json_contract)
}

fn hash_json(value: &impl Serialize) -> Result<String, E2eError> {
    let bytes = serde_json::to_vec(value).map_err(json_contract)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn unique_correlation() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("behavior-runtime-{}-{nanos}", std::process::id())
}

fn contract(message: impl Into<String>) -> E2eError {
    E2eError::Contract(message.into())
}

fn json_contract(error: serde_json::Error) -> E2eError {
    contract(error.to_string())
}

pub fn default_genesis_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/genesis/minimal-genesis.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_store_behavior_closes_health_and_reads_every_receipt_back() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = run(&BehaviorRuntimeConfig {
            artifact_root: temp.path().to_path_buf(),
            genesis_path: default_genesis_path(),
        })
        .unwrap();
        assert_eq!(
            manifest.loop_health.health,
            Epistemic::Measured(BehaviorLoopClosure::Closed)
        );
        assert!(manifest.independent_readback);
        assert_eq!(manifest.query.situation.status, QueryStatus::Complete);
        assert!(manifest.execution.physical.energy.conserved);
    }
}
