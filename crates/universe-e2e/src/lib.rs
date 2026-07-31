//! Complete headless proof harness. It composes public bootstrap APIs and owns
//! no Universe behavior: the query program, selector, limits, and proposal kind
//! are loaded from graph fixtures.

pub mod behavior_ride;
pub mod behavior_runtime;
pub mod board;
pub mod canonical;
pub mod canonical_ride;
pub mod canonical_seed_energy;
pub mod cluster;
pub mod construct_resolver;
pub mod conversation_ride;
pub mod conversation_seed_energy;
pub mod covalidity;
pub mod desktop_stream;
pub mod wake_bridge;
pub mod lantern;
pub mod magic_object;
pub mod measured_ride;
pub mod mutation_translate;
pub mod neighborhood_arc;

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use universe_core::{EntityKey, Epistemic, Revision, UniverseError};
use universe_fields::{
    default_test_host, Materialization, ReadEvent, ReadField, SpaceResonance, TopologicalFold,
};
use universe_ir::{CodeDefinition, Value};
use universe_physics::{PhysicalState, Residency, UniversePhysics};
use universe_protocol::{CorrelationId, HeadlessProtocol, ReadEntityRequest, PROTOCOL_VERSION};
use universe_query::{
    graph_read, AdjacencyIndex, LocalRelation, LocalSituation, QueryBudget, QueryOrigin,
    QueryStatus,
};
use universe_store::{EntityRecord, UniverseSnapshot};
use universe_supervisor::{
    PhaseHook, RuntimeInventory, RuntimeMechanism, RuntimeMechanismKind, Supervisor, TickPhase,
};
use universe_transactions::{CommitReceipt, UniverseCommand, UniverseWriteSet};
use universe_vm::{ExecutionLimits, ExecutionReceipt, VmHost};

#[derive(Debug)]
pub enum E2eError {
    Universe(UniverseError),
    Vm(String),
    Io(String),
    Contract(String),
}

impl From<UniverseError> for E2eError {
    fn from(value: UniverseError) -> Self {
        Self::Universe(value)
    }
}

