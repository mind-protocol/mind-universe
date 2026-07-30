//! G2 phase 4 — bounded code-migration classification pilot.
//!
//! Every imported code-bearing Node first becomes an **inert** `LegacyCodeAsset`
//! plus a `CodeMigrationTask`. This module classifies each into exactly one
//! target strategy, applies the safety gate, and records the activation state
//! machine — but it deliberately stops early and stays honest:
//!
//! - it **never executes** imported Python, Cypher, SQL policy, shell, URI, or
//!   generated files, and never imports the code payload;
//! - it performs **no** compilation or shadow execution, so every stage past
//!   `code_classified`/`translated` is reported as `not_measured`, never faked;
//! - nothing is made executable, activated, or wired to a lease/trigger; and
//! - the **engine** computes accept/reject/quarantine from declared safety
//!   facts — the manifest cannot assert its own acceptance.
//!
//! Activation of migrated code is out of scope: it requires an approved
//! ChangeSet and real shadow-execution evidence that this pilot does not produce.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};
use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseError, UniverseId};
use universe_store::{
    EntityRecord, GraphSeed, RelationRecord, SeedEntity, SeedRelation, UniverseSnapshot,
    UniverseStore,
};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

const SOURCE_FORMS: [&str; 8] = [
    "declarative_graph_ir",
    "python",
    "cypher",
    "postgres_function",
    "external_transport",
    "generated_file",
    "historical_trace",
    "obsolete",
];

/// The stages downstream of `translated` that this pilot never reaches. They are
/// reported as `not_measured` so no run implies compilation, execution, or
/// activation that did not happen.
const UNMEASURED_STAGES: [&str; 6] = [
    "validated",
    "compiled",
    "shadow_executed",
    "independently_compared",
    "approved_changeset",
    "activated_for_later_execution",
];

