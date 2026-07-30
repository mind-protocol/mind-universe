//! Bounded PostgreSQL identity, inert-relation, and ontology-adaptation bootstrap.
//!
//! PostgreSQL rows are source evidence. This crate materializes graph-owned
//! import contracts, inert Assets, endpoint-resolution outcomes, and receipts.
//! The identity and relation pilots activate nothing. The ontology pilot
//! (`ontology_pilot`) may activate *type and predicate mappings* through an
//! approved, source-graph-scoped, revision-pinned ChangeSet. The code-migration
//! pilot (`code_migration`) only *classifies* code into inert LegacyCodeAssets
//! and CodeMigrationTasks and applies a computed safety gate — it compiles,
//! shadow-executes, and activates nothing. None of these ever migrates running
//! code, activates physics, or makes a PostgreSQL row executable.

pub mod code_migration;
pub mod code_pilot;
pub mod code_translation;
pub mod cursor;
pub mod ontology_pilot;

use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};
use universe_core::{EntityKey, RelationKey, Tick, UniverseError, UniverseId};
use universe_store::{
    EntityRecord, GraphSeed, RelationRecord, SeedEntity, SeedRelation, UniverseSnapshot,
    UniverseStore,
};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImportVocabulary {
    pub source: String,
    pub contract: String,
    pub identity: String,
    pub batch: String,
    pub asset: String,
    pub ontology_mapping: String,
    pub code_strategy: String,
    pub receipt: String,
    pub imports_from: String,
    pub governed_by: String,
    pub uses_ontology_mapping: String,
    pub uses_code_strategy: String,
    pub maps_to: String,
    pub in_batch: String,
    pub has_receipt: String,
    pub relation_batch: String,
    pub source_relation: String,
    pub relation_outcome: String,
    pub inert_relation: String,
    pub has_outcome: String,
    pub source_endpoint_identity: String,
    pub target_endpoint_identity: String,
    pub has_relation_receipt: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImportSource {
    pub atom: EntityKey,
    pub authority_id: String,
    pub source_schema: String,
    pub source_graph_scope: Vec<String>,
    pub observed_at: String,
    pub read_only: bool,
    pub row_hash_contract: String,
    pub properties_hash_contract: String,
    pub census: SourceCensus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceCensus {
    pub node_count: u64,
    pub relation_count: u64,
    pub metalink_count: u64,
    pub moment_count: u64,
    pub execution_claim_count: u64,
    pub graph_count: u64,
    pub duplicate_global_node_ids: u64,
    pub node_type_distinct_count: u64,
    pub subtype_distinct_count: u64,
    pub relation_type_distinct_count: u64,
    pub exact_code_candidate_count: u64,
    pub property_sample_size: u64,
    pub property_key_distinct_sample: u64,
    pub property_code_candidate_sample: u64,
    pub relation_integrity_status: String,
    pub relation_sample_size: u64,
    pub relation_sample_dangling_count: u64,
    pub relation_sample_cross_graph_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImportContract {
    pub atom: EntityKey,
    pub contract_id: String,
    pub invariants: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OntologyMapping {
    pub atom: EntityKey,
    pub mapping_id: String,
    pub status: String,
    pub activation_allowed: bool,
    pub universal_type_binding: String,
    pub semantic_type_binding: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeStrategy {
    pub atom: EntityKey,
    pub strategy_id: String,
    pub status: String,
    pub target_kind: String,
    pub activation_allowed: bool,
    pub payload_import_allowed: bool,
    pub forbidden_fallbacks: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImportBatch {
    pub atom: EntityKey,
    pub receipt_atom: EntityKey,
    pub receipt_relation: RelationKey,
    pub relation_key_start: RelationKey,
    pub batch_id: String,
    pub expected_count: usize,
    pub status: String,
    pub watermark_updated_at: String,
    pub watermark_source_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub identity_atom: EntityKey,
    pub asset_atom: EntityKey,
    pub graph_id: String,
    pub source_id: String,
    pub node_type: String,
    pub subtype: String,
    pub source_status: Option<String>,
    pub source_revision: u64,
    pub updated_at: String,
    pub row_sha256: String,
    pub properties_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationPilot {
    pub atom: EntityKey,
    pub receipt_atom: EntityKey,
    pub receipt_relation: RelationKey,
    pub relation_key_start: RelationKey,
    pub batch_id: String,
    pub observed_at: String,
    pub expected_count: usize,
    pub expected_resolved_count: usize,
    pub expected_quarantine_count: usize,
    pub status: String,
    pub watermark_graph_id: String,
    pub watermark_source_id: String,
    pub watermark_relation_type: String,
    pub watermark_target_id: String,
    pub records: Vec<SourceRelationRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceRelationRecord {
    pub candidate_atom: EntityKey,
    pub outcome_atom: EntityKey,
    pub target_relation: Option<RelationKey>,
    pub graph_id: String,
    pub source_id: String,
    pub relation_type: String,
    pub target_id: String,
    pub source_revision: u64,
    pub updated_at: String,
    pub row_sha256: String,
    pub properties_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IdentityPilotManifest {
    pub contract_version: u16,
    pub universe: UniverseId,
    pub vocabulary: ImportVocabulary,
    pub source: ImportSource,
    pub contract: ImportContract,
    pub ontology_mapping: OntologyMapping,
    pub code_strategy: CodeStrategy,
    pub batch: ImportBatch,
    pub records: Vec<SourceRecord>,
    pub relation_pilot: RelationPilot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IdentityPilotEvidence {
    pub batch_id: String,
    pub imported_nodes: usize,
    pub executable_nodes: usize,
    pub ontology_activated: bool,
    pub source_relations_observed: usize,
    pub inert_relations_materialized: usize,
    pub quarantined_relations: usize,
    pub cross_graph_relations: usize,
    pub pre_receipt_snapshot_hash: String,
    pub final_snapshot_hash: String,
    pub final_revision: universe_core::Revision,
    pub final_tick: Tick,
    pub content_records_read_back: usize,
    pub receipt_atom: EntityKey,
    pub relation_receipt_atom: EntityKey,
}

pub fn load_manifest(path: impl AsRef<Path>) -> Result<IdentityPilotManifest, UniverseError> {
    let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| UniverseError::CorruptContent(error.to_string()))
}

pub fn validate_manifest(manifest: &IdentityPilotManifest) -> Result<(), UniverseError> {
    if manifest.contract_version != 0 {
        return Err(UniverseError::UnsupportedVersion(manifest.contract_version));
    }
    if !manifest.source.read_only {
        return Err(validation("PostgreSQL pilot source must be read-only"));
    }
    if manifest.source.row_hash_contract != "sha256:postgresql-jsonb-text-v0"
        || manifest.source.properties_hash_contract != "sha256:postgresql-jsonb-text-v0"
    {
        return Err(validation("PostgreSQL pilot hash contract is unsupported"));
    }
    if manifest.source.census.node_count == 0
        || manifest.source.census.graph_count == 0
        || manifest.source.census.relation_integrity_status == "complete"
    {
        return Err(validation(
            "source census must be non-empty and must preserve the measured integrity status",
        ));
    }
    if manifest.source.source_graph_scope.is_empty()
        || manifest
            .source
            .source_graph_scope
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != manifest.source.source_graph_scope.len()
    {
        return Err(validation(
            "source graph scope must be non-empty and contain no duplicates",
        ));
    }
    if manifest.batch.status != "prepared_identity_only"
        || manifest.ontology_mapping.status != "not_applied"
        || manifest.ontology_mapping.activation_allowed
        || manifest.code_strategy.status != "disabled"
        || manifest.code_strategy.target_kind != "legacy_code_asset"
        || manifest.code_strategy.activation_allowed
        || manifest.code_strategy.payload_import_allowed
    {
        return Err(validation(
            "identity pilot must preserve inert ontology and code state",
        ));
    }
    if manifest.batch.expected_count != manifest.records.len() || manifest.records.is_empty() {
        return Err(validation("batch count differs from source record count"));
    }
    let scoped_source_ids: BTreeSet<_> = manifest
        .records
        .iter()
        .map(|record| (&record.graph_id, &record.source_id))
        .collect();
    let global_source_ids: BTreeSet<_> = manifest
        .records
        .iter()
        .map(|record| &record.source_id)
        .collect();
    let identity_atoms: BTreeSet<_> = manifest
        .records
        .iter()
        .map(|record| record.identity_atom)
        .collect();
    let asset_atoms: BTreeSet<_> = manifest
        .records
        .iter()
        .map(|record| record.asset_atom)
        .collect();
    if scoped_source_ids.len() != manifest.records.len()
        || global_source_ids.len() != manifest.records.len()
        || identity_atoms.len() != manifest.records.len()
        || asset_atoms.len() != manifest.records.len()
        || !identity_atoms.is_disjoint(&asset_atoms)
    {
        return Err(validation("identity pilot contains duplicate identities"));
    }
    for record in &manifest.records {
        if !manifest
            .source
            .source_graph_scope
            .contains(&record.graph_id)
            || !valid_hash(&record.row_sha256)
            || !valid_hash(&record.properties_sha256)
        {
            return Err(validation(format!(
                "source record {} has invalid scope or hash",
                record.source_id
            )));
        }
    }
    validate_relation_pilot(manifest)?;
    Ok(())
}

fn validate_relation_pilot(manifest: &IdentityPilotManifest) -> Result<(), UniverseError> {
    let pilot = &manifest.relation_pilot;
    if pilot.status != "prepared_relations_inert"
        || pilot.expected_count != pilot.records.len()
        || pilot.records.is_empty()
        || pilot.expected_resolved_count + pilot.expected_quarantine_count != pilot.expected_count
    {
        return Err(validation(
            "relation pilot counts or inert preparation state are invalid",
        ));
    }
    let candidate_atoms: BTreeSet<_> = pilot
        .records
        .iter()
        .map(|record| record.candidate_atom)
        .collect();
    let outcome_atoms: BTreeSet<_> = pilot
        .records
        .iter()
        .map(|record| record.outcome_atom)
        .collect();
    let target_relations: BTreeSet<_> = pilot
        .records
        .iter()
        .filter_map(|record| record.target_relation)
        .collect();
    if candidate_atoms.len() != pilot.records.len()
        || outcome_atoms.len() != pilot.records.len()
        || !candidate_atoms.is_disjoint(&outcome_atoms)
    {
        return Err(validation(
            "relation pilot contains duplicate candidate or outcome Atoms",
        ));
    }
    let mut previous_key: Option<(&str, &str, &str, &str)> = None;
    let mut resolved_count = 0;
    let mut quarantine_count = 0;
    for record in &pilot.records {
        let key = (
            record.graph_id.as_str(),
            record.source_id.as_str(),
            record.relation_type.as_str(),
            record.target_id.as_str(),
        );
        if previous_key.is_some_and(|previous| previous >= key) {
            return Err(validation(
                "relation pilot records must use unique deterministic source ordering",
            ));
        }
        previous_key = Some(key);
        if !manifest
            .source
            .source_graph_scope
            .contains(&record.graph_id)
            || !valid_hash(&record.row_sha256)
            || !valid_hash(&record.properties_sha256)
        {
            return Err(validation(format!(
                "source relation {} -[{}]-> {} has invalid scope or hash",
                record.source_id, record.relation_type, record.target_id
            )));
        }
        let source = resolve_source_record(manifest, &record.source_id);
        let target = resolve_source_record(manifest, &record.target_id);
        match (source, target, record.target_relation) {
            (Some(_), Some(_), Some(_)) => resolved_count += 1,
            (Some(_), Some(_), None) => {
                return Err(validation(
                    "a relation with two resolved endpoints must declare its inert target key",
                ));
            }
            (_, _, None) => quarantine_count += 1,
            (_, _, Some(_)) => {
                return Err(validation(
                    "a relation with an unresolved endpoint cannot declare a target relation",
                ));
            }
        }
    }
    if target_relations.len() != resolved_count
        || resolved_count != pilot.expected_resolved_count
        || quarantine_count != pilot.expected_quarantine_count
    {
        return Err(validation(
            "relation pilot resolved/quarantine counts differ from endpoint resolution",
        ));
    }
    let watermark = (
        pilot.watermark_graph_id.as_str(),
        pilot.watermark_source_id.as_str(),
        pilot.watermark_relation_type.as_str(),
        pilot.watermark_target_id.as_str(),
    );
    if previous_key != Some(watermark) {
        return Err(validation(
            "relation pilot watermark does not match the last deterministic source row",
        ));
    }
    Ok(())
}

fn resolve_source_record<'a>(
    manifest: &'a IdentityPilotManifest,
    source_id: &str,
) -> Option<&'a SourceRecord> {
    manifest
        .records
        .iter()
        .find(|record| record.source_id == source_id)
}

pub fn materialize_seed(manifest: &IdentityPilotManifest) -> Result<GraphSeed, UniverseError> {
    validate_manifest(manifest)?;
    let vocabulary = &manifest.vocabulary;
    let symbols = vec![
        vocabulary.source.clone(),
        vocabulary.contract.clone(),
        vocabulary.identity.clone(),
        vocabulary.batch.clone(),
        vocabulary.asset.clone(),
        vocabulary.ontology_mapping.clone(),
        vocabulary.code_strategy.clone(),
        vocabulary.receipt.clone(),
        vocabulary.imports_from.clone(),
        vocabulary.governed_by.clone(),
        vocabulary.uses_ontology_mapping.clone(),
        vocabulary.uses_code_strategy.clone(),
        vocabulary.maps_to.clone(),
        vocabulary.in_batch.clone(),
        vocabulary.has_receipt.clone(),
        vocabulary.relation_batch.clone(),
        vocabulary.source_relation.clone(),
        vocabulary.relation_outcome.clone(),
        vocabulary.inert_relation.clone(),
        vocabulary.has_outcome.clone(),
        vocabulary.source_endpoint_identity.clone(),
        vocabulary.target_endpoint_identity.clone(),
        vocabulary.has_relation_receipt.clone(),
    ];
    if symbols.iter().collect::<BTreeSet<_>>().len() != symbols.len() {
        return Err(validation("import vocabulary contains duplicate symbols"));
    }

    let mut entities = vec![
        seed_entity(
            manifest.source.atom,
            &vocabulary.source,
            serde_json::json!({
                "kind": "postgres_import_source",
                "authority_id": manifest.source.authority_id,
                "source_schema": manifest.source.source_schema,
                "source_graph_scope": manifest.source.source_graph_scope,
                "observed_at": manifest.source.observed_at,
                "read_only": manifest.source.read_only,
                "credentials_stored": false,
                "row_hash_contract": manifest.source.row_hash_contract,
                "properties_hash_contract": manifest.source.properties_hash_contract,
                "census": manifest.source.census
            }),
        ),
        seed_entity(
            manifest.contract.atom,
            &vocabulary.contract,
            serde_json::json!({
                "kind": "postgres_import_contract",
                "contract_id": manifest.contract.contract_id,
                "invariants": manifest.contract.invariants
            }),
        ),
        seed_entity(
            manifest.ontology_mapping.atom,
            &vocabulary.ontology_mapping,
            serde_json::json!({
                "kind": "ontology_adaptation_mapping",
                "mapping_id": manifest.ontology_mapping.mapping_id,
                "status": manifest.ontology_mapping.status,
                "activation_allowed": manifest.ontology_mapping.activation_allowed,
                "universal_type_binding": manifest.ontology_mapping.universal_type_binding,
                "semantic_type_binding": manifest.ontology_mapping.semantic_type_binding
            }),
        ),
        seed_entity(
            manifest.code_strategy.atom,
            &vocabulary.code_strategy,
            serde_json::json!({
                "kind": "code_migration_strategy",
                "strategy_id": manifest.code_strategy.strategy_id,
                "status": manifest.code_strategy.status,
                "target_kind": manifest.code_strategy.target_kind,
                "activation_allowed": manifest.code_strategy.activation_allowed,
                "payload_import_allowed": manifest.code_strategy.payload_import_allowed,
                "forbidden_fallbacks": manifest.code_strategy.forbidden_fallbacks
            }),
        ),
        seed_entity(
            manifest.batch.atom,
            &vocabulary.batch,
            serde_json::json!({
                "kind": "import_batch",
                "batch_id": manifest.batch.batch_id,
                "expected_count": manifest.batch.expected_count,
                "status": manifest.batch.status,
                "watermark": {
                    "updated_at": manifest.batch.watermark_updated_at,
                    "source_id": manifest.batch.watermark_source_id
                }
            }),
        ),
        seed_entity(
            manifest.relation_pilot.atom,
            &vocabulary.relation_batch,
            serde_json::json!({
                "kind": "postgres_relation_import_batch",
                "batch_id": manifest.relation_pilot.batch_id,
                "observed_at": manifest.relation_pilot.observed_at,
                "expected_count": manifest.relation_pilot.expected_count,
                "expected_resolved_count": manifest.relation_pilot.expected_resolved_count,
                "expected_quarantine_count": manifest.relation_pilot.expected_quarantine_count,
                "status": manifest.relation_pilot.status,
                "watermark": {
                    "graph_id": manifest.relation_pilot.watermark_graph_id,
                    "source_id": manifest.relation_pilot.watermark_source_id,
                    "relation_type": manifest.relation_pilot.watermark_relation_type,
                    "target_id": manifest.relation_pilot.watermark_target_id
                }
            }),
        ),
    ];
    for record in &manifest.records {
        entities.push(seed_entity(
            record.identity_atom,
            &vocabulary.identity,
            serde_json::json!({
                "kind": "import_identity_map",
                "source_authority": manifest.source.authority_id,
                "graph_id": record.graph_id,
                "source_id": record.source_id,
                "target": record.asset_atom,
                "status": "resolved_for_source_revision"
            }),
        ));
        entities.push(seed_entity(
            record.asset_atom,
            &vocabulary.asset,
            serde_json::json!({
                "kind": "legacy_code_asset",
                "graph_id": record.graph_id,
                "source_id": record.source_id,
                "node_type": record.node_type,
                "subtype": record.subtype,
                "source_status": record.source_status,
                "target_status": "imported_inert",
                "source_revision": record.source_revision,
                "updated_at": record.updated_at,
                "row_sha256": record.row_sha256,
                "properties_sha256": record.properties_sha256,
                "payload_imported": false,
                "executable": false,
                "ontology_activated": false
            }),
        ));
    }
    for record in &manifest.relation_pilot.records {
        let source = resolve_source_record(manifest, &record.source_id);
        let target = resolve_source_record(manifest, &record.target_id);
        entities.push(seed_entity(
            record.candidate_atom,
            &vocabulary.source_relation,
            serde_json::json!({
                "kind": "postgres_source_relation_candidate",
                "relation_graph_id": record.graph_id,
                "source_id": record.source_id,
                "source_predicate": record.relation_type,
                "target_id": record.target_id,
                "source_revision": record.source_revision,
                "updated_at": record.updated_at,
                "row_sha256": record.row_sha256,
                "properties_sha256": record.properties_sha256,
                "properties_imported": false,
                "ontology_status": "not_applied",
                "predicate_activated": false,
                "physical_mapping_activated": false
            }),
        ));
        let status = if source.is_some() && target.is_some() {
            "resolved_inert"
        } else {
            "quarantined_unknown_endpoint"
        };
        let information_status = if status == "resolved_inert" {
            "measured"
        } else {
            "unknown"
        };
        entities.push(seed_entity(
            record.outcome_atom,
            &vocabulary.relation_outcome,
            serde_json::json!({
                "kind": "relation_adaptation_outcome",
                "source_relation": record.candidate_atom,
                "status": status,
                "information_status": information_status,
                "relation_graph_id": record.graph_id,
                "source_endpoint": endpoint_outcome(source),
                "target_endpoint": endpoint_outcome(target),
                "target_relation": record.target_relation,
                "row_sha256": record.row_sha256,
                "properties_sha256": record.properties_sha256,
                "ontology_activated": false,
                "executable": false
            }),
        ));
    }

    let mut relations = Vec::new();
    {
        let mut next_relation = manifest.batch.relation_key_start.0;
        let mut relation = |source, target, predicate: &str, justification: &str| {
            let record = SeedRelation {
                key: RelationKey(next_relation),
                generation: 0,
                source,
                target,
                predicate: predicate.to_owned(),
                content: Some(serde_json::json!({
                    "kind": "import_relation",
                    "justification": justification
                })),
            };
            next_relation += 1;
            relations.push(record);
        };
        relation(
            manifest.batch.atom,
            manifest.source.atom,
            &vocabulary.imports_from,
            "The batch is pinned to one observed read-only PostgreSQL source.",
        );
        relation(
            manifest.batch.atom,
            manifest.contract.atom,
            &vocabulary.governed_by,
            "The batch is bounded by the graph-owned import contract.",
        );
        relation(
            manifest.batch.atom,
            manifest.ontology_mapping.atom,
            &vocabulary.uses_ontology_mapping,
            "The identity pilot records but does not activate ontology adaptation.",
        );
        relation(
            manifest.batch.atom,
            manifest.code_strategy.atom,
            &vocabulary.uses_code_strategy,
            "Every code-bearing source Node is imported through the disabled strategy.",
        );
        relation(
            manifest.relation_pilot.atom,
            manifest.source.atom,
            &vocabulary.imports_from,
            "The relation batch is pinned to the independently observed read-only PostgreSQL source.",
        );
        relation(
            manifest.relation_pilot.atom,
            manifest.contract.atom,
            &vocabulary.governed_by,
            "The relation batch is bounded by the graph-owned import contract.",
        );
        relation(
            manifest.relation_pilot.atom,
            manifest.ontology_mapping.atom,
            &vocabulary.uses_ontology_mapping,
            "Source predicates remain unactivated until a graph-owned mapping is approved.",
        );
        for record in &manifest.records {
            relation(
                record.identity_atom,
                record.asset_atom,
                &vocabulary.maps_to,
                "The persisted identity mapping is the authority for this target ID.",
            );
            relation(
                record.asset_atom,
                manifest.batch.atom,
                &vocabulary.in_batch,
                "The inert Asset was observed and admitted by this bounded batch.",
            );
        }
    }
    let mut materialized_relations = Vec::new();
    {
        let mut next_relation = manifest.relation_pilot.relation_key_start.0;
        let mut relation = |source, target, predicate: &str, justification: &str| {
            let record = SeedRelation {
                key: RelationKey(next_relation),
                generation: 0,
                source,
                target,
                predicate: predicate.to_owned(),
                content: Some(serde_json::json!({
                    "kind": "import_relation",
                    "justification": justification
                })),
            };
            next_relation += 1;
            relations.push(record);
        };
        for record in &manifest.relation_pilot.records {
            relation(
                record.candidate_atom,
                manifest.relation_pilot.atom,
                &vocabulary.in_batch,
                "The source relation candidate belongs to this bounded deterministic batch.",
            );
            relation(
                record.candidate_atom,
                record.outcome_atom,
                &vocabulary.has_outcome,
                "Endpoint resolution produced this explicit graph-owned adaptation outcome.",
            );
            let source = resolve_source_record(manifest, &record.source_id);
            let target = resolve_source_record(manifest, &record.target_id);
            if let Some(source) = source {
                relation(
                    record.candidate_atom,
                    source.identity_atom,
                    &vocabulary.source_endpoint_identity,
                    "The source endpoint resolved through its persisted global identity map.",
                );
            }
            if let Some(target) = target {
                relation(
                    record.candidate_atom,
                    target.identity_atom,
                    &vocabulary.target_endpoint_identity,
                    "The target endpoint resolved through its persisted global identity map.",
                );
            }
            if let (Some(source), Some(target), Some(target_relation)) =
                (source, target, record.target_relation)
            {
                materialized_relations.push(SeedRelation {
                    key: target_relation,
                    generation: 0,
                    source: source.asset_atom,
                    target: target.asset_atom,
                    predicate: vocabulary.inert_relation.clone(),
                    content: Some(serde_json::json!({
                        "kind": "inert_imported_relation",
                        "relation_graph_id": record.graph_id,
                        "source_id": record.source_id,
                        "source_identity_graph_id": source.graph_id,
                        "source_predicate": record.relation_type,
                        "target_id": record.target_id,
                        "target_identity_graph_id": target.graph_id,
                        "source_revision": record.source_revision,
                        "updated_at": record.updated_at,
                        "row_sha256": record.row_sha256,
                        "properties_sha256": record.properties_sha256,
                        "properties_imported": false,
                        "ontology_activated": false,
                        "physical_mapping_activated": false,
                        "executable": false
                    })),
                });
            }
        }
    }
    relations.extend(materialized_relations);
    let seed = GraphSeed {
        universe: manifest.universe,
        symbols,
        entities,
        relations,
    };
    seed.validate()?;
    Ok(seed)
}

pub fn run_identity_pilot(
    manifest: &IdentityPilotManifest,
    output: impl AsRef<Path>,
) -> Result<IdentityPilotEvidence, UniverseError> {
    let seed = materialize_seed(manifest)?;
    let store = UniverseStore::open(output.as_ref())?;
    let installed = store.install_seed(&seed)?;
    let pre_receipt_snapshot_hash = installed.canonical_hash()?;

    let independent_store = UniverseStore::open(output.as_ref())?;
    let mut independent = independent_store.load_snapshot()?;
    if independent.canonical_hash()? != pre_receipt_snapshot_hash {
        return Err(UniverseError::CorruptContent(
            "identity pilot independent snapshot hash mismatch".into(),
        ));
    }
    let content_records_read_back = read_all_content(&independent_store, &independent)?;
    let imported_nodes = observe_inert_assets(&independent_store, &independent, manifest)?;
    let relation_observation = observe_relation_pilot(&independent_store, &independent, manifest)?;
    let receipt_content = serde_json::json!({
        "kind": "adaptation_receipt",
        "batch_id": manifest.batch.batch_id,
        "status": "measured_identity_only_import",
        "information_status": "measured",
        "imported_nodes": imported_nodes,
        "executable_nodes": 0,
        "ontology_activated": false,
        "source_observed_at": manifest.source.observed_at,
        "pre_receipt_snapshot_hash": pre_receipt_snapshot_hash,
        "content_records_read_back": content_records_read_back
    });
    let relation_receipt_content = serde_json::json!({
        "kind": "adaptation_receipt",
        "batch_id": manifest.relation_pilot.batch_id,
        "status": "measured_inert_relation_import",
        "information_status": "measured",
        "source_relations_observed": relation_observation.observed,
        "inert_relations_materialized": relation_observation.resolved,
        "quarantined_relations": relation_observation.quarantined,
        "cross_graph_relations": relation_observation.cross_graph,
        "ontology_activated": false,
        "physical_mapping_activated": false,
        "source_observed_at": manifest.relation_pilot.observed_at,
        "pre_receipt_snapshot_hash": pre_receipt_snapshot_hash,
        "content_records_read_back": content_records_read_back,
        "source_row_hashes": manifest.relation_pilot.records.iter().map(|record| {
            serde_json::json!({
                "candidate_atom": record.candidate_atom,
                "row_sha256": record.row_sha256,
                "properties_sha256": record.properties_sha256
            })
        }).collect::<Vec<_>>()
    });
    let content = independent_store.append_content(&receipt_content)?;
    let relation_receipt_content_ref =
        independent_store.append_content(&relation_receipt_content)?;
    let receipt_symbol = independent
        .symbol_id(&manifest.vocabulary.receipt)
        .ok_or_else(|| validation("receipt symbol is not interned"))?;
    let has_receipt = independent
        .symbol_id(&manifest.vocabulary.has_receipt)
        .ok_or_else(|| validation("HAS_RECEIPT symbol is not interned"))?;
    let has_relation_receipt = independent
        .symbol_id(&manifest.vocabulary.has_relation_receipt)
        .ok_or_else(|| validation("HAS_RELATION_RECEIPT symbol is not interned"))?;
    let transaction = UniverseTransaction::prepare(
        &independent,
        UniverseWriteSet {
            base_revision: independent.revision,
            idempotency_key: format!("{}:readback-receipt", manifest.batch.batch_id),
            causal_ancestry: vec![manifest.batch.batch_id.clone()],
            commands: vec![
                UniverseCommand::PutEntity {
                    entity: EntityRecord {
                        key: manifest.batch.receipt_atom,
                        generation: 0,
                        symbol: receipt_symbol,
                        content: Some(content),
                    },
                },
                UniverseCommand::PutRelation {
                    relation: RelationRecord {
                        key: manifest.batch.receipt_relation,
                        generation: 0,
                        source: manifest.batch.atom,
                        target: manifest.batch.receipt_atom,
                        predicate: has_receipt,
                        content: Some(independent_store.append_content(&serde_json::json!({
                            "kind": "import_relation",
                            "justification": "Independent store readback produced this measured receipt."
                        }))?),
                    },
                },
                UniverseCommand::PutEntity {
                    entity: EntityRecord {
                        key: manifest.relation_pilot.receipt_atom,
                        generation: 0,
                        symbol: receipt_symbol,
                        content: Some(relation_receipt_content_ref),
                    },
                },
                UniverseCommand::PutRelation {
                    relation: RelationRecord {
                        key: manifest.relation_pilot.receipt_relation,
                        generation: 0,
                        source: manifest.relation_pilot.atom,
                        target: manifest.relation_pilot.receipt_atom,
                        predicate: has_relation_receipt,
                        content: Some(independent_store.append_content(&serde_json::json!({
                            "kind": "import_relation",
                            "justification": "Exact endpoint, hash, ownership, and quarantine readback produced this measured relation receipt."
                        }))?),
                    },
                },
            ],
        },
    )?;
    let receipt_tick = Tick(independent.tick.0 + 1);
    transaction.commit(&independent_store, &mut independent, receipt_tick)?;

    let final_store = UniverseStore::open(output.as_ref())?;
    let final_snapshot = final_store.replay(final_store.load_snapshot()?)?;
    let receipt_entity = final_snapshot
        .entities
        .iter()
        .find(|entity| entity.key == manifest.batch.receipt_atom)
        .ok_or_else(|| validation("readback receipt Atom is missing"))?;
    let observed_receipt = final_store.read_content(
        receipt_entity
            .content
            .as_ref()
            .ok_or_else(|| validation("readback receipt has no content"))?,
    )?;
    let relation_receipt_entity = final_snapshot
        .entities
        .iter()
        .find(|entity| entity.key == manifest.relation_pilot.receipt_atom)
        .ok_or_else(|| validation("relation readback receipt Atom is missing"))?;
    let observed_relation_receipt = final_store.read_content(
        relation_receipt_entity
            .content
            .as_ref()
            .ok_or_else(|| validation("relation readback receipt has no content"))?,
    )?;
    if observed_receipt != receipt_content
        || observed_relation_receipt != relation_receipt_content
        || !final_snapshot.relations.iter().any(|relation| {
            relation.key == manifest.batch.receipt_relation
                && relation.source == manifest.batch.atom
                && relation.target == manifest.batch.receipt_atom
                && relation.predicate == has_receipt
        })
        || !final_snapshot.relations.iter().any(|relation| {
            relation.key == manifest.relation_pilot.receipt_relation
                && relation.source == manifest.relation_pilot.atom
                && relation.target == manifest.relation_pilot.receipt_atom
                && relation.predicate == has_relation_receipt
        })
    {
        return Err(UniverseError::CorruptContent(
            "identity pilot receipt readback mismatch".into(),
        ));
    }
    let final_relation_observation =
        observe_relation_pilot(&final_store, &final_snapshot, manifest)?;
    if final_relation_observation != relation_observation {
        return Err(UniverseError::CorruptContent(
            "relation pilot replay changed measured endpoint outcomes".into(),
        ));
    }
    let final_content_records_read_back = read_all_content(&final_store, &final_snapshot)?;
    Ok(IdentityPilotEvidence {
        batch_id: manifest.batch.batch_id.clone(),
        imported_nodes,
        executable_nodes: 0,
        ontology_activated: false,
        source_relations_observed: relation_observation.observed,
        inert_relations_materialized: relation_observation.resolved,
        quarantined_relations: relation_observation.quarantined,
        cross_graph_relations: relation_observation.cross_graph,
        pre_receipt_snapshot_hash,
        final_snapshot_hash: final_snapshot.canonical_hash()?,
        final_revision: final_snapshot.revision,
        final_tick: final_snapshot.tick,
        content_records_read_back: final_content_records_read_back,
        receipt_atom: manifest.batch.receipt_atom,
        relation_receipt_atom: manifest.relation_pilot.receipt_atom,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RelationObservation {
    observed: usize,
    resolved: usize,
    quarantined: usize,
    cross_graph: usize,
}

fn observe_inert_assets(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    manifest: &IdentityPilotManifest,
) -> Result<usize, UniverseError> {
    let asset_symbol = snapshot
        .symbol_id(&manifest.vocabulary.asset)
        .ok_or_else(|| validation("legacy Asset symbol is not interned"))?;
    let observed_asset_count = snapshot
        .entities
        .iter()
        .filter(|entity| entity.symbol == asset_symbol)
        .count();
    if observed_asset_count != manifest.records.len() {
        return Err(validation(format!(
            "independent readback found {observed_asset_count} inert Assets, expected {}",
            manifest.records.len()
        )));
    }
    for record in &manifest.records {
        let entity = snapshot
            .entities
            .iter()
            .find(|entity| entity.key == record.asset_atom && entity.symbol == asset_symbol)
            .ok_or_else(|| validation(format!("Asset {} is missing", record.asset_atom)))?;
        let content = store.read_content(
            entity
                .content
                .as_ref()
                .ok_or_else(|| validation("imported Asset has no content"))?,
        )?;
        if content["kind"] != "legacy_code_asset"
            || content["graph_id"] != record.graph_id
            || content["source_id"] != record.source_id
            || content["row_sha256"] != record.row_sha256
            || content["properties_sha256"] != record.properties_sha256
            || content["target_status"] != "imported_inert"
            || content["payload_imported"] != false
            || content["executable"] != false
            || content["ontology_activated"] != false
        {
            return Err(UniverseError::CorruptContent(format!(
                "independent inert Asset readback failed for {}",
                record.source_id
            )));
        }
    }

    let mapping = snapshot
        .entities
        .iter()
        .find(|entity| entity.key == manifest.ontology_mapping.atom)
        .ok_or_else(|| validation("ontology mapping Atom is missing"))?;
    let mapping_content = store.read_content(
        mapping
            .content
            .as_ref()
            .ok_or_else(|| validation("ontology mapping has no content"))?,
    )?;
    let strategy = snapshot
        .entities
        .iter()
        .find(|entity| entity.key == manifest.code_strategy.atom)
        .ok_or_else(|| validation("code strategy Atom is missing"))?;
    let strategy_content = store.read_content(
        strategy
            .content
            .as_ref()
            .ok_or_else(|| validation("code strategy has no content"))?,
    )?;
    if mapping_content["status"] != "not_applied"
        || mapping_content["activation_allowed"] != false
        || strategy_content["status"] != "disabled"
        || strategy_content["activation_allowed"] != false
        || strategy_content["payload_import_allowed"] != false
    {
        return Err(UniverseError::CorruptContent(
            "ontology or code strategy was activated during identity import".into(),
        ));
    }
    Ok(observed_asset_count)
}

fn observe_relation_pilot(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    manifest: &IdentityPilotManifest,
) -> Result<RelationObservation, UniverseError> {
    let candidate_symbol = snapshot
        .symbol_id(&manifest.vocabulary.source_relation)
        .ok_or_else(|| validation("source relation candidate symbol is not interned"))?;
    let outcome_symbol = snapshot
        .symbol_id(&manifest.vocabulary.relation_outcome)
        .ok_or_else(|| validation("relation outcome symbol is not interned"))?;
    let inert_predicate = snapshot
        .symbol_id(&manifest.vocabulary.inert_relation)
        .ok_or_else(|| validation("inert relation predicate is not interned"))?;
    let candidate_count = snapshot
        .entities
        .iter()
        .filter(|entity| entity.symbol == candidate_symbol)
        .count();
    let outcome_count = snapshot
        .entities
        .iter()
        .filter(|entity| entity.symbol == outcome_symbol)
        .count();
    if candidate_count != manifest.relation_pilot.records.len()
        || outcome_count != manifest.relation_pilot.records.len()
    {
        return Err(validation(
            "independent readback found an unexpected relation candidate/outcome count",
        ));
    }

    let mut observation = RelationObservation {
        observed: 0,
        resolved: 0,
        quarantined: 0,
        cross_graph: 0,
    };
    for record in &manifest.relation_pilot.records {
        let candidate = snapshot
            .entities
            .iter()
            .find(|entity| entity.key == record.candidate_atom && entity.symbol == candidate_symbol)
            .ok_or_else(|| validation("source relation candidate Atom is missing"))?;
        let candidate_content = store.read_content(
            candidate
                .content
                .as_ref()
                .ok_or_else(|| validation("source relation candidate has no content"))?,
        )?;
        if candidate_content["relation_graph_id"] != record.graph_id
            || candidate_content["source_id"] != record.source_id
            || candidate_content["source_predicate"] != record.relation_type
            || candidate_content["target_id"] != record.target_id
            || candidate_content["source_revision"] != record.source_revision
            || candidate_content["updated_at"] != record.updated_at
            || candidate_content["row_sha256"] != record.row_sha256
            || candidate_content["properties_sha256"] != record.properties_sha256
            || candidate_content["properties_imported"] != false
            || candidate_content["ontology_status"] != "not_applied"
            || candidate_content["predicate_activated"] != false
            || candidate_content["physical_mapping_activated"] != false
        {
            return Err(UniverseError::CorruptContent(format!(
                "source relation candidate readback mismatch for {} -[{}]-> {}",
                record.source_id, record.relation_type, record.target_id
            )));
        }
        let outcome = snapshot
            .entities
            .iter()
            .find(|entity| entity.key == record.outcome_atom && entity.symbol == outcome_symbol)
            .ok_or_else(|| validation("relation adaptation outcome Atom is missing"))?;
        let outcome_content = store.read_content(
            outcome
                .content
                .as_ref()
                .ok_or_else(|| validation("relation adaptation outcome has no content"))?,
        )?;
        if outcome_content["relation_graph_id"] != record.graph_id
            || outcome_content["source_relation"]
                != serde_json::to_value(record.candidate_atom)
                    .map_err(|error| validation(error.to_string()))?
            || outcome_content["row_sha256"] != record.row_sha256
            || outcome_content["properties_sha256"] != record.properties_sha256
            || outcome_content["ontology_activated"] != false
            || outcome_content["executable"] != false
        {
            return Err(UniverseError::CorruptContent(
                "relation adaptation outcome provenance mismatch".into(),
            ));
        }

        let source = resolve_source_record(manifest, &record.source_id);
        let target = resolve_source_record(manifest, &record.target_id);
        observation.observed += 1;
        match (source, target, record.target_relation) {
            (Some(source), Some(target), Some(target_relation)) => {
                if outcome_content["status"] != "resolved_inert"
                    || outcome_content["information_status"] != "measured"
                    || outcome_content["source_endpoint"] != endpoint_outcome(Some(source))
                    || outcome_content["target_endpoint"] != endpoint_outcome(Some(target))
                    || outcome_content["target_relation"]
                        != serde_json::to_value(target_relation)
                            .map_err(|error| validation(error.to_string()))?
                {
                    return Err(UniverseError::CorruptContent(
                        "resolved relation outcome does not match its endpoint identities".into(),
                    ));
                }
                let relation = snapshot
                    .relations
                    .iter()
                    .find(|relation| {
                        relation.key == target_relation && relation.predicate == inert_predicate
                    })
                    .ok_or_else(|| validation("resolved inert target relation is missing"))?;
                if relation.source != source.asset_atom || relation.target != target.asset_atom {
                    return Err(UniverseError::CorruptContent(
                        "resolved inert target relation endpoints do not match identity maps"
                            .into(),
                    ));
                }
                let relation_content = store.read_content(
                    relation
                        .content
                        .as_ref()
                        .ok_or_else(|| validation("resolved inert relation has no content"))?,
                )?;
                if relation_content["kind"] != "inert_imported_relation"
                    || relation_content["relation_graph_id"] != record.graph_id
                    || relation_content["source_id"] != record.source_id
                    || relation_content["source_identity_graph_id"] != source.graph_id
                    || relation_content["source_predicate"] != record.relation_type
                    || relation_content["target_id"] != record.target_id
                    || relation_content["target_identity_graph_id"] != target.graph_id
                    || relation_content["source_revision"] != record.source_revision
                    || relation_content["updated_at"] != record.updated_at
                    || relation_content["row_sha256"] != record.row_sha256
                    || relation_content["properties_sha256"] != record.properties_sha256
                    || relation_content["properties_imported"] != false
                    || relation_content["ontology_activated"] != false
                    || relation_content["physical_mapping_activated"] != false
                    || relation_content["executable"] != false
                {
                    return Err(UniverseError::CorruptContent(
                        "resolved inert relation content readback mismatch".into(),
                    ));
                }
                observation.resolved += 1;
                if record.graph_id != source.graph_id || record.graph_id != target.graph_id {
                    observation.cross_graph += 1;
                }
            }
            (_, _, None) => {
                if outcome_content["status"] != "quarantined_unknown_endpoint"
                    || outcome_content["information_status"] != "unknown"
                    || outcome_content["source_endpoint"] != endpoint_outcome(source)
                    || outcome_content["target_endpoint"] != endpoint_outcome(target)
                    || !outcome_content["target_relation"].is_null()
                {
                    return Err(UniverseError::CorruptContent(
                        "quarantined relation outcome does not preserve unknown endpoints".into(),
                    ));
                }
                let accidentally_materialized = snapshot
                    .relations
                    .iter()
                    .filter(|relation| relation.predicate == inert_predicate)
                    .any(|relation| {
                        relation
                            .content
                            .as_ref()
                            .and_then(|content| store.read_content(content).ok())
                            .is_some_and(|content| content["row_sha256"] == record.row_sha256)
                    });
                if accidentally_materialized {
                    return Err(UniverseError::CorruptContent(
                        "quarantined relation unexpectedly materialized a target relation".into(),
                    ));
                }
                observation.quarantined += 1;
            }
            _ => {
                return Err(validation(
                    "validated relation pilot produced an impossible endpoint state",
                ));
            }
        }
    }
    let inert_count = snapshot
        .relations
        .iter()
        .filter(|relation| relation.predicate == inert_predicate)
        .count();
    if inert_count != observation.resolved
        || observation.resolved != manifest.relation_pilot.expected_resolved_count
        || observation.quarantined != manifest.relation_pilot.expected_quarantine_count
    {
        return Err(validation(
            "independent relation readback count differs from the graph-owned batch contract",
        ));
    }
    Ok(observation)
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

fn seed_entity(key: EntityKey, symbol: &str, content: serde_json::Value) -> SeedEntity {
    SeedEntity {
        key,
        generation: 0,
        symbol: symbol.to_owned(),
        content,
    }
}

fn endpoint_outcome(record: Option<&SourceRecord>) -> serde_json::Value {
    match record {
        Some(record) => serde_json::json!({
            "status": "resolved",
            "information_status": "measured",
            "source_id": record.source_id,
            "identity_graph_id": record.graph_id,
            "identity_atom": record.identity_atom,
            "asset_atom": record.asset_atom
        }),
        None => serde_json::json!({
            "status": "unknown",
            "information_status": "unknown",
            "identity_graph_id": null,
            "identity_atom": null,
            "asset_atom": null
        }),
    }
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> IdentityPilotManifest {
        serde_json::from_str(include_str!(
            "../../../fixtures/import/postgres-identity-pilot.json"
        ))
        .unwrap()
    }

    #[test]
    fn identity_and_relation_pilot_is_inert_atomic_and_independently_readable() {
        let temp = tempfile::tempdir().unwrap();
        let evidence = run_identity_pilot(&manifest(), temp.path()).unwrap();
        assert_eq!(evidence.imported_nodes, 12);
        assert_eq!(evidence.executable_nodes, 0);
        assert!(!evidence.ontology_activated);
        assert_eq!(evidence.source_relations_observed, 5);
        assert_eq!(evidence.inert_relations_materialized, 3);
        assert_eq!(evidence.quarantined_relations, 2);
        assert_eq!(evidence.cross_graph_relations, 1);
        assert_eq!(evidence.final_revision, universe_core::Revision(1));
        assert_eq!(evidence.final_tick, Tick(1));
        assert_ne!(
            evidence.pre_receipt_snapshot_hash,
            evidence.final_snapshot_hash
        );
    }

    #[test]
    fn source_active_status_cannot_activate_imported_code() {
        let mut manifest = manifest();
        manifest.records[0].source_status = Some("active".into());
        let seed = materialize_seed(&manifest).unwrap();
        let asset = seed
            .entities
            .iter()
            .find(|entity| entity.key == manifest.records[0].asset_atom)
            .unwrap();
        assert_eq!(asset.content["source_status"], "active");
        assert_eq!(asset.content["target_status"], "imported_inert");
        assert_eq!(asset.content["executable"], false);
        assert_eq!(asset.content["ontology_activated"], false);
    }

    #[test]
    fn identity_pilot_rejects_code_or_ontology_activation() {
        let mut code_enabled = manifest();
        code_enabled.code_strategy.activation_allowed = true;
        assert!(validate_manifest(&code_enabled).is_err());

        let mut ontology_enabled = manifest();
        ontology_enabled.ontology_mapping.activation_allowed = true;
        assert!(validate_manifest(&ontology_enabled).is_err());
    }

    #[test]
    fn unresolved_endpoint_cannot_materialize_a_target_relation() {
        let mut manifest = manifest();
        manifest.relation_pilot.records[1].target_relation = Some(RelationKey(0x5aff));
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn invalid_relation_batch_writes_no_store_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("store");
        let mut manifest = manifest();
        manifest.relation_pilot.records[1].target_relation = Some(RelationKey(0x5aff));
        assert!(run_identity_pilot(&manifest, &output).is_err());
        assert!(!output.exists());
    }

    #[test]
    fn cross_graph_relation_preserves_owner_and_endpoint_graphs_separately() {
        let manifest = manifest();
        let seed = materialize_seed(&manifest).unwrap();
        let source_relation = &manifest.relation_pilot.records[0];
        let target = seed
            .relations
            .iter()
            .find(|relation| Some(relation.key) == source_relation.target_relation)
            .unwrap();
        assert_eq!(target.predicate, "INERT_IMPORTED_RELATION");
        assert_eq!(
            target.content.as_ref().unwrap()["relation_graph_id"],
            "l2:mind-blueprints"
        );
        assert_eq!(
            target.content.as_ref().unwrap()["source_identity_graph_id"],
            "l2:mind-blueprints"
        );
        assert_eq!(
            target.content.as_ref().unwrap()["target_identity_graph_id"],
            "l2:mind-kernel"
        );
        assert_eq!(
            target.content.as_ref().unwrap()["ontology_activated"],
            false
        );
    }

    #[test]
    fn quarantine_is_graph_owned_and_has_no_inert_target_relation() {
        let manifest = manifest();
        let seed = materialize_seed(&manifest).unwrap();
        let quarantined = &manifest.relation_pilot.records[1];
        let outcome = seed
            .entities
            .iter()
            .find(|entity| entity.key == quarantined.outcome_atom)
            .unwrap();
        assert_eq!(outcome.content["status"], "quarantined_unknown_endpoint");
        assert_eq!(outcome.content["information_status"], "unknown");
        assert!(outcome.content["target_relation"].is_null());
        assert_eq!(
            seed.relations
                .iter()
                .filter(|relation| relation.predicate == "INERT_IMPORTED_RELATION")
                .count(),
            3
        );
    }
}