#[derive(Clone, Debug)]
pub struct RunConfig {
    pub genesis_path: PathBuf,
    pub code_path: PathBuf,
    pub store_root: PathBuf,
    pub artifact_root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerificationManifest {
    pub correlation: CorrelationId,
    pub genesis_path: PathBuf,
    pub code_path: PathBuf,
    pub starting_revision: Revision,
    pub committed_revision: Revision,
    pub read_status: QueryStatus,
    pub materialized_entities: Vec<EntityKey>,
    pub read_events: Vec<ReadEvent>,
    pub execution: ExecutionReceipt,
    pub commit_receipts: Vec<CommitReceipt>,
    pub independently_observed_moment: EntityRecord,
    pub independent_local_readback: LocalSituation,
    pub final_residency: Vec<(EntityKey, String)>,
    pub forbidden_runtime_invocations: ForbiddenRuntimeInvocations,
    pub runtime_executable: PathBuf,
    pub runtime_inventory: RuntimeInventory,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForbiddenRuntimeInvocations {
    pub python: u64,
    pub cypher: u64,
}

#[derive(Default)]
struct RecordingHook {
    phases: Vec<String>,
}

impl PhaseHook for RecordingHook {
    fn run(&mut self, phase: TickPhase, _snapshot: &UniverseSnapshot) -> Result<(), UniverseError> {
        self.phases.push(format!("{phase:?}"));
        Ok(())
    }
}

struct RealReadHost {
    graph: AdjacencyIndex,
    physics: UniversePhysics,
    snapshot: UniverseSnapshot,
    fold: Option<TopologicalFold>,
    situation: Option<LocalSituation>,
    events: Vec<ReadEvent>,
    query_policy: Option<EntityKey>,
    query_space: Option<EntityKey>,
    query_metric: Option<String>,
}

impl RealReadHost {
    fn new(snapshot: UniverseSnapshot) -> Result<Self, E2eError> {
        let graph = AdjacencyIndex::from_parts(
            snapshot.entities.iter().map(|entity| entity.key),
            snapshot.relations.iter().map(|relation| LocalRelation {
                key: relation.key,
                source: relation.source,
                target: relation.target,
            }),
        );
        Ok(Self {
            graph,
            physics: default_test_host(snapshot.entities.len().max(1)),
            snapshot,
            fold: None,
            situation: None,
            events: Vec::new(),
            query_policy: None,
            query_space: None,
            query_metric: None,
        })
    }

    fn release(&mut self) {
        if let Some(fold) = &mut self.fold {
            self.events.extend(fold.release(&mut self.physics));
        }
    }

    fn symbol_id(&self, name: &str) -> Result<u32, String> {
        self.snapshot
            .symbols
            .iter()
            .position(|symbol| symbol == name)
            .map(|index| index as u32)
            .ok_or_else(|| format!("Genesis symbol is absent: {name}"))
    }

    fn entity(&self, key: EntityKey) -> Result<&EntityRecord, String> {
        self.snapshot
            .entities
            .iter()
            .find(|entity| entity.key == key)
            .ok_or_else(|| format!("entity is absent: {key}"))
    }

    fn follow_key(&self, source: EntityKey, predicate: &str) -> Result<EntityKey, String> {
        let predicate = self.symbol_id(predicate)?;
        let mut targets = self
            .snapshot
            .relations
            .iter()
            .filter(|relation| relation.source == source && relation.predicate == predicate)
            .map(|relation| relation.target);
        let target = targets
            .next()
            .ok_or_else(|| "required graph relation is absent".to_owned())?;
        if targets.next().is_some() {
            return Err("expected exactly one graph relation".into());
        }
        Ok(target)
    }

    fn position(&self, entity: EntityKey) -> Result<[f32; 3], String> {
        // Position is a DERIVED projection of the topology, never a stored datum
        // (doctrine: "Where is a projection, not a datum"). We run the layout
        // authority over the snapshot and read back this entity's derived pose —
        // no `position_mm` coordinate is ever read, and none exists to read.
        use std::collections::{BTreeMap, BTreeSet};
        use universe_assets::layout::{self, LayoutParams, ProfileInput, RelationInput};
        let keys: Vec<EntityKey> = self.snapshot.entities.iter().map(|e| e.key).collect();
        let relations: Vec<RelationInput> = self
            .snapshot
            .relations
            .iter()
            .map(|relation| RelationInput {
                source: relation.source,
                target: relation.target,
                predicate: self
                    .snapshot
                    .symbols
                    .get(relation.predicate as usize)
                    .cloned()
                    .unwrap_or_default(),
            })
            .collect();
        let profiles: BTreeMap<String, ProfileInput> = BTreeMap::new();
        let containment: BTreeSet<String> = BTreeSet::from(["PART_OF".to_owned()]);
        let similarity = |_: EntityKey, _: EntityKey| 0.0;
        let input = layout::project(
            &keys,
            &relations,
            &profiles,
            &containment,
            &similarity,
            layout::DEFAULT_RADIUS,
            LayoutParams::default(),
        );
        let derived = layout::compute(&input).map_err(|error| format!("layout failed: {error:?}"))?;
        let position = derived
            .position(entity)
            .ok_or_else(|| "layout produced no position for entity".to_owned())?;
        Ok([position[0] as f32, position[1] as f32, position[2] as f32])
    }

    fn has_type(&self, entity: EntityKey, target_type: EntityKey) -> Result<bool, String> {
        let predicate = self.symbol_id("instance_of")?;
        Ok(self.snapshot.relations.iter().any(|relation| {
            relation.source == entity
                && relation.target == target_type
                && relation.predicate == predicate
        }))
    }
}

impl VmHost for RealReadHost {
    fn is_cancelled(&self) -> bool {
        false
    }

    fn capabilities(&self) -> BTreeSet<String> {
        BTreeSet::from(["local_query".to_owned()])
    }

    fn open_query(
        &mut self,
        spec: &universe_ir::QuerySpec,
        origin: &Value,
        selector: &Value,
    ) -> Result<Value, String> {
        let Value::Entity(origin) = origin else {
            return Err("query origin is not an entity".into());
        };
        let Value::Entity(policy) = selector else {
            return Err("query selector is not a QueryPolicy entity".into());
        };
        let policy_symbol = self.symbol_id("QueryPolicy")?;
        if self.entity(*policy)?.symbol != policy_symbol {
            return Err("query selector is not typed QueryPolicy".into());
        }
        let space = self.follow_key(*origin, "located_in")?;
        let scoring = self.follow_key(*policy, "scoring")?;
        let parameter_predicate = self.symbol_id("parameter")?;
        let metric = self
            .snapshot
            .relations
            .iter()
            .filter(|relation| {
                relation.source == scoring && relation.predicate == parameter_predicate
            })
            .filter_map(|relation| {
                let entity = self.entity(relation.target).ok()?;
                self.snapshot.symbols.get(entity.symbol as usize)
            })
            .find_map(|symbol| symbol.strip_prefix("score_field:"))
            .ok_or_else(|| "QueryPolicy scoring has no score_field parameter".to_owned())?
            .to_owned();
        self.query_policy = Some(*policy);
        self.query_space = Some(space);
        self.query_metric = Some(metric.clone());
        let materialization = self
            .snapshot
            .entities
            .iter()
            .filter_map(|entity| {
                self.position(entity.key)
                    .ok()
                    .map(|position| Materialization {
                        entity: entity.key,
                        generation: entity.generation,
                        state: PhysicalState {
                            position,
                            velocity: [0.0; 3],
                        },
                    })
            })
            .take(spec.budget.max_entities)
            .collect::<Vec<_>>();
        let space_position = self.position(space)?;
        let candidate_type =
            self.follow_key(self.follow_key(*policy, "selector")?, "required_type")?;
        let candidate = self
            .snapshot
            .entities
            .iter()
            .find(|entity| self.has_type(entity.key, candidate_type).unwrap_or(false))
            .ok_or_else(|| "selector found no typed candidate".to_owned())?;
        let candidate_position = self.position(candidate.key)?;
        let distance_mm = candidate_position
            .iter()
            .zip(space_position)
            .map(|(candidate, space)| ((*candidate - space) * 1000.0).powi(2))
            .sum::<f32>()
            .sqrt();
        let resonance = 1000.0 - f64::from(distance_mm);
        let field = ReadField {
            actor: *origin,
            origin: QueryOrigin::Entity(*origin),
            budget: spec.budget,
            materialization,
            resonances: vec![SpaceResonance {
                space,
                score: resonance,
                metric,
            }],
        };
        let (fold, situation, events) =
            TopologicalFold::open(field, &self.graph, &mut self.physics);
        self.fold = Some(fold);
        self.situation = Some(situation);
        self.events.extend(events);
        Ok(Value::Text("read-field-0".into()))
    }

    fn await_query(&mut self, _handle: &Value) -> Result<Value, String> {
        let situation = self
            .situation
            .as_ref()
            .ok_or_else(|| "query was not opened".to_owned())?;
        let policy = self
            .query_policy
            .ok_or_else(|| "query policy was not retained".to_owned())?;
        let selector = self.follow_key(policy, "selector")?;
        let required_type = self.follow_key(selector, "required_type")?;
        let eligible_parameter = self.symbol_id("eligible:true")?;
        let selector_is_eligible = self.snapshot.relations.iter().any(|relation| {
            relation.source == selector
                && relation.predicate == self.symbol_id("parameter").unwrap_or(u32::MAX)
                && self
                    .entity(relation.target)
                    .map(|entity| entity.symbol == eligible_parameter)
                    .unwrap_or(false)
        });
        let space = self
            .query_space
            .ok_or_else(|| "query Space was not retained".to_owned())?;
        let space_position = self.position(space)?;
        let metric = self
            .query_metric
            .clone()
            .ok_or_else(|| "query metric was not retained".to_owned())?;
        let mut measured = Vec::new();
        for entity in &situation.entities {
            if !self.has_type(*entity, required_type)? {
                continue;
            }
            let position = self.position(*entity)?;
            let distance_mm = position
                .iter()
                .zip(space_position)
                .map(|(candidate, space)| ((*candidate - space) * 1000.0).powi(2))
                .sum::<f32>()
                .sqrt();
            measured.push(Value::Record(BTreeMap::from([
                ("entity".into(), Value::Entity(*entity)),
                ("eligible".into(), Value::Bool(selector_is_eligible)),
                (
                    metric.clone(),
                    Value::Integer((1000.0 - distance_mm) as i64),
                ),
            ])));
        }
        Ok(Value::List(measured))
    }

    fn follow_one(&mut self, source: &Value, predicate: &Value) -> Result<Value, String> {
        let Value::Entity(source) = source else {
            return Err("follow_one source is not an entity".into());
        };
        let Value::Text(predicate) = predicate else {
            return Err("follow_one predicate is not text".into());
        };
        self.follow_key(*source, predicate).map(Value::Entity)
    }

    fn entity_symbol(&mut self, entity: &Value) -> Result<Value, String> {
        let Value::Entity(entity) = entity else {
            return Err("entity_symbol input is not an entity".into());
        };
        Ok(Value::Integer(i64::from(self.entity(*entity)?.symbol)))
    }

    fn hydrate(&mut self, selected: &[Value], _max_bytes: u32) -> Result<Vec<Value>, String> {
        Ok(selected.to_vec())
    }
}

/// Fixture-only adapter. It translates a graph-produced proposal to the one
/// generic entity command supported by transaction v0; production policy does
/// not live here.
fn translate_fixture_proposal(
    receipt: &ExecutionReceipt,
    snapshot: &UniverseSnapshot,
    correlation: &CorrelationId,
) -> Result<Option<UniverseWriteSet>, UniverseError> {
    if receipt.proposals.len() != 1 {
        return Err(UniverseError::Validation(
            "vertical slice requires exactly one graph proposal".into(),
        ));
    }
    let Value::Record(command) = &receipt.proposals[0].command else {
        return Err(UniverseError::Validation(
            "graph proposal command must be a record".into(),
        ));
    };
    if command.get("command") != Some(&Value::Text("put_entity".into())) {
        return Err(UniverseError::Validation(
            "unsupported graph command".into(),
        ));
    }
    let Some(Value::Entity(key)) = command.get("entity") else {
        return Err(UniverseError::Validation(
            "put_entity requires an entity key".into(),
        ));
    };
    let Some(Value::Integer(generation)) = command.get("generation") else {
        return Err(UniverseError::Validation(
            "put_entity requires a generation".into(),
        ));
    };
    let Some(Value::Integer(symbol)) = command.get("symbol") else {
        return Err(UniverseError::Validation(
            "put_entity requires a symbol".into(),
        ));
    };
    let Some(Value::Entity(result_type)) = command.get("result_type") else {
        return Err(UniverseError::Validation(
            "put_entity requires its graph result type".into(),
        ));
    };
    let result_type_record = snapshot
        .entities
        .iter()
        .find(|entity| entity.key == *result_type)
        .ok_or_else(|| UniverseError::Validation("graph result type is absent".into()))?;
    if i64::from(result_type_record.symbol) != *symbol {
        return Err(UniverseError::Validation(
            "command symbol does not match graph result type".into(),
        ));
    }
    if !matches!(command.get("result"), Some(Value::List(_)))
        || command.get("content") != Some(&Value::Unit)
    {
        return Err(UniverseError::Validation(
            "put_entity result/content contract is invalid".into(),
        ));
    }
    let generation = u32::try_from(*generation)
        .map_err(|_| UniverseError::Validation("generation is outside u32".into()))?;
    let symbol = u32::try_from(*symbol)
        .map_err(|_| UniverseError::Validation("symbol is outside u32".into()))?;
    Ok(Some(UniverseWriteSet {
        base_revision: snapshot.revision,
        idempotency_key: format!("{}:put_entity", correlation.0),
        causal_ancestry: vec![correlation.0.clone(), receipt.code_hash.clone()],
        commands: vec![UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: *key,
                generation,
                symbol,
                content: None,
            },
        }],
    }))
}