const SYM_SOURCE: &str = "postgres_import_source";
const SYM_BATCH: &str = "code_migration_batch";
const SYM_LEGACY: &str = "legacy_code_asset";
const SYM_TASK: &str = "code_migration_task";
const SYM_CANDIDATE_DEF: &str = "candidate_code_definition";
const SYM_RECEIPT: &str = "import_receipt";
const SYM_GOVERNED_BY: &str = "GOVERNED_BY";
const SYM_DESCRIBES: &str = "DESCRIBES";
const SYM_IMPORTS_FROM: &str = "IMPORTS_FROM";
const SYM_TRANSLATES_TO: &str = "TRANSLATES_TO";
const SYM_IN_BATCH: &str = "IN_BATCH";
const SYM_HAS_RECEIPT: &str = "HAS_RECEIPT";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PilotSource {
    pub atom: EntityKey,
    pub authority_id: String,
    pub source_graph_scope: Vec<String>,
    pub observed_at: String,
    pub ontology_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MigrationBatch {
    pub atom: EntityKey,
    pub batch_id: String,
    pub receipt_atom: EntityKey,
    pub receipt_relation: RelationKey,
    pub relation_key_start: RelationKey,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeclaredCode {
    #[serde(default)]
    pub effects: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub has_timeout: bool,
    #[serde(default)]
    pub has_cancellation: bool,
    #[serde(default)]
    pub bounded_loops: bool,
    #[serde(default)]
    pub budget_bounded: bool,
    #[serde(default)]
    pub hidden_predicate_dispatch: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeCandidate {
    pub legacy_asset_atom: EntityKey,
    pub task_atom: EntityKey,
    #[serde(default)]
    pub definition_atom: Option<EntityKey>,
    pub source_label: String,
    pub graph_id: String,
    pub source_id: String,
    pub source_form: String,
    pub row_sha256: String,
    pub source_revision: u64,
    #[serde(default)]
    pub code_definition_revision: Option<u64>,
    pub declared: DeclaredCode,
    pub justification: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeMigrationManifest {
    pub contract_version: u16,
    pub universe: UniverseId,
    pub source: PilotSource,
    pub batch: MigrationBatch,
    pub candidates: Vec<CodeCandidate>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeMigrationEvidence {
    pub batch_id: String,
    pub universe: UniverseId,
    pub total_candidates: usize,
    pub accepted_inert: usize,
    pub rejected: usize,
    pub quarantined: usize,
    pub translated_candidates: usize,
    pub migration_required: usize,
    /// Must all be zero — this pilot compiles, executes, and activates nothing.
    pub executable_count: usize,
    pub activated_count: usize,
    pub shadow_executed_count: usize,
    pub pre_receipt_snapshot_hash: String,
    pub final_snapshot_hash: String,
    pub final_revision: Revision,
    pub final_tick: Tick,
    pub content_records_read_back: usize,
    pub receipt_atom: EntityKey,
}

pub fn load_manifest(path: impl AsRef<Path>) -> Result<CodeMigrationManifest, UniverseError> {
    let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| UniverseError::CorruptContent(error.to_string()))
}

/// Target migration strategy required by the adaptation matrix for a source form.
fn expected_strategy(form: &str) -> &'static str {
    match form {
        "declarative_graph_ir" => "translate_to_code_definition",
        "python" => "extract_and_rewrite",
        "cypher" => "translate_bounded_query",
        "postgres_function" => "separate_policy",
        "external_transport" => "capability_intent",
        "generated_file" => "content_asset",
        "historical_trace" => "observation_evidence",
        "obsolete" => "preserve_and_quarantine",
        _ => "unknown",
    }
}

/// Default imported state required by the adaptation matrix for a source form.
fn expected_state(form: &str) -> &'static str {
    match form {
        "declarative_graph_ir" => "translated_inert",
        "python" | "cypher" => "migration_required",
        "postgres_function" => "quarantined_policy_split",
        "external_transport" => "capability_disabled",
        "generated_file" => "inert_asset",
        "historical_trace" => "non_executable_evidence",
        "obsolete" => "quarantined",
        _ => "unknown",
    }
}

/// The safety gate. Postgres functions and obsolete code are always quarantined;
/// content/evidence forms are inert-safe; translatable/executable forms must pass
/// declared bounds, timeout, cancellation, capability, and dispatch checks or are
/// rejected. Computed by the engine, never taken from the manifest.
fn gate(form: &str, declared: &DeclaredCode) -> &'static str {
    match form {
        "postgres_function" | "obsolete" => "quarantined",
        "generated_file" | "historical_trace" => "accepted_inert",
        "declarative_graph_ir" | "python" | "cypher" | "external_transport" => {
            let effects_uncapable =
                !declared.effects.is_empty() && declared.capabilities.is_empty();
            let transport_missing_capability =
                form == "external_transport" && declared.capabilities.is_empty();
            if effects_uncapable
                || transport_missing_capability
                || !declared.bounded_loops
                || !declared.has_timeout
                || !declared.has_cancellation
                || !declared.budget_bounded
                || declared.hidden_predicate_dispatch
            {
                "rejected"
            } else {
                "accepted_inert"
            }
        }
        _ => "rejected",
    }
}

fn transitions(form: &str, gate_outcome: &str) -> Vec<&'static str> {
    let mut stages = vec![
        "source_observed",
        "imported_inert",
        "identity_resolved",
        "ontology_classified",
        "relations_resolved",
        "code_classified",
    ];
    match gate_outcome {
        "rejected" => stages.push("rejected"),
        "quarantined" => stages.push("quarantined"),
        "accepted_inert" if form == "declarative_graph_ir" => stages.push("translated"),
        _ => {}
    }
    stages
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn validate_manifest(manifest: &CodeMigrationManifest) -> Result<(), UniverseError> {
    if manifest.contract_version != 0 {
        return Err(UniverseError::UnsupportedVersion(manifest.contract_version));
    }
    if manifest.source.source_graph_scope.is_empty() || manifest.source.ontology_revision == 0 {
        return Err(validation(
            "code migration source scope or revision is missing",
        ));
    }
    if manifest.candidates.is_empty() {
        return Err(validation("code migration pilot declares no candidate"));
    }
    let mut atoms = BTreeSet::new();
    for candidate in &manifest.candidates {
        for atom in [candidate.legacy_asset_atom, candidate.task_atom]
            .into_iter()
            .chain(candidate.definition_atom)
        {
            if !atoms.insert(atom) {
                return Err(validation(format!(
                    "candidate {} reuses an Atom identity",
                    candidate.source_label
                )));
            }
        }
        if !SOURCE_FORMS.contains(&candidate.source_form.as_str()) {
            return Err(validation(format!(
                "candidate {} has unknown source form {}",
                candidate.source_label, candidate.source_form
            )));
        }
        if candidate.justification.trim().is_empty() {
            return Err(validation(format!(
                "candidate {} is not justified",
                candidate.source_label
            )));
        }
        if !valid_hash(&candidate.row_sha256) {
            return Err(validation(format!(
                "candidate {} has an invalid source row hash",
                candidate.source_label
            )));
        }
        // A translated candidate must carry its own CodeDefinition Atom and a
        // pinned CodeDefinition revision; a non-translated one must not.
        let translated = gate(&candidate.source_form, &candidate.declared) == "accepted_inert"
            && candidate.source_form == "declarative_graph_ir";
        if translated
            && (candidate.definition_atom.is_none() || candidate.code_definition_revision.is_none())
        {
            return Err(validation(format!(
                "translated candidate {} lacks a pinned CodeDefinition Atom/revision",
                candidate.source_label
            )));
        }
        if !translated && candidate.definition_atom.is_some() {
            return Err(validation(format!(
                "non-translated candidate {} must not carry a CodeDefinition Atom",
                candidate.source_label
            )));
        }
    }
    Ok(())
}

/// One candidate's derived (engine-computed) classification.
struct Classified<'a> {
    candidate: &'a CodeCandidate,
    strategy: &'static str,
    state: &'static str,
    gate: &'static str,
    transitions: Vec<&'static str>,
    translated: bool,
}

