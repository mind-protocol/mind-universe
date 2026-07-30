//! Bounded and resumable inventory over graph-declared Node/Asset subjects.

use crate::{AssetVocabulary, CanonicalNode, ProjectionContract, ProjectionMapping, HASH_CONTRACT};
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

const MAX_INVENTORY_BATCH: usize = 1_024;
const STATES: [&str; 7] = [
    "current",
    "stale",
    "missing",
    "corrupt",
    "duplicate",
    "orphaned",
    "unknown",
];
/// Node-to-Asset conversion classes required by G1. A class answers "how far has
/// this Node been converted into its required Asset projection?", a distinct
/// axis from the projection-freshness `STATES`. Every freshness state maps to a
/// class through graph-owned policy; `intentionally_assetless` is reserved for
/// Nodes that declare they require no Asset and is never derived from freshness.
const CLASSES: [&str; 5] = [
    "converted",
    "partial",
    "blocked",
    "intentionally_assetless",
    "unknown",
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryVocabulary {
    #[serde(flatten)]
    pub asset: AssetVocabulary,
    pub inventory: String,
    pub orphan_source: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryPolicy {
    pub atom: EntityKey,
    pub inventory_id: String,
    pub ordering: String,
    pub cursor_mode: String,
    pub batch_limit: usize,
    pub rebuild_states: Vec<String>,
    pub state_labels: Vec<String>,
    pub class_labels: Vec<String>,
    pub conversion_classes: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryNode {
    #[serde(flatten)]
    pub node: CanonicalNode,
    pub asset_requirement: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ObservedAsset {
    pub asset_atom: EntityKey,
    pub payload_atom: EntityKey,
    pub node_atom: EntityKey,
    pub asset_id: String,
    pub asset_version: u64,
    pub source_node_revision: u64,
    pub mapping_revision: u64,
    pub payload: Value,
    pub payload_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventorySubject {
    pub cursor_key: EntityKey,
    pub kind: String,
    pub node_atom: Option<EntityKey>,
    pub asset_atom: Option<EntityKey>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RebuildProjection {
    pub node_atom: EntityKey,
    pub trigger_state: String,
    pub previous_asset: Option<EntityKey>,
    pub asset_atom: EntityKey,
    pub payload_atom: EntityKey,
    pub asset_id: String,
    pub asset_version: u64,
    pub payload: Value,
    pub payload_sha256: String,
    pub relation_key_start: RelationKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryBatch {
    pub batch_id: String,
    pub receipt_atom: EntityKey,
    pub receipt_relation: RelationKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryManifest {
    pub contract_version: u16,
    pub universe: UniverseId,
    pub vocabulary: InventoryVocabulary,
    pub contract: ProjectionContract,
    pub mapping: ProjectionMapping,
    pub policy: InventoryPolicy,
    pub orphan_source_atom: EntityKey,
    pub nodes: Vec<InventoryNode>,
    pub observed_assets: Vec<ObservedAsset>,
    pub subjects: Vec<InventorySubject>,
    pub rebuilds: Vec<RebuildProjection>,
    pub batches: Vec<InventoryBatch>,
    pub initial_relation_key_start: RelationKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubjectObservation {
    pub cursor_key: EntityKey,
    pub subject_atom: EntityKey,
    pub state: String,
    pub conversion_class: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryBatchReceipt {
    pub kind: String,
    pub inventory_id: String,
    pub batch_id: String,
    pub epistemic_state: String,
    pub input_cursor: Option<EntityKey>,
    pub next_cursor: Option<EntityKey>,
    pub limit: usize,
    pub processed: usize,
    pub observations: Vec<SubjectObservation>,
    pub state_counts: BTreeMap<String, usize>,
    pub class_counts: BTreeMap<String, usize>,
    pub reconstruction_count: usize,
    pub reconstruction_readback_count: usize,
    pub readback_completed: bool,
    pub readback_snapshot_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryEvidence {
    pub inventory_id: String,
    pub observed_status: String,
    pub batch_limit: usize,
    pub batch_receipts: Vec<InventoryBatchReceipt>,
    pub total_state_counts: BTreeMap<String, usize>,
    pub total_class_counts: BTreeMap<String, usize>,
    pub total_processed: usize,
    pub total_reconstructed: usize,
    pub next_cursor: Option<EntityKey>,
    pub cursor_published_only_after_readback: bool,
    pub replay_revision_unchanged: bool,
    pub idempotent_batch_count: usize,
    pub final_revision: Revision,
    pub final_tick: Tick,
    pub final_snapshot_hash: String,
}

pub fn load_inventory_manifest(path: impl AsRef<Path>) -> Result<InventoryManifest, UniverseError> {
    let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| UniverseError::CorruptContent(error.to_string()))
}

pub fn inventory_asset_id(
    manifest: &InventoryManifest,
    node_atom: EntityKey,
    node_revision: u64,
    mapping_revision: u64,
    payload_sha256: &str,
) -> Result<String, UniverseError> {
    Ok(format!(
        "sha256:{}",
        canonical_hash(&json!({
            "contract_atom": manifest.contract.atom,
            "contract_revision": manifest.contract.contract_revision,
            "mapping_atom": manifest.mapping.atom,
            "mapping_revision": mapping_revision,
            "node_atom": node_atom,
            "node_revision": node_revision,
            "payload_sha256": payload_sha256,
        }))?
    ))
}

pub fn validate_inventory_manifest(manifest: &InventoryManifest) -> Result<(), UniverseError> {
    if manifest.contract_version != 0 {
        return Err(UniverseError::UnsupportedVersion(manifest.contract_version));
    }
    if manifest.contract.hash_contract != HASH_CONTRACT
        || !manifest.contract.node_remains_authoritative
        || !manifest.contract.asset_is_derived
    {
        return Err(validation("inventory projection contract is unsafe"));
    }
    if manifest.policy.ordering != "entity_key_ascending"
        || manifest.policy.cursor_mode != "after_entity_key_exclusive"
        || manifest.policy.batch_limit == 0
        || manifest.policy.batch_limit > MAX_INVENTORY_BATCH
    {
        return Err(validation("inventory cursor or batch bound is unsupported"));
    }
    if manifest
        .policy
        .state_labels
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != STATES.into_iter().collect::<BTreeSet<_>>()
    {
        return Err(validation("inventory state vocabulary is incomplete"));
    }
    if manifest
        .policy
        .class_labels
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != CLASSES.into_iter().collect::<BTreeSet<_>>()
    {
        return Err(validation(
            "inventory conversion class vocabulary is incomplete",
        ));
    }
    let class_labels = manifest
        .policy
        .class_labels
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    // Every freshness state a subject can observe must resolve to exactly one
    // declared conversion class. Missing coverage stays a validation failure
    // rather than defaulting to `unknown`, preserving epistemic honesty.
    if manifest
        .policy
        .conversion_classes
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != STATES.into_iter().collect::<BTreeSet<_>>()
        || manifest
            .policy
            .conversion_classes
            .values()
            .any(|class| !class_labels.contains(class.as_str()))
    {
        return Err(validation(
            "inventory conversion class mapping does not cover every observed state",
        ));
    }
    let allowed_rebuilds = BTreeSet::from(["stale", "missing", "corrupt"]);
    let rebuild_states = manifest
        .policy
        .rebuild_states
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if rebuild_states.is_empty() || !rebuild_states.is_subset(&allowed_rebuilds) {
        return Err(validation("inventory rebuild policy is unsupported"));
    }
    if canonical_hash(&manifest.mapping.configuration)? != manifest.mapping.configuration_sha256 {
        return Err(validation("inventory mapping hash is invalid"));
    }

    let mut node_keys = BTreeSet::new();
    for node in &manifest.nodes {
        if !node_keys.insert(node.node.atom)
            || canonical_hash(&node.node.content)? != node.node.content_sha256
            || !matches!(node.asset_requirement.as_str(), "required" | "unknown")
        {
            return Err(validation("inventory Node declaration is invalid"));
        }
    }
    let mut asset_keys = BTreeSet::new();
    let mut payload_keys = BTreeSet::new();
    for asset in &manifest.observed_assets {
        if !asset_keys.insert(asset.asset_atom)
            || !payload_keys.insert(asset.payload_atom)
            || asset.asset_id
                != inventory_asset_id(
                    manifest,
                    asset.node_atom,
                    asset.source_node_revision,
                    asset.mapping_revision,
                    &asset.payload_sha256,
                )?
            || (!node_keys.contains(&asset.node_atom)
                && asset.node_atom != manifest.orphan_source_atom)
        {
            return Err(validation("observed Asset declaration is invalid"));
        }
    }

    let mut cursors = BTreeSet::new();
    let mut subject_nodes = BTreeSet::new();
    let mut orphan_assets = BTreeSet::new();
    for subject in &manifest.subjects {
        if !cursors.insert(subject.cursor_key) {
            return Err(validation("inventory cursor key is duplicated"));
        }
        match subject.kind.as_str() {
            "node" => {
                let node = subject
                    .node_atom
                    .ok_or_else(|| validation("Node subject has no Node"))?;
                if subject.asset_atom.is_some()
                    || subject.cursor_key != node
                    || !node_keys.contains(&node)
                    || !subject_nodes.insert(node)
                {
                    return Err(validation("Node inventory subject is invalid"));
                }
            }
            "orphan_asset" => {
                let asset = subject
                    .asset_atom
                    .ok_or_else(|| validation("orphan subject has no Asset"))?;
                if subject.node_atom.is_some()
                    || subject.cursor_key != asset
                    || !asset_keys.contains(&asset)
                    || !orphan_assets.insert(asset)
                {
                    return Err(validation("orphan inventory subject is invalid"));
                }
            }
            _ => return Err(validation("inventory subject kind is unknown")),
        }
    }
    if subject_nodes != node_keys
        || manifest
            .observed_assets
            .iter()
            .filter(|asset| asset.node_atom == manifest.orphan_source_atom)
            .any(|asset| !orphan_assets.contains(&asset.asset_atom))
    {
        return Err(validation(
            "inventory subjects do not cover the declared scope",
        ));
    }

    let required_batches = manifest
        .subjects
        .len()
        .div_ceil(manifest.policy.batch_limit);
    if manifest.batches.len() != required_batches {
        return Err(validation("inventory has insufficient receipt batches"));
    }
    let mut receipt_atoms = BTreeSet::new();
    let mut receipt_relations = BTreeSet::new();
    for batch in &manifest.batches {
        if batch.batch_id.trim().is_empty()
            || !receipt_atoms.insert(batch.receipt_atom)
            || !receipt_relations.insert(batch.receipt_relation)
        {
            return Err(validation("inventory batch receipt identity is invalid"));
        }
    }

    let mut rebuild_nodes = BTreeSet::new();
    for rebuild in &manifest.rebuilds {
        let node = manifest
            .nodes
            .iter()
            .find(|node| node.node.atom == rebuild.node_atom)
            .ok_or_else(|| validation("rebuild Node is absent"))?;
        if !rebuild_states.contains(rebuild.trigger_state.as_str())
            || !rebuild_nodes.insert(rebuild.node_atom)
            || asset_keys.contains(&rebuild.asset_atom)
            || payload_keys.contains(&rebuild.payload_atom)
            || canonical_hash(&rebuild.payload)? != rebuild.payload_sha256
            || rebuild.asset_id
                != inventory_asset_id(
                    manifest,
                    rebuild.node_atom,
                    node.node.revision,
                    manifest.mapping.revision,
                    &rebuild.payload_sha256,
                )?
        {
            return Err(validation("rebuild projection is invalid"));
        }
    }
    Ok(())
}

pub fn materialize_inventory_seed(
    manifest: &InventoryManifest,
) -> Result<GraphSeed, UniverseError> {
    validate_inventory_manifest(manifest)?;
    let v = &manifest.vocabulary;
    let symbols = vec![
        v.asset.node.clone(),
        v.asset.contract.clone(),
        v.asset.mapping.clone(),
        v.asset.asset.clone(),
        v.asset.payload.clone(),
        v.asset.receipt.clone(),
        v.asset.governed_by.clone(),
        v.asset.derived_from.clone(),
        v.asset.uses_mapping.clone(),
        v.asset.has_payload.clone(),
        v.asset.invalidated_by.clone(),
        v.asset.has_receipt.clone(),
        v.inventory.clone(),
        v.orphan_source.clone(),
    ];
    if symbols.iter().collect::<BTreeSet<_>>().len() != symbols.len() {
        return Err(validation("inventory vocabulary has duplicate symbols"));
    }
    let mut entities = vec![
        entity(
            manifest.contract.atom,
            &v.asset.contract,
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
        entity(
            manifest.mapping.atom,
            &v.asset.mapping,
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
        entity(
            manifest.policy.atom,
            &v.inventory,
            json!({
                "kind": "asset_inventory_policy",
                "inventory_id": manifest.policy.inventory_id,
                "ordering": manifest.policy.ordering,
                "cursor_mode": manifest.policy.cursor_mode,
                "batch_limit": manifest.policy.batch_limit,
                "rebuild_states": manifest.policy.rebuild_states,
                "state_labels": manifest.policy.state_labels,
                "class_labels": manifest.policy.class_labels,
                "conversion_classes": manifest.policy.conversion_classes,
            }),
        ),
        entity(
            manifest.orphan_source_atom,
            &v.orphan_source,
            json!({"kind": "unresolved_asset_source", "epistemic_state": "known_absent"}),
        ),
    ];
    for node in &manifest.nodes {
        entities.push(entity(
            node.node.atom,
            &v.asset.node,
            json!({
                "kind": "canonical_node",
                "stable_id": node.node.stable_id,
                "revision": node.node.revision,
                "content": node.node.content,
                "content_sha256": node.node.content_sha256,
                "asset_requirement": node.asset_requirement,
            }),
        ));
    }
    for asset in &manifest.observed_assets {
        entities.extend(observed_asset_entities(manifest, asset));
    }
    let mut next = manifest.initial_relation_key_start.0;
    let mut relations = vec![
        relation(
            &mut next,
            manifest.mapping.atom,
            manifest.contract.atom,
            &v.asset.governed_by,
        ),
        relation(
            &mut next,
            manifest.policy.atom,
            manifest.contract.atom,
            &v.asset.governed_by,
        ),
    ];
    for asset in &manifest.observed_assets {
        relations.extend(asset_relations(
            &mut next,
            asset.asset_atom,
            asset.payload_atom,
            asset.node_atom,
            manifest.mapping.atom,
            &v.asset,
        ));
    }
    Ok(GraphSeed {
        universe: manifest.universe,
        symbols,
        entities,
        relations,
    })
}

fn classify(
    manifest: &InventoryManifest,
    subject: &InventorySubject,
) -> Result<SubjectObservation, UniverseError> {
    let (subject_atom, state) = match subject.kind.as_str() {
        "orphan_asset" => (
            subject.asset_atom.expect("validated orphan Asset"),
            "orphaned",
        ),
        "node" => {
            let node_atom = subject.node_atom.expect("validated Node");
            let node = manifest
                .nodes
                .iter()
                .find(|node| node.node.atom == node_atom)
                .expect("validated Node subject");
            if node.asset_requirement == "unknown" {
                (node_atom, "unknown")
            } else {
                let assets = manifest
                    .observed_assets
                    .iter()
                    .filter(|asset| asset.node_atom == node_atom)
                    .collect::<Vec<_>>();
                match assets.as_slice() {
                    [] => (node_atom, "missing"),
                    [asset] => {
                        if canonical_hash(&asset.payload)? != asset.payload_sha256 {
                            (node_atom, "corrupt")
                        } else if asset.source_node_revision != node.node.revision
                            || asset.mapping_revision != manifest.mapping.revision
                        {
                            (node_atom, "stale")
                        } else {
                            (node_atom, "current")
                        }
                    }
                    _ => (node_atom, "duplicate"),
                }
            }
        }
        _ => return Err(validation("unvalidated inventory subject")),
    };
    let conversion_class = manifest
        .policy
        .conversion_classes
        .get(state)
        .cloned()
        .ok_or_else(|| validation("observed state has no graph-owned conversion class"))?;
    Ok(SubjectObservation {
        cursor_key: subject.cursor_key,
        subject_atom,
        state: state.into(),
        conversion_class,
    })
}

fn process_batch(
    manifest: &InventoryManifest,
    store_root: &Path,
    batch: &InventoryBatch,
    input_cursor: Option<EntityKey>,
) -> Result<(InventoryBatchReceipt, bool), UniverseError> {
    let store = UniverseStore::open(store_root)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    if let Some(receipt) = snapshot
        .entities
        .iter()
        .find(|entity| entity.key == batch.receipt_atom)
        .and_then(|entity| entity.content.as_ref())
    {
        let content = store.read_content(receipt)?;
        let receipt: InventoryBatchReceipt = serde_json::from_value(content)
            .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
        if receipt.batch_id != batch.batch_id
            || receipt.input_cursor != input_cursor
            || !receipt.readback_completed
        {
            return Err(validation(
                "existing inventory receipt does not match resume cursor",
            ));
        }
        return Ok((receipt, true));
    }

    let mut subjects = manifest.subjects.iter().collect::<Vec<_>>();
    subjects.sort_by_key(|subject| subject.cursor_key);
    let remaining = subjects
        .into_iter()
        .filter(|subject| input_cursor.is_none_or(|cursor| subject.cursor_key > cursor))
        .collect::<Vec<_>>();
    let selected = remaining
        .iter()
        .take(manifest.policy.batch_limit)
        .copied()
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(validation("inventory batch has no remaining subject"));
    }
    let next_cursor = if remaining.len() > selected.len() {
        selected.last().map(|subject| subject.cursor_key)
    } else {
        None
    };
    let observations = selected
        .iter()
        .map(|subject| classify(manifest, subject))
        .collect::<Result<Vec<_>, _>>()?;
    let mut state_counts = empty_counts();
    let mut class_counts = empty_class_counts(manifest);
    for observation in &observations {
        *state_counts
            .get_mut(&observation.state)
            .expect("validated state") += 1;
        *class_counts
            .get_mut(&observation.conversion_class)
            .expect("validated conversion class") += 1;
    }

    let mut reconstruction_count = 0;
    let mut commands = Vec::new();
    let asset_symbol = symbol(&snapshot, &manifest.vocabulary.asset.asset)?;
    let payload_symbol = symbol(&snapshot, &manifest.vocabulary.asset.payload)?;
    let derived = symbol(&snapshot, &manifest.vocabulary.asset.derived_from)?;
    let uses_mapping = symbol(&snapshot, &manifest.vocabulary.asset.uses_mapping)?;
    let has_payload = symbol(&snapshot, &manifest.vocabulary.asset.has_payload)?;
    let invalidated_by = symbol(&snapshot, &manifest.vocabulary.asset.invalidated_by)?;

    for observation in &observations {
        if !manifest
            .policy
            .rebuild_states
            .iter()
            .any(|state| state == &observation.state)
        {
            continue;
        }
        let rebuild = manifest
            .rebuilds
            .iter()
            .find(|rebuild| {
                rebuild.node_atom == observation.subject_atom
                    && rebuild.trigger_state == observation.state
            })
            .ok_or_else(|| validation("rebuild policy has no graph-owned projection"))?;
        let node = manifest
            .nodes
            .iter()
            .find(|node| node.node.atom == rebuild.node_atom)
            .expect("validated rebuild Node");
        let records = asset_record_values(
            manifest,
            rebuild.asset_atom,
            rebuild.payload_atom,
            rebuild.node_atom,
            node.node.revision,
            manifest.mapping.revision,
            rebuild.asset_version,
            &rebuild.asset_id,
            &rebuild.payload,
            &rebuild.payload_sha256,
            "current",
        );
        let payload_ref = store.append_content(&records.0)?;
        let asset_ref = store.append_content(&records.1)?;
        commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: rebuild.payload_atom,
                generation: 0,
                symbol: payload_symbol,
                content: Some(payload_ref),
            },
        });
        commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: rebuild.asset_atom,
                generation: 0,
                symbol: asset_symbol,
                content: Some(asset_ref),
            },
        });
        let mut relation_key = rebuild.relation_key_start.0;
        for (target, predicate) in [
            (rebuild.node_atom, derived),
            (manifest.mapping.atom, uses_mapping),
            (rebuild.payload_atom, has_payload),
        ] {
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(relation_key),
                    generation: 0,
                    source: rebuild.asset_atom,
                    target,
                    predicate,
                    content: None,
                },
            });
            relation_key += 1;
        }
        if let Some(previous) = rebuild.previous_asset {
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(relation_key),
                    generation: 0,
                    source: previous,
                    target: rebuild.asset_atom,
                    predicate: invalidated_by,
                    content: None,
                },
            });
        }
        reconstruction_count += 1;
    }
    if !commands.is_empty() {
        let tick = Tick(snapshot.tick.0 + 1);
        let transaction = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: snapshot.revision,
                idempotency_key: format!("asset-inventory-rebuild:{}", batch.batch_id),
                causal_ancestry: vec![
                    manifest.policy.inventory_id.clone(),
                    manifest.mapping.mapping_id.clone(),
                ],
                commands,
            },
        )?;
        transaction.commit(&store, &mut snapshot, tick)?;
    }

    let readback_store = UniverseStore::open(store_root)?;
    let mut readback = readback_store.replay(readback_store.load_snapshot()?)?;
    let mut reconstruction_readback_count = 0;
    for observation in &observations {
        let Some(rebuild) = manifest.rebuilds.iter().find(|rebuild| {
            rebuild.node_atom == observation.subject_atom
                && rebuild.trigger_state == observation.state
        }) else {
            continue;
        };
        let asset = readback
            .entities
            .iter()
            .find(|entity| entity.key == rebuild.asset_atom)
            .and_then(|entity| entity.content.as_ref())
            .ok_or_else(|| validation("reconstructed Asset is missing after reopen"))?;
        let payload = readback
            .entities
            .iter()
            .find(|entity| entity.key == rebuild.payload_atom)
            .and_then(|entity| entity.content.as_ref())
            .ok_or_else(|| validation("reconstructed payload is missing after reopen"))?;
        let asset_content = readback_store.read_content(asset)?;
        let payload_content = readback_store.read_content(payload)?;
        let node_content = readback
            .entities
            .iter()
            .find(|entity| entity.key == rebuild.node_atom)
            .and_then(|entity| entity.content.as_ref())
            .ok_or_else(|| validation("rebuild source Node is missing after reopen"))
            .and_then(|content| readback_store.read_content(content))?;
        let node = manifest
            .nodes
            .iter()
            .find(|node| node.node.atom == rebuild.node_atom)
            .expect("validated rebuild Node");
        let derived = symbol(&readback, &manifest.vocabulary.asset.derived_from)?;
        let uses_mapping = symbol(&readback, &manifest.vocabulary.asset.uses_mapping)?;
        let has_payload = symbol(&readback, &manifest.vocabulary.asset.has_payload)?;
        let links_read_back = [
            (rebuild.node_atom, derived),
            (manifest.mapping.atom, uses_mapping),
            (rebuild.payload_atom, has_payload),
        ]
        .into_iter()
        .all(|(target, predicate)| {
            readback.relations.iter().any(|relation| {
                relation.source == rebuild.asset_atom
                    && relation.target == target
                    && relation.predicate == predicate
            })
        });
        let invalidation_read_back = if let Some(previous) = rebuild.previous_asset {
            let predicate = symbol(&readback, &manifest.vocabulary.asset.invalidated_by)?;
            readback.relations.iter().any(|relation| {
                relation.source == previous
                    && relation.target == rebuild.asset_atom
                    && relation.predicate == predicate
            })
        } else {
            true
        };
        if asset_content.get("asset_id").and_then(Value::as_str) != Some(rebuild.asset_id.as_str())
            || payload_content
                .get("value")
                .and_then(|value| canonical_hash(value).ok())
                .as_deref()
                != Some(rebuild.payload_sha256.as_str())
            || node_content.get("content_sha256").and_then(Value::as_str)
                != Some(node.node.content_sha256.as_str())
            || node_content.get("content") != Some(&node.node.content)
            || !links_read_back
            || !invalidation_read_back
        {
            return Err(validation(
                "reconstructed Asset failed independent readback",
            ));
        }
        reconstruction_readback_count += 1;
    }
    if reconstruction_readback_count != reconstruction_count {
        return Err(validation(
            "not every reconstruction was independently read back",
        ));
    }
    let readback_snapshot_hash = readback.canonical_hash()?;
    let receipt = InventoryBatchReceipt {
        kind: "asset_inventory_receipt".into(),
        inventory_id: manifest.policy.inventory_id.clone(),
        batch_id: batch.batch_id.clone(),
        epistemic_state: "measured".into(),
        input_cursor,
        next_cursor,
        limit: manifest.policy.batch_limit,
        processed: observations.len(),
        observations,
        state_counts,
        class_counts,
        reconstruction_count,
        reconstruction_readback_count,
        readback_completed: true,
        readback_snapshot_hash,
    };
    let receipt_value = serde_json::to_value(&receipt)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    let receipt_ref = readback_store.append_content(&receipt_value)?;
    let receipt_symbol = symbol(&readback, &manifest.vocabulary.asset.receipt)?;
    let has_receipt = symbol(&readback, &manifest.vocabulary.asset.has_receipt)?;
    let tick = Tick(readback.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(
        &readback,
        UniverseWriteSet {
            base_revision: readback.revision,
            idempotency_key: format!("asset-inventory-receipt:{}", batch.batch_id),
            causal_ancestry: vec![manifest.policy.inventory_id.clone()],
            commands: vec![
                UniverseCommand::PutEntity {
                    entity: EntityRecord {
                        key: batch.receipt_atom,
                        generation: 0,
                        symbol: receipt_symbol,
                        content: Some(receipt_ref),
                    },
                },
                UniverseCommand::PutRelation {
                    relation: RelationRecord {
                        key: batch.receipt_relation,
                        generation: 0,
                        source: manifest.policy.atom,
                        target: batch.receipt_atom,
                        predicate: has_receipt,
                        content: None,
                    },
                },
            ],
        },
    )?;
    transaction.commit(&readback_store, &mut readback, tick)?;
    let final_store = UniverseStore::open(store_root)?;
    let final_snapshot = final_store.replay(final_store.load_snapshot()?)?;
    let final_receipt = final_snapshot
        .entities
        .iter()
        .find(|entity| entity.key == batch.receipt_atom)
        .and_then(|entity| entity.content.as_ref())
        .ok_or_else(|| validation("inventory receipt is missing after final reopen"))?;
    if final_store.read_content(final_receipt)? != receipt_value {
        return Err(validation("inventory receipt differs after final reopen"));
    }
    Ok((receipt, false))
}

