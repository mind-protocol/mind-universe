//! First real Node→Asset conversion.
//!
//! The census reports which canonical Nodes are `blocked` — declared `required`
//! but not yet projected. This module converts those Nodes into content-addressed
//! Asset projections through **one** bounded, attributable, idempotent authorized
//! change, and proves the outcome by independent readback and re-census.
//!
//! Discipline preserved here:
//! - The canonical Node is never replaced or edited; the Asset is derived and
//!   carries `canonical_node_replaced: false`.
//! - The ontology seed authority is untouched — assets are added to the store as
//!   a bounded transaction, not merged into the ontology registry cluster.
//! - The change is one attributable ChangeSet (a graph node) with an idempotency
//!   key; re-running commits nothing new.
//! - Only Nodes the policy declares `required` are converted; a source whose
//!   embedded document does not hash to its declared digest is refused, not
//!   silently projected.

use crate::census::{run_census, CensusPolicy};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};
use universe_core::{EntityKey, RelationKey, Tick, UniverseError, UniverseId};
use universe_store::{
    canonical_hash, EntityRecord, RelationRecord, UniverseSnapshot, UniverseStore,
};
use universe_transactions::{
    CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet,
};

const CONTRACT_ATOM: EntityKey = EntityKey(0xA000);
const MAPPING_ATOM: EntityKey = EntityKey(0xA001);
const CHANGESET_ATOM: EntityKey = EntityKey(0xA002);
const PAYLOAD_BASE: u128 = 0xA100;
const ASSET_BASE: u128 = 0xA110;
const RELATION_BASE: u128 = 0xA200;

const CONTRACT_ID: &str = "canonical-source-conversion-v0";
const MAPPING_ID: &str = "ontology-source-document-v0";
const CHANGE_ID: &str = "asset-conversion-ontology-sources-v0";
const AUTHORITY: &str = "graph_first_conversion_authority";
const STATUS: &str = "approved_for_conversion";