fn classify(candidate: &CodeCandidate) -> Classified<'_> {
    let gate_outcome = gate(&candidate.source_form, &candidate.declared);
    let translated =
        gate_outcome == "accepted_inert" && candidate.source_form == "declarative_graph_ir";
    Classified {
        candidate,
        strategy: expected_strategy(&candidate.source_form),
        state: expected_state(&candidate.source_form),
        gate: gate_outcome,
        transitions: transitions(&candidate.source_form, gate_outcome),
        translated,
    }
}

pub fn materialize_seed(manifest: &CodeMigrationManifest) -> Result<GraphSeed, UniverseError> {
    validate_manifest(manifest)?;
    let symbols = vec![
        SYM_SOURCE.to_owned(),
        SYM_BATCH.to_owned(),
        SYM_LEGACY.to_owned(),
        SYM_TASK.to_owned(),
        SYM_CANDIDATE_DEF.to_owned(),
        SYM_RECEIPT.to_owned(),
        SYM_GOVERNED_BY.to_owned(),
        SYM_DESCRIBES.to_owned(),
        SYM_IMPORTS_FROM.to_owned(),
        SYM_TRANSLATES_TO.to_owned(),
        SYM_IN_BATCH.to_owned(),
        SYM_HAS_RECEIPT.to_owned(),
    ];

    let mut entities = vec![
        entity(
            manifest.source.atom,
            SYM_SOURCE,
            json!({
                "kind": "postgres_import_source",
                "authority_id": manifest.source.authority_id,
                "source_graph_scope": manifest.source.source_graph_scope,
                "observed_at": manifest.source.observed_at,
                "ontology_revision": manifest.source.ontology_revision,
                "read_only": true,
                "credentials_stored": false,
            }),
        ),
        entity(
            manifest.batch.atom,
            SYM_BATCH,
            json!({
                "kind": "code_migration_batch",
                "batch_id": manifest.batch.batch_id,
                "status": "prepared",
            }),
        ),
    ];

    let mut next = manifest.batch.relation_key_start.0;
    let mut relations = vec![relation(
        &mut next,
        manifest.batch.atom,
        manifest.source.atom,
        SYM_GOVERNED_BY,
    )];

    for candidate in &manifest.candidates {
        let c = classify(candidate);
        // Inert legacy source Asset: references the source row hash but imports
        // no executable payload.
        entities.push(entity(
            candidate.legacy_asset_atom,
            SYM_LEGACY,
            json!({
                "kind": "legacy_code_asset",
                "source_label": candidate.source_label,
                "graph_id": candidate.graph_id,
                "source_id": candidate.source_id,
                "source_form": candidate.source_form,
                "row_sha256": candidate.row_sha256,
                "source_revision": candidate.source_revision,
                "payload_imported": false,
                "executable": false,
            }),
        ));
        entities.push(entity(
            candidate.task_atom,
            SYM_TASK,
            json!({
                "kind": "code_migration_task",
                "source_label": candidate.source_label,
                "source_form": candidate.source_form,
                "strategy": c.strategy,
                "imported_state": c.state,
                "gate_outcome": c.gate,
                "declared": candidate.declared,
                "transitions": c.transitions,
                "downstream_not_measured": UNMEASURED_STAGES,
                "executable": false,
                "activated_for_later_execution": false,
                "lease_transferred": false,
                "trigger_wired": false,
            }),
        ));
        if c.translated {
            let definition_atom = candidate
                .definition_atom
                .expect("validated translated candidate has a definition Atom");
            entities.push(entity(
                definition_atom,
                SYM_CANDIDATE_DEF,
                json!({
                    "kind": "candidate_code_definition",
                    "source_label": candidate.source_label,
                    "state": "translated_inert",
                    "row_sha256": candidate.row_sha256,
                    "source_revision": candidate.source_revision,
                    "ontology_revision": manifest.source.ontology_revision,
                    "code_definition_revision": candidate.code_definition_revision,
                    "capabilities": candidate.declared.capabilities,
                    "budget_bounded": candidate.declared.budget_bounded,
                    "executable": false,
                    "activated": false,
                    "compiled": false,
                    "shadow_executed": false,
                }),
            ));
            relations.push(relation(
                &mut next,
                candidate.task_atom,
                definition_atom,
                SYM_TRANSLATES_TO,
            ));
        }
        relations.push(relation(
            &mut next,
            candidate.task_atom,
            candidate.legacy_asset_atom,
            SYM_DESCRIBES,
        ));
        relations.push(relation(
            &mut next,
            candidate.task_atom,
            manifest.source.atom,
            SYM_GOVERNED_BY,
        ));
        relations.push(relation(
            &mut next,
            candidate.legacy_asset_atom,
            manifest.source.atom,
            SYM_IMPORTS_FROM,
        ));
        relations.push(relation(
            &mut next,
            candidate.task_atom,
            manifest.batch.atom,
            SYM_IN_BATCH,
        ));
    }

    Ok(GraphSeed {
        universe: manifest.universe,
        symbols,
        entities,
        relations,
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct GateCounts {
    accepted_inert: usize,
    rejected: usize,
    quarantined: usize,
    translated: usize,
    migration_required: usize,
}

fn count(manifest: &CodeMigrationManifest) -> GateCounts {
    let mut counts = GateCounts::default();
    for candidate in &manifest.candidates {
        let c = classify(candidate);
        match c.gate {
            "accepted_inert" => counts.accepted_inert += 1,
            "rejected" => counts.rejected += 1,
            "quarantined" => counts.quarantined += 1,
            _ => {}
        }
        if c.translated {
            counts.translated += 1;
        }
        if c.state == "migration_required" {
            counts.migration_required += 1;
        }
    }
    counts
}

pub fn run_code_migration(
    manifest: &CodeMigrationManifest,
    output: impl AsRef<Path>,
) -> Result<CodeMigrationEvidence, UniverseError> {
    let store_root = output.as_ref();
    let store = UniverseStore::open(store_root)?;
    let installed = if store_root.join("snapshot.json").exists() {
        store.replay(store.load_snapshot()?)?
    } else {
        store.install_seed(&materialize_seed(manifest)?)?
    };
    let pre_receipt_snapshot_hash = installed.canonical_hash()?;

    let independent_store = UniverseStore::open(store_root)?;
    let mut independent = independent_store.replay(independent_store.load_snapshot()?)?;

    let counts = count(manifest);
    let receipt_content = json!({
        "kind": "adaptation_receipt",
        "batch_id": manifest.batch.batch_id,
        "status": "measured_code_classification_only",
        "information_status": "measured",
        "code_activated": false,
        "code_executed": false,
        "code_compiled": false,
        "shadow_executed": false,
        "executable_nodes": 0,
        "outcomes": {
            "accepted_inert": counts.accepted_inert,
            "rejected": counts.rejected,
            "quarantined": counts.quarantined,
            "translated_inert": counts.translated,
            "migration_required": counts.migration_required,
        },
        "downstream_not_measured": UNMEASURED_STAGES,
    });

    let activate_key = format!("{}:classify", manifest.batch.batch_id);
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
                        "justification": "Independent readback produced this measured code-classification receipt; no code was executed or activated."
                    }))?),
                },
            },
        ];
        let transaction = UniverseTransaction::prepare(
            &independent,
            UniverseWriteSet {
                base_revision: independent.revision,
                idempotency_key: activate_key,
                causal_ancestry: vec![manifest.batch.batch_id.clone()],
                commands,
            },
        )?;
        let tick = Tick(independent.tick.0 + 1);
        transaction.commit(&independent_store, &mut independent, tick)?;
    }

    // Independent replay and inertness verification.
    let final_store = UniverseStore::open(store_root)?;
    let final_snapshot = final_store.replay(final_store.load_snapshot()?)?;

    let mut executable_count = 0;
    let mut activated_count = 0;
    let mut shadow_executed_count = 0;
    for entity in &final_snapshot.entities {
        let Some(content_ref) = entity.content.as_ref() else {
            continue;
        };
        let content = final_store.read_content(content_ref)?;
        if content.get("executable") == Some(&Value::Bool(true)) {
            executable_count += 1;
        }
        if content.get("activated") == Some(&Value::Bool(true))
            || content.get("activated_for_later_execution") == Some(&Value::Bool(true))
        {
            activated_count += 1;
        }
        if content.get("shadow_executed") == Some(&Value::Bool(true)) {
            shadow_executed_count += 1;
        }
    }
    if executable_count != 0 || activated_count != 0 || shadow_executed_count != 0 {
        return Err(UniverseError::CorruptContent(
            "code migration pilot produced executable, activated, or shadow-executed state".into(),
        ));
    }

    let receipt_entity = final_snapshot
        .entities
        .iter()
        .find(|entity| entity.key == manifest.batch.receipt_atom)
        .and_then(|entity| entity.content.as_ref())
        .ok_or_else(|| validation("code migration receipt is missing after replay"))?;
    if final_store.read_content(receipt_entity)? != receipt_content {
        return Err(UniverseError::CorruptContent(
            "code migration receipt differs after replay".into(),
        ));
    }
    if count(manifest) != counts {
        return Err(UniverseError::CorruptContent(
            "code migration replay changed measured classification".into(),
        ));
    }

    let content_records_read_back = read_all_content(&final_store, &final_snapshot)?;
    Ok(CodeMigrationEvidence {
        batch_id: manifest.batch.batch_id.clone(),
        universe: final_snapshot.universe,
        total_candidates: manifest.candidates.len(),
        accepted_inert: counts.accepted_inert,
        rejected: counts.rejected,
        quarantined: counts.quarantined,
        translated_candidates: counts.translated,
        migration_required: counts.migration_required,
        executable_count: 0,
        activated_count: 0,
        shadow_executed_count: 0,
        pre_receipt_snapshot_hash,
        final_snapshot_hash: final_snapshot.canonical_hash()?,
        final_revision: final_snapshot.revision,
        final_tick: final_snapshot.tick,
        content_records_read_back,
        receipt_atom: manifest.batch.receipt_atom,
    })
}