pub fn run_inventory(
    manifest: &InventoryManifest,
    store_root: impl AsRef<Path>,
) -> Result<InventoryEvidence, UniverseError> {
    validate_inventory_manifest(manifest)?;
    let store_root = store_root.as_ref();
    let store = UniverseStore::open(store_root)?;
    if !store_root.join("snapshot.json").exists() {
        store.install_seed(&materialize_inventory_seed(manifest)?)?;
    }
    let start = store.replay(store.load_snapshot()?)?;
    if start.universe != manifest.universe
        || !start
            .entities
            .iter()
            .any(|entity| entity.key == manifest.policy.atom)
    {
        return Err(validation(
            "inventory cannot resume against another authority",
        ));
    }

    let mut cursor = None;
    let mut receipts = Vec::new();
    let mut idempotent_batch_count = 0;
    for batch in &manifest.batches {
        let (receipt, idempotent) = process_batch(manifest, store_root, batch, cursor)?;
        cursor = receipt.next_cursor;
        receipts.push(receipt);
        idempotent_batch_count += usize::from(idempotent);
        if cursor.is_none() {
            break;
        }
    }
    if cursor.is_some() {
        return Err(validation(
            "inventory exhausted receipt batches before its subjects",
        ));
    }
    let completed = UniverseStore::open(store_root)?
        .replay(UniverseStore::open(store_root)?.load_snapshot()?)?;
    let revision_before_replay = completed.revision;

    let mut replay_cursor = None;
    let mut replayed = 0;
    for batch in &manifest.batches[..receipts.len()] {
        let (receipt, idempotent) = process_batch(manifest, store_root, batch, replay_cursor)?;
        if !idempotent {
            return Err(validation("inventory replay produced a new mutation"));
        }
        replay_cursor = receipt.next_cursor;
        replayed += 1;
    }
    let final_store = UniverseStore::open(store_root)?;
    let final_snapshot = final_store.replay(final_store.load_snapshot()?)?;
    let mut total_state_counts = empty_counts();
    let mut total_class_counts = empty_class_counts(manifest);
    let mut total_processed = 0;
    let mut total_reconstructed = 0;
    for receipt in &receipts {
        total_processed += receipt.processed;
        total_reconstructed += receipt.reconstruction_count;
        for (state, count) in &receipt.state_counts {
            *total_state_counts.get_mut(state).expect("known state") += count;
        }
        for (class, count) in &receipt.class_counts {
            *total_class_counts.get_mut(class).expect("known class") += count;
        }
    }
    Ok(InventoryEvidence {
        inventory_id: manifest.policy.inventory_id.clone(),
        observed_status: "measured".into(),
        batch_limit: manifest.policy.batch_limit,
        batch_receipts: receipts,
        total_state_counts,
        total_class_counts,
        total_processed,
        total_reconstructed,
        next_cursor: replay_cursor,
        cursor_published_only_after_readback: true,
        replay_revision_unchanged: final_snapshot.revision == revision_before_replay,
        idempotent_batch_count: idempotent_batch_count + replayed,
        final_revision: final_snapshot.revision,
        final_tick: final_snapshot.tick,
        final_snapshot_hash: final_snapshot.canonical_hash()?,
    })
}

