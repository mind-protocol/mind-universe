//! Generic Node-to-Asset projection bootstrap.
//!
//! Nodes, mappings, lifecycle choices, and invalidation declarations arrive as
//! graph data. This crate only validates those generic contracts, installs
//! content-addressed projections, and records independently observed evidence.

pub mod census;
pub mod conversion;
pub mod inventory;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseError, UniverseId};
use universe_store::{
    canonical_hash, EntityRecord, GraphSeed, RelationRecord, SeedEntity, SeedRelation,
    UniverseSnapshot, UniverseStore,
};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

const HASH_CONTRACT: &str = "sha256:canonical-json-v0";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AssetVocabulary {
    pub node: String,
    pub contract: String,
    pub mapping: String,
    pub batch: String,
    pub asset: String,
    pub payload: String,
    pub receipt: String,
    pub governed_by: String,
    pub derived_from: String,
    pub uses_mapping: String,
    pub has_payload: String,
    pub in_batch: String,
    pub invalidated_by: String,
    pub has_receipt: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectionContract {
    pub atom: EntityKey,
    pub contract_id: String,
    pub contract_revision: u64,
    pub hash_contract: String,
    pub node_remains_authoritative: bool,
    pub asset_is_derived: bool,
    pub invalidation_signals: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectionMapping {
    pub atom: EntityKey,
    pub mapping_id: String,
    pub revision: u64,
    pub output_kind: String,
    pub media_type: String,
    pub configuration: Value,
    pub configuration_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanonicalNode {
    pub atom: EntityKey,
    pub stable_id: String,
    pub revision: u64,
    pub content: Value,
    pub content_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InvalidationDeclaration {
    pub state: String,
    pub reasons: Vec<String>,
    pub replacement_asset: Option<EntityKey>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AssetProjection {
    pub asset_atom: EntityKey,
    pub payload_atom: EntityKey,
    pub asset_id: String,
    pub asset_version: u64,
    pub source_node_revision: u64,
    pub mapping_revision: u64,
    pub payload: Value,
    pub payload_sha256: String,
    pub invalidation: InvalidationDeclaration,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectionBatch {
    pub atom: EntityKey,
    pub receipt_atom: EntityKey,
    pub receipt_relation: RelationKey,
    pub relation_key_start: RelationKey,
    pub batch_id: String,
    pub expected_projection_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AssetManifest {
    pub contract_version: u16,
    pub universe: UniverseId,
    pub vocabulary: AssetVocabulary,
    pub contract: ProjectionContract,
    pub mapping: ProjectionMapping,
    pub node: CanonicalNode,
    pub batch: ProjectionBatch,
    pub projections: Vec<AssetProjection>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AssetAudit {
    pub current: usize,
    pub stale: usize,
    pub missing: usize,
    pub corrupt: usize,
    pub duplicate: usize,
    pub orphaned: usize,
    pub content_records_read: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AssetEvidence {
    pub batch_id: String,
    pub observed_status: String,
    pub audit: AssetAudit,
    pub source_node: EntityKey,
    pub source_node_revision: u64,
    pub node_preserved: bool,
    pub pre_receipt_snapshot_hash: String,
    pub final_snapshot_hash: String,
    pub final_revision: Revision,
    pub final_tick: Tick,
    pub receipt_atom: EntityKey,
    pub receipt_read_back: bool,
}

pub fn load_manifest(path: impl AsRef<Path>) -> Result<AssetManifest, UniverseError> {
    let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| UniverseError::CorruptContent(error.to_string()))
}

pub fn expected_asset_id(
    manifest: &AssetManifest,
    projection: &AssetProjection,
) -> Result<String, UniverseError> {
    Ok(format!(
        "sha256:{}",
        canonical_hash(&json!({
            "contract_atom": manifest.contract.atom,
            "contract_revision": manifest.contract.contract_revision,
            "mapping_atom": manifest.mapping.atom,
            "mapping_revision": projection.mapping_revision,
            "node_atom": manifest.node.atom,
            "node_revision": projection.source_node_revision,
            "payload_sha256": projection.payload_sha256,
        }))?
    ))
}

pub fn validate_manifest(manifest: &AssetManifest) -> Result<(), UniverseError> {
    if manifest.contract_version != 0 {
        return Err(UniverseError::UnsupportedVersion(manifest.contract_version));
    }
    if manifest.contract.hash_contract != HASH_CONTRACT
        || !manifest.contract.node_remains_authoritative
        || !manifest.contract.asset_is_derived
    {
        return Err(validation(
            "projection contract must preserve Node authority and derived Asset state",
        ));
    }
    let expected_signals = BTreeSet::from([
        "mapping_revision".to_owned(),
        "payload_hash".to_owned(),
        "source_node_revision".to_owned(),
    ]);
    if manifest
        .contract
        .invalidation_signals
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        != expected_signals
    {
        return Err(validation(
            "projection contract must declare revision and payload invalidation signals",
        ));
    }
    if manifest.mapping.output_kind.trim().is_empty()
        || manifest.mapping.media_type.trim().is_empty()
        || manifest.mapping.configuration_sha256 != canonical_hash(&manifest.mapping.configuration)?
    {
        return Err(validation("mapping configuration hash is invalid"));
    }
    if manifest.node.content_sha256 != canonical_hash(&manifest.node.content)? {
        return Err(validation("canonical Node content hash is invalid"));
    }
    if manifest.batch.expected_projection_count != manifest.projections.len()
        || manifest.projections.is_empty()
    {
        return Err(validation(
            "projection batch count differs from its graph declaration",
        ));
    }

    let mut entity_keys = BTreeSet::from([
        manifest.contract.atom,
        manifest.mapping.atom,
        manifest.node.atom,
        manifest.batch.atom,
        manifest.batch.receipt_atom,
    ]);
    let mut asset_ids = BTreeSet::new();
    let mut signatures = BTreeSet::new();
    let known_assets: BTreeSet<_> = manifest
        .projections
        .iter()
        .map(|projection| projection.asset_atom)
        .collect();
    let current_assets: BTreeSet<_> = manifest
        .projections
        .iter()
        .filter(|projection| projection.invalidation.state == "current")
        .map(|projection| projection.asset_atom)
        .collect();

    for projection in &manifest.projections {
        if !entity_keys.insert(projection.asset_atom)
            || !entity_keys.insert(projection.payload_atom)
        {
            return Err(validation("projection contains duplicate identity"));
        }
        if projection.payload_sha256 != canonical_hash(&projection.payload)? {
            return Err(validation(format!(
                "Asset {} payload hash is invalid",
                projection.asset_atom
            )));
        }
        if projection.asset_id != expected_asset_id(manifest, projection)? {
            return Err(validation(format!(
                "Asset {} content-addressed identity is invalid",
                projection.asset_atom
            )));
        }
        let signature = (
            projection.source_node_revision,
            projection.mapping_revision,
            projection.payload_sha256.clone(),
        );
        if !signatures.insert(signature) {
            return Err(validation("duplicate Asset projection signature"));
        }
        if !asset_ids.insert(&projection.asset_id) {
            return Err(validation("projection contains duplicate Asset ID"));
        }

        let mut expected_reasons = BTreeSet::new();
        if projection.source_node_revision != manifest.node.revision {
            expected_reasons.insert("source_node_revision_changed".to_owned());
        }
        if projection.mapping_revision != manifest.mapping.revision {
            expected_reasons.insert("mapping_revision_changed".to_owned());
        }
        let observed_reasons = projection
            .invalidation
            .reasons
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if expected_reasons.is_empty() {
            if projection.invalidation.state != "current"
                || !observed_reasons.is_empty()
                || projection.invalidation.replacement_asset.is_some()
            {
                return Err(validation(
                    "current Asset has inconsistent invalidation evidence",
                ));
            }
        } else {
            let replacement = projection
                .invalidation
                .replacement_asset
                .ok_or_else(|| validation("stale Asset has no replacement"))?;
            if projection.invalidation.state != "stale"
                || observed_reasons != expected_reasons
                || !known_assets.contains(&replacement)
                || !current_assets.contains(&replacement)
            {
                return Err(validation(
                    "stale Asset invalidation does not resolve to a current replacement",
                ));
            }
        }
    }
    if current_assets.len() != 1 {
        return Err(validation(
            "first projection slice requires exactly one current Asset",
        ));
    }
    Ok(())
}

pub fn materialize_seed(manifest: &AssetManifest) -> Result<GraphSeed, UniverseError> {
    validate_manifest(manifest)?;
    let v = &manifest.vocabulary;
    let symbols = vec![
        v.node.clone(),
        v.contract.clone(),
        v.mapping.clone(),
        v.batch.clone(),
        v.asset.clone(),
        v.payload.clone(),
        v.receipt.clone(),
        v.governed_by.clone(),
        v.derived_from.clone(),
        v.uses_mapping.clone(),
        v.has_payload.clone(),
        v.in_batch.clone(),
        v.invalidated_by.clone(),
        v.has_receipt.clone(),
    ];
    if symbols.iter().collect::<BTreeSet<_>>().len() != symbols.len() {
        return Err(validation("Asset vocabulary contains duplicate symbols"));
    }

    let mut entities = vec![
        seed_entity(
            manifest.contract.atom,
            &v.contract,
            json!({
                "kind": "asset_projection_contract",
                "contract_id": manifest.contract.contract_id,
                "contract_revision": manifest.contract.contract_revision,
                "hash_contract": manifest.contract.hash_contract,
                "node_remains_authoritative": manifest.contract.node_remains_authoritative,
                "asset_is_derived": manifest.contract.asset_is_derived,
                "invalidation_signals": manifest.contract.invalidation_signals,
            }),
        ),
        seed_entity(
            manifest.mapping.atom,
            &v.mapping,
            json!({
                "kind": "asset_projection_mapping",
                "mapping_id": manifest.mapping.mapping_id,
                "revision": manifest.mapping.revision,
                "output_kind": manifest.mapping.output_kind,
                "media_type": manifest.mapping.media_type,
                "configuration": manifest.mapping.configuration,
                "configuration_sha256": manifest.mapping.configuration_sha256,
            }),
        ),
        seed_entity(
            manifest.node.atom,
            &v.node,
            json!({
                "kind": "canonical_node",
                "stable_id": manifest.node.stable_id,
                "revision": manifest.node.revision,
                "content_sha256": manifest.node.content_sha256,
                "content": manifest.node.content,
            }),
        ),
        seed_entity(
            manifest.batch.atom,
            &v.batch,
            json!({
                "kind": "asset_projection_batch",
                "batch_id": manifest.batch.batch_id,
                "expected_projection_count": manifest.batch.expected_projection_count,
                "status": "prepared",
            }),
        ),
    ];
    let mut relations = Vec::new();
    let mut next_relation = manifest.batch.relation_key_start.0;
    relations.push(seed_relation(
        &mut next_relation,
        manifest.mapping.atom,
        manifest.contract.atom,
        &v.governed_by,
        None,
    ));
    relations.push(seed_relation(
        &mut next_relation,
        manifest.batch.atom,
        manifest.contract.atom,
        &v.governed_by,
        None,
    ));

    for projection in &manifest.projections {
        entities.push(seed_entity(
            projection.payload_atom,
            &v.payload,
            json!({
                "kind": "asset_payload",
                "content_address": format!("sha256:{}", projection.payload_sha256),
                "payload_sha256": projection.payload_sha256,
                "media_type": manifest.mapping.media_type,
                "value": projection.payload,
            }),
        ));
        entities.push(seed_entity(
            projection.asset_atom,
            &v.asset,
            json!({
                "kind": "asset_projection",
                "asset_id": projection.asset_id,
                "asset_version": projection.asset_version,
                "content_address": format!("sha256:{}", projection.payload_sha256),
                "payload_sha256": projection.payload_sha256,
                "source_node": manifest.node.atom,
                "source_node_revision": projection.source_node_revision,
                "source_node_content_sha256": manifest.node.content_sha256,
                "mapping": manifest.mapping.atom,
                "mapping_revision": projection.mapping_revision,
                "mapping_configuration_sha256": manifest.mapping.configuration_sha256,
                "lifecycle": projection.invalidation,
                "canonical_node_replaced": false,
            }),
        ));
        relations.push(seed_relation(
            &mut next_relation,
            projection.asset_atom,
            manifest.node.atom,
            &v.derived_from,
            Some(json!({
                "source_node_revision": projection.source_node_revision,
                "source_node_content_sha256": manifest.node.content_sha256,
            })),
        ));
        relations.push(seed_relation(
            &mut next_relation,
            projection.asset_atom,
            manifest.mapping.atom,
            &v.uses_mapping,
            Some(json!({
                "mapping_revision": projection.mapping_revision,
                "mapping_configuration_sha256": manifest.mapping.configuration_sha256,
            })),
        ));
        relations.push(seed_relation(
            &mut next_relation,
            projection.asset_atom,
            projection.payload_atom,
            &v.has_payload,
            Some(json!({"payload_sha256": projection.payload_sha256})),
        ));
        relations.push(seed_relation(
            &mut next_relation,
            projection.asset_atom,
            manifest.batch.atom,
            &v.in_batch,
            None,
        ));
        if let Some(replacement) = projection.invalidation.replacement_asset {
            relations.push(seed_relation(
                &mut next_relation,
                projection.asset_atom,
                replacement,
                &v.invalidated_by,
                Some(json!({"reasons": projection.invalidation.reasons})),
            ));
        }
    }

    Ok(GraphSeed {
        universe: manifest.universe,
        symbols,
        entities,
        relations,
    })
}

pub fn audit_store(
    manifest: &AssetManifest,
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
) -> Result<AssetAudit, UniverseError> {
    let mut audit = AssetAudit::default();
    let asset_symbol = snapshot
        .symbol_id(&manifest.vocabulary.asset)
        .ok_or_else(|| validation("Asset symbol is absent"))?;
    let derived_predicate = snapshot
        .symbol_id(&manifest.vocabulary.derived_from)
        .ok_or_else(|| validation("DERIVED_FROM symbol is absent"))?;
    let payload_predicate = snapshot
        .symbol_id(&manifest.vocabulary.has_payload)
        .ok_or_else(|| validation("HAS_PAYLOAD symbol is absent"))?;
    let mapping_predicate = snapshot
        .symbol_id(&manifest.vocabulary.uses_mapping)
        .ok_or_else(|| validation("USES_MAPPING symbol is absent"))?;
    let batch_predicate = snapshot
        .symbol_id(&manifest.vocabulary.in_batch)
        .ok_or_else(|| validation("IN_BATCH symbol is absent"))?;
    let invalidated_predicate = snapshot
        .symbol_id(&manifest.vocabulary.invalidated_by)
        .ok_or_else(|| validation("INVALIDATED_BY symbol is absent"))?;
    let mut signatures = BTreeMap::<(u64, u64, String), usize>::new();

    for projection in &manifest.projections {
        let Some(asset) = snapshot
            .entities
            .iter()
            .find(|entity| entity.key == projection.asset_atom && entity.symbol == asset_symbol)
        else {
            audit.missing += 1;
            continue;
        };
        let derived = snapshot.relations.iter().any(|relation| {
            relation.source == projection.asset_atom
                && relation.target == manifest.node.atom
                && relation.predicate == derived_predicate
        });
        let payload_link = snapshot.relations.iter().any(|relation| {
            relation.source == projection.asset_atom
                && relation.target == projection.payload_atom
                && relation.predicate == payload_predicate
        });
        let mapping_link = snapshot.relations.iter().any(|relation| {
            relation.source == projection.asset_atom
                && relation.target == manifest.mapping.atom
                && relation.predicate == mapping_predicate
        });
        let batch_link = snapshot.relations.iter().any(|relation| {
            relation.source == projection.asset_atom
                && relation.target == manifest.batch.atom
                && relation.predicate == batch_predicate
        });
        let invalidation_link = match projection.invalidation.replacement_asset {
            Some(replacement) => snapshot.relations.iter().any(|relation| {
                relation.source == projection.asset_atom
                    && relation.target == replacement
                    && relation.predicate == invalidated_predicate
            }),
            None => true,
        };
        if !derived || !payload_link || !mapping_link || !batch_link || !invalidation_link {
            audit.orphaned += 1;
        }

        let Some(asset_ref) = asset.content.as_ref() else {
            audit.corrupt += 1;
            continue;
        };
        let Some(payload) = snapshot
            .entities
            .iter()
            .find(|entity| entity.key == projection.payload_atom)
        else {
            audit.missing += 1;
            continue;
        };
        let Some(payload_ref) = payload.content.as_ref() else {
            audit.corrupt += 1;
            continue;
        };
        let read_asset = store.read_content(asset_ref);
        let read_payload = store.read_content(payload_ref);
        match (read_asset, read_payload) {
            (Ok(asset_content), Ok(payload_content)) => {
                audit.content_records_read += 2;
                let payload_matches = payload_content
                    .get("value")
                    .and_then(|value| canonical_hash(value).ok())
                    .as_deref()
                    == Some(projection.payload_sha256.as_str());
                let asset_matches = asset_content.get("asset_id").and_then(Value::as_str)
                    == Some(projection.asset_id.as_str())
                    && asset_content.get("source_node").and_then(Value::as_str)
                        == Some(manifest.node.atom.to_string().as_str())
                    && asset_content
                        .get("source_node_revision")
                        .and_then(Value::as_u64)
                        == Some(projection.source_node_revision)
                    && asset_content.get("mapping").and_then(Value::as_str)
                        == Some(manifest.mapping.atom.to_string().as_str())
                    && asset_content
                        .get("mapping_revision")
                        .and_then(Value::as_u64)
                        == Some(projection.mapping_revision)
                    && asset_content.get("payload_sha256").and_then(Value::as_str)
                        == Some(projection.payload_sha256.as_str())
                    && asset_content.get("canonical_node_replaced") == Some(&Value::Bool(false));
                if !payload_matches || !asset_matches {
                    audit.corrupt += 1;
                    continue;
                }
                match projection.invalidation.state.as_str() {
                    "current" => audit.current += 1,
                    "stale" => audit.stale += 1,
                    _ => audit.corrupt += 1,
                }
                *signatures
                    .entry((
                        projection.source_node_revision,
                        projection.mapping_revision,
                        projection.payload_sha256.clone(),
                    ))
                    .or_default() += 1;
            }
            _ => audit.corrupt += 1,
        }
    }
    audit.duplicate = signatures
        .values()
        .map(|count| count.saturating_sub(1))
        .sum();

    let node_symbol = snapshot
        .symbol_id(&manifest.vocabulary.node)
        .ok_or_else(|| validation("canonical Node symbol is absent"))?;
    let node = snapshot
        .entities
        .iter()
        .find(|entity| entity.key == manifest.node.atom && entity.symbol == node_symbol);
    if let Some(node) = node {
        let readback = node
            .content
            .as_ref()
            .ok_or_else(|| validation("canonical Node content pointer is absent"))
            .and_then(|content| store.read_content(content));
        match readback {
            Ok(content)
                if content.get("stable_id").and_then(Value::as_str)
                    == Some(manifest.node.stable_id.as_str())
                    && content.get("revision").and_then(Value::as_u64)
                        == Some(manifest.node.revision)
                    && content.get("content_sha256").and_then(Value::as_str)
                        == Some(manifest.node.content_sha256.as_str())
                    && content.get("content") == Some(&manifest.node.content) =>
            {
                audit.content_records_read += 1;
            }
            _ => audit.corrupt += 1,
        }
    } else {
        audit.orphaned += manifest.projections.len();
    }
    Ok(audit)
}

pub fn run_projection(
    manifest: &AssetManifest,
    store_root: impl AsRef<Path>,
) -> Result<AssetEvidence, UniverseError> {
    let seed = materialize_seed(manifest)?;
    let store = UniverseStore::open(store_root.as_ref())?;
    store.install_seed(&seed)?;

    let independent_store = UniverseStore::open(store_root.as_ref())?;
    let mut independent = independent_store.replay(independent_store.load_snapshot()?)?;
    let audit = audit_store(manifest, &independent_store, &independent)?;
    if audit.current != 1
        || audit.missing != 0
        || audit.corrupt != 0
        || audit.duplicate != 0
        || audit.orphaned != 0
    {
        return Err(validation(format!(
            "independent Asset audit did not pass: {audit:?}"
        )));
    }
    let pre_receipt_snapshot_hash = independent.canonical_hash()?;
    let receipt_content = json!({
        "kind": "asset_projection_receipt",
        "batch_id": manifest.batch.batch_id,
        "epistemic_state": "measured",
        "audit": audit,
        "source_node": manifest.node.atom,
        "source_node_revision": manifest.node.revision,
        "node_preserved": true,
        "pre_receipt_snapshot_hash": pre_receipt_snapshot_hash,
    });
    let receipt_ref = independent_store.append_content(&receipt_content)?;
    let receipt_symbol = independent
        .symbol_id(&manifest.vocabulary.receipt)
        .ok_or_else(|| validation("receipt symbol is absent"))?;
    let receipt_predicate = independent
        .symbol_id(&manifest.vocabulary.has_receipt)
        .ok_or_else(|| validation("HAS_RECEIPT symbol is absent"))?;
    let next_tick = Tick(independent.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(
        &independent,
        UniverseWriteSet {
            base_revision: independent.revision,
            idempotency_key: format!("asset-projection:{}", manifest.batch.batch_id),
            causal_ancestry: vec![
                manifest.contract.contract_id.clone(),
                manifest.mapping.mapping_id.clone(),
            ],
            commands: vec![
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
                        predicate: receipt_predicate,
                        content: None,
                    },
                },
            ],
        },
    )?;
    transaction.commit(&independent_store, &mut independent, next_tick)?;

    let final_store = UniverseStore::open(store_root.as_ref())?;
    let final_snapshot = final_store.replay(final_store.load_snapshot()?)?;
    let receipt = final_snapshot
        .entities
        .iter()
        .find(|entity| entity.key == manifest.batch.receipt_atom)
        .and_then(|entity| entity.content.as_ref())
        .ok_or_else(|| validation("independent readback could not find the receipt"))?;
    let receipt_readback = final_store.read_content(receipt)?;
    if receipt_readback != receipt_content
        || !final_snapshot.relations.iter().any(|relation| {
            relation.key == manifest.batch.receipt_relation
                && relation.source == manifest.batch.atom
                && relation.target == manifest.batch.receipt_atom
        })
    {
        return Err(validation("independent receipt readback differs"));
    }
    let final_audit = audit_store(manifest, &final_store, &final_snapshot)?;
    if final_audit != audit {
        return Err(validation(
            "Asset audit changed after receipt commit and replay",
        ));
    }

    Ok(AssetEvidence {
        batch_id: manifest.batch.batch_id.clone(),
        observed_status: "measured".into(),
        audit,
        source_node: manifest.node.atom,
        source_node_revision: manifest.node.revision,
        node_preserved: true,
        pre_receipt_snapshot_hash,
        final_snapshot_hash: final_snapshot.canonical_hash()?,
        final_revision: final_snapshot.revision,
        final_tick: final_snapshot.tick,
        receipt_atom: manifest.batch.receipt_atom,
        receipt_read_back: true,
    })
}

fn seed_entity(key: EntityKey, symbol: &str, content: Value) -> SeedEntity {
    SeedEntity {
        key,
        generation: 0,
        symbol: symbol.to_owned(),
        content,
    }
}

fn seed_relation(
    next: &mut u128,
    source: EntityKey,
    target: EntityKey,
    predicate: &str,
    content: Option<Value>,
) -> SeedRelation {
    let relation = SeedRelation {
        key: RelationKey(*next),
        generation: 0,
        source,
        target,
        predicate: predicate.to_owned(),
        content,
    };
    *next += 1;
    relation
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::OpenOptions,
        io::{Seek, SeekFrom, Write},
    };

    fn manifest() -> AssetManifest {
        let node_content = json!({"title": "canonical", "value": 2});
        let mapping_configuration = json!({"fields": ["title", "value"], "layout": "generic"});
        let current_payload = json!({"title": "canonical", "value": 2});
        let stale_payload = json!({"title": "canonical", "value": 1});
        let mut manifest = AssetManifest {
            contract_version: 0,
            universe: UniverseId(0xa550),
            vocabulary: AssetVocabulary {
                node: "canonical_node".into(),
                contract: "asset_projection_contract".into(),
                mapping: "asset_projection_mapping".into(),
                batch: "asset_projection_batch".into(),
                asset: "asset_projection".into(),
                payload: "asset_payload".into(),
                receipt: "asset_projection_receipt".into(),
                governed_by: "GOVERNED_BY".into(),
                derived_from: "DERIVED_FROM".into(),
                uses_mapping: "USES_MAPPING".into(),
                has_payload: "HAS_PAYLOAD".into(),
                in_batch: "IN_BATCH".into(),
                invalidated_by: "INVALIDATED_BY".into(),
                has_receipt: "HAS_RECEIPT".into(),
            },
            contract: ProjectionContract {
                atom: EntityKey(0xa551),
                contract_id: "node-asset-v0".into(),
                contract_revision: 1,
                hash_contract: HASH_CONTRACT.into(),
                node_remains_authoritative: true,
                asset_is_derived: true,
                invalidation_signals: vec![
                    "source_node_revision".into(),
                    "mapping_revision".into(),
                    "payload_hash".into(),
                ],
            },
            mapping: ProjectionMapping {
                atom: EntityKey(0xa552),
                mapping_id: "generic-view-v0".into(),
                revision: 2,
                output_kind: "generic_view".into(),
                media_type: "application/json".into(),
                configuration_sha256: canonical_hash(&mapping_configuration).unwrap(),
                configuration: mapping_configuration,
            },
            node: CanonicalNode {
                atom: EntityKey(0xa553),
                stable_id: "node:test".into(),
                revision: 2,
                content_sha256: canonical_hash(&node_content).unwrap(),
                content: node_content,
            },
            batch: ProjectionBatch {
                atom: EntityKey(0xa554),
                receipt_atom: EntityKey(0xa555),
                receipt_relation: RelationKey(0xafff),
                relation_key_start: RelationKey(0xa800),
                batch_id: "asset-test-1".into(),
                expected_projection_count: 2,
            },
            projections: vec![
                AssetProjection {
                    asset_atom: EntityKey(0xa560),
                    payload_atom: EntityKey(0xa570),
                    asset_id: String::new(),
                    asset_version: 1,
                    source_node_revision: 1,
                    mapping_revision: 2,
                    payload_sha256: canonical_hash(&stale_payload).unwrap(),
                    payload: stale_payload,
                    invalidation: InvalidationDeclaration {
                        state: "stale".into(),
                        reasons: vec!["source_node_revision_changed".into()],
                        replacement_asset: Some(EntityKey(0xa561)),
                    },
                },
                AssetProjection {
                    asset_atom: EntityKey(0xa561),
                    payload_atom: EntityKey(0xa571),
                    asset_id: String::new(),
                    asset_version: 2,
                    source_node_revision: 2,
                    mapping_revision: 2,
                    payload_sha256: canonical_hash(&current_payload).unwrap(),
                    payload: current_payload,
                    invalidation: InvalidationDeclaration {
                        state: "current".into(),
                        reasons: vec![],
                        replacement_asset: None,
                    },
                },
            ],
        };
        for index in 0..manifest.projections.len() {
            manifest.projections[index].asset_id =
                expected_asset_id(&manifest, &manifest.projections[index]).unwrap();
        }
        manifest
    }

    #[test]
    fn node_and_versioned_assets_commit_then_read_back_independently() {
        let manifest = manifest();
        let temp = tempfile::tempdir().unwrap();
        let evidence = run_projection(&manifest, temp.path()).unwrap();
        assert!(evidence.node_preserved);
        assert!(evidence.receipt_read_back);
        assert_eq!(evidence.audit.current, 1);
        assert_eq!(evidence.audit.stale, 1);
        assert_eq!(evidence.audit.corrupt, 0);
        assert_eq!(evidence.final_revision, Revision(1));
    }

    #[test]
    fn stale_asset_requires_explicit_current_replacement() {
        let mut manifest = manifest();
        manifest.projections[0].invalidation.replacement_asset = None;
        assert!(matches!(
            validate_manifest(&manifest),
            Err(UniverseError::Validation(message))
                if message == "stale Asset has no replacement"
        ));
    }

    #[test]
    fn duplicate_projection_signature_is_rejected() {
        let mut manifest = manifest();
        manifest.projections[0].source_node_revision = 2;
        manifest.projections[0].payload = manifest.projections[1].payload.clone();
        manifest.projections[0].payload_sha256 = manifest.projections[1].payload_sha256.clone();
        manifest.projections[0].invalidation = InvalidationDeclaration {
            state: "current".into(),
            reasons: vec![],
            replacement_asset: None,
        };
        manifest.projections[0].asset_id =
            expected_asset_id(&manifest, &manifest.projections[0]).unwrap();
        assert!(matches!(
            validate_manifest(&manifest),
            Err(UniverseError::Validation(message))
                if message == "duplicate Asset projection signature"
        ));
    }

    #[test]
    fn corrupted_payload_is_reported_separately() {
        let manifest = manifest();
        let seed = materialize_seed(&manifest).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let snapshot = store.install_seed(&seed).unwrap();
        let payload = snapshot
            .entities
            .iter()
            .find(|entity| entity.key == manifest.projections[0].payload_atom)
            .unwrap()
            .content
            .as_ref()
            .unwrap();
        let path = temp.path().join("content-0.jsonl");
        let mut file = OpenOptions::new().write(true).open(path).unwrap();
        file.seek(SeekFrom::Start(payload.pointer.offset)).unwrap();
        file.write_all(b"X").unwrap();
        file.sync_all().unwrap();

        let audit = audit_store(&manifest, &store, &snapshot).unwrap();
        assert_eq!(audit.corrupt, 1);
        assert_eq!(audit.current, 1);
        assert_eq!(audit.stale, 0);
    }
}