fn read_all_content(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
) -> Result<usize, UniverseError> {
    let mut count = 0;
    for content in snapshot
        .entities
        .iter()
        .filter_map(|entity| entity.content.as_ref())
        .chain(
            snapshot
                .relations
                .iter()
                .filter_map(|relation| relation.content.as_ref()),
        )
    {
        store.read_content(content)?;
        count += 1;
    }
    Ok(count)
}

fn entity(key: EntityKey, symbol: &str, content: Value) -> SeedEntity {
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
        .ok_or_else(|| validation(format!("code migration symbol {value} is not interned")))
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> CodeMigrationManifest {
        serde_json::from_str(include_str!(
            "../../../fixtures/import/postgres-code-migration-pilot.json"
        ))
        .unwrap()
    }

    #[test]
    fn code_is_classified_inert_and_never_executed() {
        let manifest = manifest();
        let temp = tempfile::tempdir().unwrap();
        let evidence = run_code_migration(&manifest, temp.path()).unwrap();

        assert_eq!(evidence.total_candidates, 8);
        assert_eq!(evidence.accepted_inert, 5);
        assert_eq!(evidence.rejected, 1);
        assert_eq!(evidence.quarantined, 2);
        assert_eq!(evidence.translated_candidates, 1);
        // python + cypher carry the migration_required imported-state (independent
        // of the gate: the cypher candidate is still rejected).
        assert_eq!(evidence.migration_required, 2);
        assert_eq!(evidence.executable_count, 0);
        assert_eq!(evidence.activated_count, 0);
        assert_eq!(evidence.shadow_executed_count, 0);
        assert_eq!(
            evidence.accepted_inert + evidence.rejected + evidence.quarantined,
            evidence.total_candidates
        );
    }

    #[test]
    fn rerun_is_idempotent() {
        let manifest = manifest();
        let temp = tempfile::tempdir().unwrap();
        let first = run_code_migration(&manifest, temp.path()).unwrap();
        let second = run_code_migration(&manifest, temp.path()).unwrap();
        assert_eq!(first.final_revision, second.final_revision);
        assert_eq!(first.final_snapshot_hash, second.final_snapshot_hash);
    }

    #[test]
    fn unbounded_or_untimed_code_is_rejected_by_the_engine() {
        // The gate is computed, not declared: flipping a safety fact flips the outcome.
        let mut manifest = manifest();
        let python = manifest
            .candidates
            .iter_mut()
            .find(|candidate| candidate.source_form == "python")
            .expect("fixture has a python candidate");
        assert_eq!(
            gate(&python.source_form, &python.declared),
            "accepted_inert"
        );
        python.declared.has_timeout = false;
        assert_eq!(gate(&python.source_form, &python.declared), "rejected");
    }

    #[test]
    fn postgres_function_and_obsolete_are_always_quarantined() {
        let declared = DeclaredCode {
            has_timeout: true,
            has_cancellation: true,
            bounded_loops: true,
            budget_bounded: true,
            ..Default::default()
        };
        assert_eq!(gate("postgres_function", &declared), "quarantined");
        assert_eq!(gate("obsolete", &declared), "quarantined");
    }

    #[test]
    fn translated_candidate_requires_pinned_definition() {
        let mut manifest = manifest();
        let declarative = manifest
            .candidates
            .iter_mut()
            .find(|candidate| candidate.source_form == "declarative_graph_ir")
            .expect("fixture has a declarative candidate");
        declarative.definition_atom = None;
        assert!(matches!(
            validate_manifest(&manifest),
            Err(UniverseError::Validation(message))
                if message.contains("lacks a pinned CodeDefinition")
        ));
    }
}