pub fn run(config: &RunConfig) -> Result<VerificationManifest, E2eError> {
    let correlation = unique_correlation();
    let mut supervisor = Supervisor::boot(&config.store_root, &config.genesis_path)?;
    let starting_revision = supervisor.revision();
    let code: CodeDefinition = serde_json::from_slice(
        &fs::read(&config.code_path).map_err(|error| E2eError::Io(error.to_string()))?,
    )
    .map_err(|error| E2eError::Contract(error.to_string()))?;
    let actor = find_instance_of(supervisor.snapshot(), "Actor")?;
    let moment = next_entity_key(supervisor.snapshot())?;
    let mut host = RealReadHost::new(supervisor.snapshot().clone())?;
    let inputs = BTreeMap::from([
        ("actor".into(), Value::Entity(actor)),
        ("result_entity".into(), Value::Entity(moment)),
    ]);
    let execution = supervisor
        .execute_graph_program(
            &code,
            &mut host,
            &inputs,
            ExecutionLimits {
                fuel: 64,
                max_proposals: 1,
            },
            |receipt, snapshot| translate_fixture_proposal(receipt, snapshot, &correlation),
        )
        .map_err(|error| E2eError::Vm(format!("{error:?}")))?;
    let situation = host
        .situation
        .clone()
        .ok_or_else(|| E2eError::Contract("Graph IR did not execute graph_read".into()))?;
    // A resonance MUST be measured for the Space with the graph-owned metric.
    // Its exact score is a DERIVED projection of the layout (positions are never
    // stored), so we assert the measurement happened, not a pinned coordinate
    // value — the number may drift and we do not force it to be deterministic.
    if !host.events.iter().any(|event| {
        matches!(
            event,
            ReadEvent::SpaceResonanceMeasured { metric, .. } if metric == "resonance"
        )
    }) {
        return Err(E2eError::Contract(
            "graph-owned Space resonance was not measured".into(),
        ));
    }
    if !host.events.iter().any(|event| {
        matches!(
            event,
            ReadEvent::ReadStabilized {
                state: Epistemic::Measured(_),
                ..
            }
        )
    }) {
        return Err(E2eError::Contract(
            "ReadField did not report measured stabilization".into(),
        ));
    }
    let materialized_entities = host.physics.active_entities();
    if materialized_entities.is_empty() {
        return Err(E2eError::Contract(
            "graph_read produced no physical materialization".into(),
        ));
    }
    let mut hook = RecordingHook::default();
    let commit_receipts = supervisor.advance(&mut hook)?;
    if commit_receipts.is_empty() {
        return Err(E2eError::Contract("Moment was not committed".into()));
    }
    let protocol = HeadlessProtocol::new(&supervisor);
    let response = protocol.read_entity(ReadEntityRequest {
        protocol_version: PROTOCOL_VERSION,
        correlation: correlation.clone(),
        key: moment,
        max_entities: 1,
    })?;
    let runtime_inventory = protocol.runtime_inventory();
    let expected_inventory = RuntimeInventory {
        mechanisms: vec![RuntimeMechanism {
            kind: RuntimeMechanismKind::Executor,
            name: "universe-vm".into(),
            activations: 1,
        }],
    };
    if runtime_inventory != expected_inventory {
        return Err(E2eError::Contract(format!(
            "runtime inventory differs from exact allowlist: {runtime_inventory:?}"
        )));
    }
    let Epistemic::Observed(independently_observed_moment) = response.result else {
        return Err(E2eError::Contract(
            "fresh protocol readback did not observe Moment".into(),
        ));
    };
    let independently_replayed = supervisor.independent_readback()?;
    let independent_index = AdjacencyIndex::from_parts(
        independently_replayed
            .entities
            .iter()
            .map(|entity| entity.key),
        independently_replayed
            .relations
            .iter()
            .map(|relation| LocalRelation {
                key: relation.key,
                source: relation.source,
                target: relation.target,
            }),
    );
    let independent_local_readback = graph_read(
        &independent_index,
        QueryOrigin::Entity(moment),
        QueryBudget {
            max_entities: 1,
            max_relations: 1,
            max_depth: 1,
        },
    );
    if independent_local_readback.entities != vec![moment] {
        return Err(E2eError::Contract(
            "new local query did not read committed Moment".into(),
        ));
    }
    host.release();
    let final_residency = materialized_entities
        .iter()
        .map(|entity| {
            let state = match host.physics.residency(*entity) {
                Residency::Dormant => "dormant",
                Residency::Hot => "hot",
            };
            (*entity, state.to_owned())
        })
        .collect::<Vec<_>>();
    if final_residency.iter().any(|(_, state)| state != "dormant") {
        return Err(E2eError::Contract("fold did not fully release".into()));
    }
    let manifest = VerificationManifest {
        correlation: correlation.clone(),
        genesis_path: config.genesis_path.clone(),
        code_path: config.code_path.clone(),
        starting_revision,
        committed_revision: response.revision,
        read_status: situation.status,
        materialized_entities,
        read_events: host.events,
        execution,
        commit_receipts,
        independently_observed_moment,
        independent_local_readback,
        final_residency,
        forbidden_runtime_invocations: ForbiddenRuntimeInvocations::default(),
        runtime_executable: std::env::current_exe()
            .map_err(|error| E2eError::Io(error.to_string()))?,
        runtime_inventory,
    };
    write_artifacts(&config.artifact_root, &manifest, &hook.phases)?;
    Ok(manifest)
}