const VOCAB: [&str; 10] = [
    "asset_projection_contract",
    "asset_projection_mapping",
    "asset_conversion_changeset",
    "asset_projection",
    "asset_payload",
    "GOVERNED_BY",
    "DERIVED_FROM",
    "USES_MAPPING",
    "HAS_PAYLOAD",
    "PART_OF",
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConvertedSource {
    pub source_node: EntityKey,
    pub source_node_content_sha256: String,
    pub asset_atom: EntityKey,
    pub payload_atom: EntityKey,
    pub asset_id: String,
    pub payload_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConversionReceipt {
    pub kind: String,
    pub change_id: String,
    pub authority: String,
    pub status: String,
    pub universe: UniverseId,
    /// True when this run appended the change; false when it was already present
    /// (idempotent replay).
    pub newly_committed: bool,
    pub base_revision: u64,
    pub final_revision: u64,
    pub converted: Vec<ConvertedSource>,
    pub nodes_preserved: bool,
    pub assets_read_back: usize,
    pub census_before: BTreeMap<String, usize>,
    pub census_after: BTreeMap<String, usize>,
    pub final_snapshot_hash: String,
}

struct SourceToConvert {
    key: EntityKey,
    content_sha256: String,
    document: Value,
    payload_sha256: String,
}

/// Content-addressed Asset identity, anchored on the exact source Node content
/// hash so a Node revision that changes its content yields a different Asset.
fn asset_id(source: &SourceToConvert) -> Result<String, UniverseError> {
    Ok(format!(
        "sha256:{}",
        canonical_hash(&json!({
            "contract_atom": CONTRACT_ATOM,
            "contract_revision": 1,
            "mapping_atom": MAPPING_ATOM,
            "mapping_revision": 1,
            "source_node": source.key,
            "source_node_content_sha256": source.content_sha256,
            "payload_sha256": source.payload_sha256,
        }))?
    ))
}

fn collect_required_sources(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    policy: &CensusPolicy,
) -> Result<Vec<SourceToConvert>, UniverseError> {
    let mut sources = Vec::new();
    for entity in &snapshot.entities {
        let Some(content_ref) = entity.content.as_ref() else {
            continue;
        };
        let content = store.read_content(content_ref)?;
        if content.get("kind").and_then(Value::as_str) != Some("ontology_source") {
            continue;
        }
        if policy.requirement_for("ontology_source") != "required" {
            continue;
        }
        let document = content
            .get("document")
            .cloned()
            .ok_or_else(|| validation(format!("source {} has no document", entity.key)))?;
        let payload_sha256 = canonical_hash(&document)?;
        // Faithful derivation: the projected payload must reproduce the source's
        // own declared digest, or the Node is refused rather than misprojected.
        if let Some(declared) = content.get("canonical_json_sha256").and_then(Value::as_str) {
            if declared != payload_sha256 {
                return Err(validation(format!(
                    "source {} document does not match its declared canonical_json_sha256",
                    entity.key
                )));
            }
        }
        sources.push(SourceToConvert {
            key: entity.key,
            content_sha256: content_ref.sha256.clone(),
            document,
            payload_sha256,
        });
    }
    sources.sort_by_key(|source| source.key);
    Ok(sources)
}

pub fn convert_sources(
    store_root: impl AsRef<Path>,
    policy: &CensusPolicy,
) -> Result<ConversionReceipt, UniverseError> {
    let store_root = store_root.as_ref();
    let store = UniverseStore::open(store_root)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;
    let census_before = run_census(&store, &snapshot, policy)?.class_counts;

    let sources = collect_required_sources(&store, &snapshot, policy)?;
    if sources.is_empty() {
        return Err(validation("no required source Node to convert"));
    }

    let converted: Vec<ConvertedSource> = sources
        .iter()
        .enumerate()
        .map(|(index, source)| {
            Ok(ConvertedSource {
                source_node: source.key,
                source_node_content_sha256: source.content_sha256.clone(),
                asset_atom: EntityKey(ASSET_BASE + index as u128),
                payload_atom: EntityKey(PAYLOAD_BASE + index as u128),
                asset_id: asset_id(source)?,
                payload_sha256: source.payload_sha256.clone(),
            })
        })
        .collect::<Result<_, UniverseError>>()?;

    let already = snapshot.event_keys.contains(CHANGE_ID);
    if !already {
        let plan = snapshot.plan_symbol_interning(
            &VOCAB
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>(),
        )?;
        let sym = |name: &str| -> Result<u32, UniverseError> {
            plan.assignments
                .get(name)
                .copied()
                .ok_or_else(|| validation(format!("symbol {name} was not planned")))
        };

        let mut commands = Vec::new();
        if !plan.additions.is_empty() {
            commands.push(UniverseCommand::InternSymbols {
                symbols: plan.additions.clone(),
            });
        }
        commands.push(put_entity(
            CONTRACT_ATOM,
            sym("asset_projection_contract")?,
            &store,
            json!({
                "kind": "asset_projection_contract",
                "contract_id": CONTRACT_ID,
                "contract_revision": 1,
                "hash_contract": "sha256:canonical-json-v0",
                "node_remains_authoritative": true,
                "asset_is_derived": true,
                "invalidation_signals": ["source_node_content_sha256", "mapping_revision", "payload_hash"],
            }),
        )?);
        commands.push(put_entity(
            MAPPING_ATOM,
            sym("asset_projection_mapping")?,
            &store,
            json!({
                "kind": "asset_projection_mapping",
                "mapping_id": MAPPING_ID,
                "revision": 1,
                "output_kind": "ontology_source_document",
                "media_type": "application/json",
            }),
        )?);
        commands.push(put_entity(
            CHANGESET_ATOM,
            sym("asset_conversion_changeset")?,
            &store,
            json!({
                "kind": "asset_conversion_changeset",
                "change_id": CHANGE_ID,
                "authority": AUTHORITY,
                "status": STATUS,
                "contract": CONTRACT_ATOM,
                "mapping": MAPPING_ATOM,
                "scope": converted.iter().map(|c| c.source_node).collect::<Vec<_>>(),
            }),
        )?);

        for (converted_source, source) in converted.iter().zip(&sources) {
            commands.push(put_entity(
                converted_source.payload_atom,
                sym("asset_payload")?,
                &store,
                json!({
                    "kind": "asset_payload",
                    "content_address": format!("sha256:{}", source.payload_sha256),
                    "payload_sha256": source.payload_sha256,
                    "media_type": "application/json",
                    "value": source.document,
                }),
            )?);
            commands.push(put_entity(
                converted_source.asset_atom,
                sym("asset_projection")?,
                &store,
                json!({
                    "kind": "asset_projection",
                    "asset_id": converted_source.asset_id,
                    "asset_version": 1,
                    "content_address": format!("sha256:{}", source.payload_sha256),
                    "payload_sha256": source.payload_sha256,
                    "source_node": source.key,
                    "source_node_content_sha256": source.content_sha256,
                    "mapping": MAPPING_ATOM,
                    "mapping_revision": 1,
                    "lifecycle": "current",
                    "canonical_node_replaced": false,
                }),
            )?);
        }

        let mut relation_key = RELATION_BASE;
        let mut relations = vec![
            (MAPPING_ATOM, CONTRACT_ATOM, sym("GOVERNED_BY")?),
            (CHANGESET_ATOM, CONTRACT_ATOM, sym("GOVERNED_BY")?),
        ];
        for converted_source in &converted {
            relations.push((
                converted_source.asset_atom,
                converted_source.source_node,
                sym("DERIVED_FROM")?,
            ));
            relations.push((
                converted_source.asset_atom,
                MAPPING_ATOM,
                sym("USES_MAPPING")?,
            ));
            relations.push((
                converted_source.asset_atom,
                converted_source.payload_atom,
                sym("HAS_PAYLOAD")?,
            ));
            relations.push((converted_source.asset_atom, CHANGESET_ATOM, sym("PART_OF")?));
        }
        for (source, target, predicate) in relations {
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(relation_key),
                    generation: 0,
                    source,
                    target,
                    predicate,
                    content: None,
                },
            });
            relation_key += 1;
        }

        let transaction = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: snapshot.revision,
                idempotency_key: CHANGE_ID.to_owned(),
                causal_ancestry: vec![
                    CHANGE_ID.to_owned(),
                    CONTRACT_ID.to_owned(),
                    MAPPING_ID.to_owned(),
                ],
                commands,
            },
        )?;
        let tick = Tick(snapshot.tick.0 + 1);
        let receipt = transaction.commit(&store, &mut snapshot, tick)?;
        if matches!(receipt, CommitReceipt::AlreadyCommitted { .. }) {
            return Err(validation("conversion change key already present mid-run"));
        }
    }

    // Independent readback: reopen the store, replay, and verify the projections
    // exist, derive from their preserved source Nodes, and re-census as converted.
    let readback_store = UniverseStore::open(store_root)?;
    let readback = readback_store.replay(readback_store.load_snapshot()?)?;
    let mut nodes_preserved = true;
    let mut assets_read_back = 0;
    for converted_source in &converted {
        let source_entity = readback
            .entities
            .iter()
            .find(|entity| entity.key == converted_source.source_node)
            .ok_or_else(|| validation("source Node vanished after conversion"))?;
        if source_entity.content.as_ref().map(|c| c.sha256.as_str())
            != Some(converted_source.source_node_content_sha256.as_str())
        {
            nodes_preserved = false;
        }

        let asset = readback
            .entities
            .iter()
            .find(|entity| entity.key == converted_source.asset_atom)
            .and_then(|entity| entity.content.as_ref())
            .ok_or_else(|| validation("converted Asset is missing after reopen"))?;
        let payload = readback
            .entities
            .iter()
            .find(|entity| entity.key == converted_source.payload_atom)
            .and_then(|entity| entity.content.as_ref())
            .ok_or_else(|| validation("converted payload is missing after reopen"))?;
        let asset_content = readback_store.read_content(asset)?;
        let payload_content = readback_store.read_content(payload)?;

        let derived = readback.symbol_id("DERIVED_FROM");
        let uses_mapping = readback.symbol_id("USES_MAPPING");
        let has_payload = readback.symbol_id("HAS_PAYLOAD");
        let links = [
            (converted_source.source_node, derived),
            (MAPPING_ATOM, uses_mapping),
            (converted_source.payload_atom, has_payload),
        ]
        .into_iter()
        .all(|(target, predicate)| {
            predicate.is_some_and(|predicate| {
                readback.relations.iter().any(|relation| {
                    relation.source == converted_source.asset_atom
                        && relation.target == target
                        && relation.predicate == predicate
                })
            })
        });

        let payload_ok = payload_content
            .get("value")
            .and_then(|value| canonical_hash(value).ok())
            .as_deref()
            == Some(converted_source.payload_sha256.as_str());
        let asset_ok = asset_content.get("asset_id").and_then(Value::as_str)
            == Some(converted_source.asset_id.as_str())
            && asset_content.get("source_node").and_then(Value::as_str)
                == Some(converted_source.source_node.to_string().as_str())
            && asset_content.get("canonical_node_replaced") == Some(&Value::Bool(false))
            && asset_content.get("lifecycle").and_then(Value::as_str) == Some("current");
        if !links || !payload_ok || !asset_ok {
            return Err(validation("converted Asset failed independent readback"));
        }
        assets_read_back += 1;
    }

    let census_after = run_census(&readback_store, &readback, policy)?.class_counts;
    let n = converted.len();
    let before = |k: &str| *census_before.get(k).unwrap_or(&0);
    let after = |k: &str| *census_after.get(k).unwrap_or(&0);
    // On the committing run the sources move blocked→converted; on an idempotent
    // replay the census already reflects that end state and must not change.
    let census_consistent = if already {
        after("converted") == before("converted")
            && after("blocked") == before("blocked")
            && after("converted") >= n
    } else {
        after("converted") == before("converted") + n
            && after("blocked") == before("blocked").saturating_sub(n)
    };
    if !census_consistent {
        return Err(validation(
            "re-census does not reflect the expected converted/blocked state",
        ));
    }

    Ok(ConversionReceipt {
        kind: "node_asset_conversion_receipt".into(),
        change_id: CHANGE_ID.into(),
        authority: AUTHORITY.into(),
        status: STATUS.into(),
        universe: readback.universe,
        newly_committed: !already,
        base_revision: base_revision.0,
        final_revision: readback.revision.0,
        converted,
        nodes_preserved,
        assets_read_back,
        census_before,
        census_after,
        final_snapshot_hash: readback.canonical_hash()?,
    })
}

