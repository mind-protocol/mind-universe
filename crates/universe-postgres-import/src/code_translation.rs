//! G2 phase 4 — real Graph-IR translation, shadow execution, and comparison.
//!
//! The classification pilot leaves declarative candidates `translated_inert`.
//! This module takes the single reconciliation candidate one honest step
//! further: it emits a **real** `CodeDefinition` (Graph IR), stores it as graph
//! data, reads it back, `compile`s it, and **shadow-executes** it on the real
//! fuel-bounded VM. The VM host boundary is mutation-free — execution yields
//! `WriteProposal`s that are never applied — so the shadow run causes no effect.
//!
//! The run is compared against the migration's declared expected contract and
//! executed twice to measure determinism. Equivalence advances the state to
//! `independently_compared`; non-equivalence is preserved as evidence. Neither
//! outcome activates the code: activation still requires an approved ChangeSet
//! and is out of scope here. Nothing external runs; no proposal is committed.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::{collections::BTreeSet, path::Path};
use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseError, UniverseId};
use universe_ir::{CodeDefinition, ComparisonKind, Operator, Value as IrValue, IR_VERSION};
use universe_store::{
    EntityRecord, GraphSeed, RelationRecord, SeedEntity, SeedRelation, UniverseSnapshot,
    UniverseStore,
};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};
use universe_vm::{execute_program, ExecutionLimits, VmError, VmHost};

const SYM_SOURCE: &str = "postgres_import_source";
const SYM_DEFINITION: &str = "candidate_code_definition";
const SYM_BATCH: &str = "shadow_execution_batch";
const SYM_RECEIPT: &str = "import_receipt";
const SYM_GOVERNED_BY: &str = "GOVERNED_BY";
const SYM_HAS_RECEIPT: &str = "HAS_RECEIPT";
const CODE_DEFINITION_REVISION: u64 = 1;

/// A pure host: no capabilities, no queries, never cancelled. A CodeDefinition
/// that touches the graph or a capability fails deterministically here instead
/// of silently reaching outside the shadow boundary.
struct PureHost;