fn observed_asset_entities(manifest: &InventoryManifest, asset: &ObservedAsset) -> Vec<SeedEntity> {
    let (payload_value, asset_value) = asset_record_values(
        manifest,
        asset.asset_atom,
        asset.payload_atom,
        asset.node_atom,
        asset.source_node_revision,
        asset.mapping_revision,
        asset.asset_version,
        &asset.asset_id,
        &asset.payload,
        &asset.payload_sha256,
        "observed",
    );
    vec![
        entity(
            asset.payload_atom,
            &manifest.vocabulary.asset.payload,
            payload_value,
        ),
        entity(
            asset.asset_atom,
            &manifest.vocabulary.asset.asset,
            asset_value,
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn asset_record_values(
    manifest: &InventoryManifest,
    _asset_atom: EntityKey,
    _payload_atom: EntityKey,
    node_atom: EntityKey,
    node_revision: u64,
    mapping_revision: u64,
    asset_version: u64,
    asset_id: &str,
    payload: &Value,
    payload_sha256: &str,
    lifecycle: &str,
) -> (Value, Value) {
    (
        json!({
            "kind": "asset_payload",
            "content_address": format!("sha256:{payload_sha256}"),
            "payload_sha256": payload_sha256,
            "media_type": manifest.mapping.media_type,
            "value": payload,
        }),
        json!({
            "kind": "asset_projection",
            "asset_id": asset_id,
            "asset_version": asset_version,
            "content_address": format!("sha256:{payload_sha256}"),
            "payload_sha256": payload_sha256,
            "source_node": node_atom,
            "source_node_revision": node_revision,
            "mapping": manifest.mapping.atom,
            "mapping_revision": mapping_revision,
            "mapping_configuration_sha256": manifest.mapping.configuration_sha256,
            "lifecycle": lifecycle,
            "canonical_node_replaced": false,
        }),
    )
}

fn asset_relations(
    next: &mut u128,
    asset: EntityKey,
    payload: EntityKey,
    node: EntityKey,
    mapping: EntityKey,
    vocabulary: &AssetVocabulary,
) -> Vec<SeedRelation> {
    vec![
        relation(next, asset, node, &vocabulary.derived_from),
        relation(next, asset, mapping, &vocabulary.uses_mapping),
        relation(next, asset, payload, &vocabulary.has_payload),
    ]
}

fn entity(key: EntityKey, symbol: &str, content: Value) -> SeedEntity {
    SeedEntity {
        key,
        generation: 0,
        symbol: symbol.into(),
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
        predicate: predicate.into(),
        content: None,
    };
    *next += 1;
    result
}

fn symbol(snapshot: &UniverseSnapshot, value: &str) -> Result<u32, UniverseError> {
    snapshot
        .symbol_id(value)
        .ok_or_else(|| validation(format!("inventory symbol {value} is absent")))
}

fn empty_counts() -> BTreeMap<String, usize> {
    STATES
        .into_iter()
        .map(|state| (state.to_owned(), 0))
        .collect()
}

fn empty_class_counts(manifest: &InventoryManifest) -> BTreeMap<String, usize> {
    manifest
        .policy
        .class_labels
        .iter()
        .map(|class| (class.clone(), 0))
        .collect()
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> InventoryManifest {
        load_inventory_manifest(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/assets/node-asset-inventory.json"),
        )
        .unwrap()
    }

    #[test]
    fn bounded_inventory_rebuilds_then_publishes_resumable_receipts() {
        let manifest = fixture();
        let temp = tempfile::tempdir().unwrap();
        let evidence = run_inventory(&manifest, temp.path()).unwrap();

        assert_eq!(evidence.batch_limit, 3);
        assert_eq!(evidence.batch_receipts.len(), 3);
        assert_eq!(evidence.total_processed, 8);
        assert_eq!(evidence.total_reconstructed, 4);
        assert_eq!(evidence.total_state_counts["current"], 1);
        assert_eq!(evidence.total_state_counts["stale"], 2);
        for state in ["missing", "corrupt", "duplicate", "orphaned", "unknown"] {
            assert_eq!(evidence.total_state_counts[state], 1);
        }
        // Conversion classes derived from the graph-owned policy mapping:
        // current->converted, stale/duplicate->partial, missing/corrupt->blocked,
        // orphaned + requirement-unknown->unknown. No Node declares itself
        // intentionally_assetless in this pilot, so that class stays measured 0.
        assert_eq!(evidence.total_class_counts["converted"], 1);
        assert_eq!(evidence.total_class_counts["partial"], 3);
        assert_eq!(evidence.total_class_counts["blocked"], 2);
        assert_eq!(evidence.total_class_counts["unknown"], 2);
        assert_eq!(evidence.total_class_counts["intentionally_assetless"], 0);
        assert_eq!(
            evidence.total_class_counts.values().sum::<usize>(),
            evidence.total_processed
        );
        for receipt in &evidence.batch_receipts {
            assert!(receipt
                .observations
                .iter()
                .all(|observation| !observation.conversion_class.is_empty()));
        }
        assert_eq!(
            evidence.batch_receipts[0].next_cursor,
            Some(EntityKey(0x7002))
        );
        assert_eq!(
            evidence.batch_receipts[1].input_cursor,
            Some(EntityKey(0x7002))
        );
        assert_eq!(
            evidence.batch_receipts[1].next_cursor,
            Some(EntityKey(0x7005))
        );
        assert_eq!(
            evidence.batch_receipts[2].input_cursor,
            Some(EntityKey(0x7005))
        );
        assert_eq!(evidence.batch_receipts[2].next_cursor, None);
        assert!(evidence
            .batch_receipts
            .iter()
            .all(|receipt| receipt.readback_completed
                && receipt.reconstruction_count == receipt.reconstruction_readback_count));
        assert!(evidence.cursor_published_only_after_readback);
        assert!(evidence.replay_revision_unchanged);
        assert_eq!(evidence.idempotent_batch_count, 3);
        assert_eq!(evidence.final_revision, Revision(6));
    }

    #[test]
    fn completed_inventory_replay_is_idempotent() {
        let manifest = fixture();
        let temp = tempfile::tempdir().unwrap();
        let first = run_inventory(&manifest, temp.path()).unwrap();
        let second = run_inventory(&manifest, temp.path()).unwrap();

        assert_eq!(second.final_revision, first.final_revision);
        assert_eq!(second.final_tick, first.final_tick);
        assert_eq!(second.final_snapshot_hash, first.final_snapshot_hash);
        assert_eq!(second.idempotent_batch_count, 6);
    }

    #[test]
    fn unbounded_inventory_is_rejected_before_store_install() {
        let mut manifest = fixture();
        manifest.policy.batch_limit = MAX_INVENTORY_BATCH + 1;
        assert!(matches!(
            validate_inventory_manifest(&manifest),
            Err(UniverseError::Validation(message))
                if message == "inventory cursor or batch bound is unsupported"
        ));
    }

    #[test]
    fn missing_graph_owned_rebuild_publishes_no_cursor() {
        let mut manifest = fixture();
        manifest
            .rebuilds
            .retain(|rebuild| rebuild.node_atom != EntityKey(0x7002));
        let temp = tempfile::tempdir().unwrap();
        assert!(matches!(
            run_inventory(&manifest, temp.path()),
            Err(UniverseError::Validation(message))
                if message == "rebuild policy has no graph-owned projection"
        ));
        let store = UniverseStore::open(temp.path()).unwrap();
        let snapshot = store.replay(store.load_snapshot().unwrap()).unwrap();
        assert_eq!(snapshot.revision, Revision(0));
        assert!(manifest.batches.iter().all(|batch| !snapshot
            .entities
            .iter()
            .any(|entity| entity.key == batch.receipt_atom)));
    }
}