fn put_entity(
    key: EntityKey,
    symbol: u32,
    store: &UniverseStore,
    content: Value,
) -> Result<UniverseCommand, UniverseError> {
    let content_ref = store.append_content(&content)?;
    Ok(UniverseCommand::PutEntity {
        entity: EntityRecord {
            key,
            generation: 0,
            symbol,
            content: Some(content_ref),
        },
    })
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_store::load_seed;

    fn policy() -> CensusPolicy {
        CensusPolicy::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/assets/node-asset-census-policy.json"),
        )
        .unwrap()
    }

    fn install_canonical(root: &Path) {
        let seed = load_seed(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/ontology/canonical-ontology.json"),
        )
        .unwrap();
        UniverseStore::open(root)
            .unwrap()
            .install_seed(&seed)
            .unwrap();
    }

    #[test]
    fn blocked_sources_convert_and_read_back() {
        let temp = tempfile::tempdir().unwrap();
        install_canonical(temp.path());
        let policy = policy();
        let receipt = convert_sources(temp.path(), &policy).unwrap();

        assert!(receipt.newly_committed);
        assert_eq!(receipt.converted.len(), 3);
        assert_eq!(receipt.assets_read_back, 3);
        assert!(receipt.nodes_preserved);
        assert_eq!(receipt.census_before["blocked"], 3);
        assert_eq!(receipt.census_before["converted"], 0);
        assert_eq!(receipt.census_after["blocked"], 0);
        assert_eq!(receipt.census_after["converted"], 3);
        assert!(receipt.final_revision > receipt.base_revision);
    }

    #[test]
    fn conversion_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        install_canonical(temp.path());
        let policy = policy();
        let first = convert_sources(temp.path(), &policy).unwrap();
        let second = convert_sources(temp.path(), &policy).unwrap();

        assert!(first.newly_committed);
        assert!(!second.newly_committed);
        assert_eq!(first.final_revision, second.final_revision);
        assert_eq!(first.final_snapshot_hash, second.final_snapshot_hash);
        assert_eq!(second.census_after["converted"], 3);
    }
}