impl VmHost for PureHost {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn capabilities(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn open_query(
        &mut self,
        _: &universe_ir::QuerySpec,
        _: &IrValue,
        _: &IrValue,
    ) -> Result<IrValue, String> {
        Err("shadow host performs no query".into())
    }
    fn await_query(&mut self, _: &IrValue) -> Result<IrValue, String> {
        Err("shadow host performs no query".into())
    }
    fn follow_one(&mut self, _: &IrValue, _: &IrValue) -> Result<IrValue, String> {
        Err("shadow host follows no relation".into())
    }
    fn entity_symbol(&mut self, _: &IrValue) -> Result<IrValue, String> {
        Err("shadow host resolves no symbol".into())
    }
    fn hydrate(&mut self, _: &[IrValue], _: u32) -> Result<Vec<IrValue>, String> {
        Err("shadow host hydrates nothing".into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TranslationSource {
    pub atom: EntityKey,
    pub authority_id: String,
    pub source_id: String,
    pub row_sha256: String,
    pub source_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ShadowBatch {
    pub atom: EntityKey,
    pub batch_id: String,
    pub receipt_atom: EntityKey,
    pub receipt_relation: RelationKey,
    pub relation_key_start: RelationKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TranslationInputs {
    pub l1_state: i64,
    pub blueprint_state: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExpectedContract {
    pub reconciled: bool,
    pub proposal_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TranslationManifest {
    pub contract_version: u16,
    pub universe: UniverseId,
    pub source: TranslationSource,
    pub definition_atom: EntityKey,
    pub batch: ShadowBatch,
    pub inputs: TranslationInputs,
    pub expected: ExpectedContract,
    pub fuel: u64,
    pub max_proposals: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TranslationEvidence {
    pub batch_id: String,
    pub universe: UniverseId,
    pub code_hash: String,
    pub compiled: bool,
    pub shadow_executed: bool,
    pub deterministic: bool,
    pub equivalent: bool,
    pub external_effects: bool,
    pub activated: bool,
    pub fuel_used: u64,
    pub proposal_count: usize,
    pub trace_len: usize,
    pub state_reached: String,
    pub pre_receipt_snapshot_hash: String,
    pub final_snapshot_hash: String,
    pub final_revision: Revision,
    pub final_tick: Tick,
    pub receipt_atom: EntityKey,
}

pub fn load_manifest(path: impl AsRef<Path>) -> Result<TranslationManifest, UniverseError> {
    let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| UniverseError::CorruptContent(error.to_string()))
}

/// The translator output: a real, pure reconciliation CodeDefinition. It reads
/// two evidence integers, decides whether they are reconciled (equal), proposes
/// an inert reconciliation record, and returns the decision. It requires no
/// capability and performs no query, so it is safe to shadow-execute.
pub fn translated_code_definition() -> CodeDefinition {
    CodeDefinition {
        ir_version: IR_VERSION,
        revision: Revision(CODE_DEFINITION_REVISION),
        required_capabilities: vec![],
        operators: vec![
            Operator::Input {
                name: "l1_state".into(),
                output: 0,
            },
            Operator::Input {
                name: "blueprint_state".into(),
                output: 1,
            },
            Operator::Compare {
                left: 0,
                right: 1,
                kind: ComparisonKind::Equal,
                output: 2,
            },
            Operator::MakeRecord {
                fields: vec![("reconciled".into(), 2)],
                output: 3,
            },
            Operator::Propose {
                command: 3,
                output: 4,
            },
            Operator::Return { value: 2 },
        ],
    }
}

fn materialize_seed(
    manifest: &TranslationManifest,
    code: &CodeDefinition,
) -> Result<GraphSeed, UniverseError> {
    let ir = serde_json::to_value(code)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    let symbols = vec![
        SYM_SOURCE.to_owned(),
        SYM_DEFINITION.to_owned(),
        SYM_BATCH.to_owned(),
        SYM_RECEIPT.to_owned(),
        SYM_GOVERNED_BY.to_owned(),
        SYM_HAS_RECEIPT.to_owned(),
    ];
    let entities = vec![
        entity(
            manifest.source.atom,
            SYM_SOURCE,
            json!({
                "kind": "postgres_import_source",
                "authority_id": manifest.source.authority_id,
                "source_id": manifest.source.source_id,
                "row_sha256": manifest.source.row_sha256,
                "source_revision": manifest.source.source_revision,
                "read_only": true,
            }),
        ),
        // The translated IR is graph data: stored as content, executable only
        // through the VM after readback, never marked activated.
        entity(
            manifest.definition_atom,
            SYM_DEFINITION,
            json!({
                "kind": "candidate_code_definition",
                "state": "translated_inert",
                "source_id": manifest.source.source_id,
                "row_sha256": manifest.source.row_sha256,
                "source_revision": manifest.source.source_revision,
                "code_definition_revision": CODE_DEFINITION_REVISION,
                "ir": ir,
                "executable": false,
                "activated": false,
                "compiled": false,
                "shadow_executed": false,
            }),
        ),
        entity(
            manifest.batch.atom,
            SYM_BATCH,
            json!({
                "kind": "shadow_execution_batch",
                "batch_id": manifest.batch.batch_id,
                "status": "prepared",
            }),
        ),
    ];
    let mut next = manifest.batch.relation_key_start.0;
    let relations = vec![
        relation(
            &mut next,
            manifest.definition_atom,
            manifest.source.atom,
            SYM_GOVERNED_BY,
        ),
        relation(
            &mut next,
            manifest.batch.atom,
            manifest.source.atom,
            SYM_GOVERNED_BY,
        ),
    ];
    Ok(GraphSeed {
        universe: manifest.universe,
        symbols,
        entities,
        relations,
    })
}

pub fn run_translation(
    manifest: &TranslationManifest,
    output: impl AsRef<Path>,
) -> Result<TranslationEvidence, UniverseError> {
    if manifest.contract_version != 0 {
        return Err(UniverseError::UnsupportedVersion(manifest.contract_version));
    }
    let code = translated_code_definition();
    let code_hash =
        universe_compiler::canonical_hash(&code).map_err(|error| compile_error("hash", error))?;

    let store_root = output.as_ref();
    let store = UniverseStore::open(store_root)?;
    let installed = if store_root.join("snapshot.json").exists() {
        store.replay(store.load_snapshot()?)?
    } else {
        store.install_seed(&materialize_seed(manifest, &code)?)?
    };
    let pre_receipt_snapshot_hash = installed.canonical_hash()?;

    let independent_store = UniverseStore::open(store_root)?;
    let mut independent = independent_store.replay(independent_store.load_snapshot()?)?;

    // Read the IR back from the store — the executed program is the graph-owned
    // one, not the in-memory translator output.
    let definition = independent
        .entities
        .iter()
        .find(|entity| entity.key == manifest.definition_atom)
        .and_then(|entity| entity.content.as_ref())
        .ok_or_else(|| validation("candidate CodeDefinition Atom is missing"))?;
    let stored_ir = independent_store
        .read_content(definition)?
        .get("ir")
        .cloned()
        .ok_or_else(|| validation("candidate CodeDefinition Atom has no IR"))?;
    let code_from_graph: CodeDefinition = serde_json::from_value(stored_ir)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    let readback_hash = universe_compiler::canonical_hash(&code_from_graph)
        .map_err(|error| compile_error("hash", error))?;
    if readback_hash != code_hash {
        return Err(UniverseError::CorruptContent(
            "graph-stored IR does not match the translated CodeDefinition".into(),
        ));
    }

    // Real compilation and two shadow executions on the fuel-bounded VM.
    universe_compiler::validate(&code_from_graph)
        .map_err(|error| compile_error("validate", error))?;
    let inputs = std::collections::BTreeMap::from([
        (
            "l1_state".to_owned(),
            IrValue::Integer(manifest.inputs.l1_state),
        ),
        (
            "blueprint_state".to_owned(),
            IrValue::Integer(manifest.inputs.blueprint_state),
        ),
    ]);
    let first = shadow_execute(
        &code_from_graph,
        &inputs,
        manifest.fuel,
        manifest.max_proposals,
    )?;
    let second = shadow_execute(
        &code_from_graph,
        &inputs,
        manifest.fuel,
        manifest.max_proposals,
    )?;
    let deterministic = first == second;

    let expected_result = IrValue::Bool(manifest.expected.reconciled);
    let equivalent = deterministic
        && first.result == expected_result
        && first.proposals.len() == manifest.expected.proposal_count;
    let state_reached = if equivalent {
        "independently_compared"
    } else {
        "non_equivalent_evidence"
    };

    let receipt_content = json!({
        "kind": "adaptation_receipt",
        "batch_id": manifest.batch.batch_id,
        "status": "measured_shadow_execution",
        "information_status": "measured",
        "code_hash": code_hash,
        "code_definition_revision": CODE_DEFINITION_REVISION,
        "compiled": true,
        "shadow_executed": true,
        "deterministic": deterministic,
        "external_effects": false,
        "activated": false,
        "result": serde_json::to_value(&first.result)
            .map_err(|error| UniverseError::CorruptContent(error.to_string()))?,
        "proposal_count": first.proposals.len(),
        "fuel_used": first.fuel_used,
        "trace_len": first.trace.len(),
        "expected": {
            "reconciled": manifest.expected.reconciled,
            "proposal_count": manifest.expected.proposal_count,
        },
        "equivalent": equivalent,
        "state_reached": state_reached,
        "downstream_not_measured": ["approved_changeset", "activated_for_later_execution"],
    });

    let activate_key = format!("{}:shadow", manifest.batch.batch_id);
    if !independent.event_keys.contains(&activate_key) {
        let receipt_symbol = symbol(&independent, SYM_RECEIPT)?;
        let has_receipt = symbol(&independent, SYM_HAS_RECEIPT)?;
        let receipt_ref = independent_store.append_content(&receipt_content)?;
        let commands = vec![
            UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key: manifest.batch.receipt_atom,
                    generation: 0,
                    symbol: receipt_symbol,
                    content: Some(receipt_ref),
                },
            },
            UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: manifest.batch.receipt_relation,
                    generation: 0,
                    source: manifest.batch.atom,
                    target: manifest.batch.receipt_atom,
                    predicate: has_receipt,
                    content: Some(independent_store.append_content(&json!({
                        "kind": "import_relation",
                        "justification": "Shadow execution on the fuel-bounded VM produced this measured, non-activating comparison receipt. No proposal was applied."
                    }))?),
                },
            },
        ];
        let transaction = UniverseTransaction::prepare(
            &independent,
            UniverseWriteSet {
                base_revision: independent.revision,
                idempotency_key: activate_key,
                commands,
            },
        )?;
        let tick = Tick(independent.tick.0 + 1);
        transaction.commit(&independent_store, &mut independent, tick)?;
    }

    // Independent replay + verification: the receipt reads back, and no VM
    // proposal became a real universe entity (the shadow boundary held).
    let final_store = UniverseStore::open(store_root)?;
    let final_snapshot = final_store.replay(final_store.load_snapshot()?)?;
    let receipt_entity = final_snapshot
        .entities
        .iter()
        .find(|entity| entity.key == manifest.batch.receipt_atom)
        .and_then(|entity| entity.content.as_ref())
        .ok_or_else(|| validation("shadow execution receipt is missing after replay"))?;
    if final_store.read_content(receipt_entity)? != receipt_content {
        return Err(UniverseError::CorruptContent(
            "shadow execution receipt differs after replay".into(),
        ));
    }
    // Only source, definition, batch, and the shadow receipt exist — the write
    // proposal (which would create a reconciliation entity) was never applied.
    let mut activated = false;
    for entity in &final_snapshot.entities {
        if let Some(content_ref) = entity.content.as_ref() {
            let content = final_store.read_content(content_ref)?;
            if content.get("activated") == Some(&JsonValue::Bool(true)) {
                activated = true;
            }
        }
    }
    if activated {
        return Err(UniverseError::CorruptContent(
            "shadow execution activated code".into(),
        ));
    }

    Ok(TranslationEvidence {
        batch_id: manifest.batch.batch_id.clone(),
        universe: final_snapshot.universe,
        code_hash,
        compiled: true,
        shadow_executed: true,
        deterministic,
        equivalent,
        external_effects: false,
        activated: false,
        fuel_used: first.fuel_used,
        proposal_count: first.proposals.len(),
        trace_len: first.trace.len(),
        state_reached: state_reached.to_owned(),
        pre_receipt_snapshot_hash,
        final_snapshot_hash: final_snapshot.canonical_hash()?,
        final_revision: final_snapshot.revision,
        final_tick: final_snapshot.tick,
        receipt_atom: manifest.batch.receipt_atom,
    })
}

fn shadow_execute(
    code: &CodeDefinition,
    inputs: &std::collections::BTreeMap<String, IrValue>,
    fuel: u64,
    max_proposals: u32,
) -> Result<universe_vm::ExecutionReceipt, UniverseError> {
    let mut host = PureHost;
    let limits = ExecutionLimits {
        fuel,
        max_proposals,
    };
    execute_program(code, &mut host, inputs, Revision(0), Tick(0), limits)
        .map_err(|error: VmError| validation(format!("shadow execution failed: {error}")))
}

fn entity(key: EntityKey, symbol: &str, content: JsonValue) -> SeedEntity {
    SeedEntity {
        key,
        generation: 0,
        symbol: symbol.to_owned(),
        content,
    }
}

fn relation(
    next: &mut u128,
    source: EntityKey,
    target: EntityKey,
    predicate: &str,
) -> SeedRelation {
    let result = SeedRelation {
        key: RelationKey(*next),
        generation: 0,
        source,
        target,
        predicate: predicate.to_owned(),
        content: None,
    };
    *next += 1;
    result
}

fn symbol(snapshot: &UniverseSnapshot, value: &str) -> Result<u32, UniverseError> {
    snapshot
        .symbol_id(value)
        .ok_or_else(|| validation(format!("translation symbol {value} is not interned")))
}

fn compile_error(stage: &str, error: universe_compiler::CompileError) -> UniverseError {
    UniverseError::Validation(format!("code {stage} failed: {error:?}"))
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> TranslationManifest {
        serde_json::from_str(include_str!(
            "../../../fixtures/import/postgres-code-translation-pilot.json"
        ))
        .unwrap()
    }

    #[test]
    fn real_ir_compiles_shadow_executes_and_matches_contract() {
        let manifest = manifest();
        let temp = tempfile::tempdir().unwrap();
        let evidence = run_translation(&manifest, temp.path()).unwrap();

        assert!(evidence.compiled);
        assert!(evidence.shadow_executed);
        assert!(evidence.deterministic);
        assert!(evidence.equivalent);
        assert!(!evidence.external_effects);
        assert!(!evidence.activated);
        assert_eq!(evidence.proposal_count, 1);
        assert!(evidence.fuel_used > 0);
        assert_eq!(evidence.code_hash.len(), 64);
        assert_eq!(evidence.state_reached, "independently_compared");
    }

    #[test]
    fn rerun_is_idempotent() {
        let manifest = manifest();
        let temp = tempfile::tempdir().unwrap();
        let first = run_translation(&manifest, temp.path()).unwrap();
        let second = run_translation(&manifest, temp.path()).unwrap();
        assert_eq!(first.final_snapshot_hash, second.final_snapshot_hash);
        assert_eq!(first.final_revision, second.final_revision);
    }

    #[test]
    fn non_equivalent_contract_is_preserved_not_activated() {
        // Declaring the wrong expected outcome must surface as measured
        // non-equivalence, never as activation.
        let mut manifest = manifest();
        manifest.inputs.blueprint_state = manifest.inputs.l1_state + 1; // now not reconciled
        let temp = tempfile::tempdir().unwrap();
        let evidence = run_translation(&manifest, temp.path()).unwrap();
        assert!(!evidence.equivalent);
        assert_eq!(evidence.state_reached, "non_equivalent_evidence");
        assert!(!evidence.activated);
    }

    #[test]
    fn translated_definition_is_a_valid_program() {
        let code = translated_code_definition();
        assert!(universe_compiler::validate(&code).is_ok());
        assert!(universe_compiler::compile(&code).is_ok());
    }
}