fn next_entity_key(snapshot: &UniverseSnapshot) -> Result<EntityKey, E2eError> {
    snapshot
        .entities
        .iter()
        .map(|entity| entity.key.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(EntityKey)
        .ok_or_else(|| E2eError::Contract("entity key space exhausted".into()))
}

fn find_instance_of(snapshot: &UniverseSnapshot, type_name: &str) -> Result<EntityKey, E2eError> {
    let instance_of = snapshot
        .symbols
        .iter()
        .position(|symbol| symbol == "instance_of")
        .ok_or_else(|| E2eError::Contract("Genesis lacks instance_of".into()))?
        as u32;
    let type_symbol = snapshot
        .symbols
        .iter()
        .position(|symbol| symbol == type_name)
        .ok_or_else(|| E2eError::Contract(format!("Genesis lacks type {type_name}")))?
        as u32;
    let type_keys = snapshot
        .entities
        .iter()
        .filter(|entity| entity.symbol == type_symbol)
        .map(|entity| entity.key)
        .collect::<BTreeSet<_>>();
    let mut instances = snapshot
        .relations
        .iter()
        .filter(|relation| {
            relation.predicate == instance_of && type_keys.contains(&relation.target)
        })
        .map(|relation| relation.source);
    let instance = instances
        .next()
        .ok_or_else(|| E2eError::Contract(format!("Genesis has no {type_name} instance")))?;
    if instances.next().is_some() {
        return Err(E2eError::Contract(format!(
            "Genesis has ambiguous {type_name} instances"
        )));
    }
    Ok(instance)
}

fn unique_correlation() -> CorrelationId {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    CorrelationId(format!("e2e-{}-{nanos}", std::process::id()))
}

fn write_artifacts(
    root: &Path,
    manifest: &VerificationManifest,
    phases: &[String],
) -> Result<(), E2eError> {
    let run_root = root.join(&manifest.correlation.0);
    fs::create_dir_all(&run_root).map_err(|error| E2eError::Io(error.to_string()))?;
    let manifest_bytes =
        serde_json::to_vec_pretty(manifest).map_err(|error| E2eError::Io(error.to_string()))?;
    fs::write(run_root.join("manifest.json"), manifest_bytes)
        .map_err(|error| E2eError::Io(error.to_string()))?;
    let trace = manifest
        .execution
        .trace
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| E2eError::Io(error.to_string()))?
        .join("\n");
    fs::write(run_root.join("vm-trace.jsonl"), format!("{trace}\n"))
        .map_err(|error| E2eError::Io(error.to_string()))?;
    fs::write(
        run_root.join("phases.json"),
        serde_json::to_vec_pretty(phases).map_err(|error| E2eError::Io(error.to_string()))?,
    )
    .map_err(|error| E2eError::Io(error.to_string()))?;
    fs::write(
        run_root.join("runtime-inventory.json"),
        serde_json::to_vec_pretty(&manifest.runtime_inventory)
            .map_err(|error| E2eError::Io(error.to_string()))?,
    )
    .map_err(|error| E2eError::Io(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_vertical_slice_has_real_readback_and_release() {
        let root = tempfile::tempdir().unwrap();
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest = run(&RunConfig {
            genesis_path: repository.join("fixtures/genesis/minimal-genesis.json"),
            code_path: repository.join("fixtures/graph-ir/minimal-read.json"),
            store_root: root.path().join("store"),
            artifact_root: root.path().join("artifacts"),
        })
        .unwrap();
        assert_eq!(manifest.committed_revision, Revision(1));
        assert_eq!(
            manifest.forbidden_runtime_invocations,
            ForbiddenRuntimeInvocations::default()
        );
        assert!(manifest
            .read_events
            .iter()
            .any(|event| matches!(event, ReadEvent::ReadReleased { .. })));
        assert!(manifest
            .final_residency
            .iter()
            .all(|(_, state)| state == "dormant"));
        assert!(root
            .path()
            .join("artifacts")
            .join(&manifest.correlation.0)
            .join("manifest.json")
            .is_file());
    }
}
