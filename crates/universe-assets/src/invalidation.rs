//! Live, recorded rebuild/invalidation of an Asset when its authoritative
//! mapping revision changes.
//!
//! The conversion path (`conversion.rs`) projects `current` Assets; the manifest
//! path (`lib.rs`) can *declare* a stale-with-replacement end state. Neither
//! demonstrates the *triggered transition*: an Asset that is currently `current`
//! becoming `stale` because its authority changed. This module does exactly that,
//! over a real store, and records the before/after evidence.
//!
//! Append-only discipline (see `universe-store`: entities are immutable — a key
//! cannot be re-put). So invalidation is expressed *structurally*, never by
//! mutating the old Asset:
//! - the authoritative change is a NEW mapping-revision entity (`mapping_v2`);
//! - the rebuild is a NEW `current` Asset (`asset_v2`) derived from the preserved
//!   Node under `mapping_v2`;
//! - the invalidation is an `INVALIDATED_BY` edge `asset_v1 -> asset_v2`.
//!
//! Effective lifecycle is then *derived* from the graph: an Asset is `stale` iff
//! it has an outgoing `INVALIDATED_BY` edge, `current` otherwise. The Node is
//! never edited; only its Assets change. One attributable, idempotent ChangeSet.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use universe_core::{EntityKey, RelationKey, Tick, UniverseError, UniverseId};
use universe_store::{
    canonical_hash, EntityRecord, GraphSeed, RelationRecord, SeedEntity, SeedRelation,
    UniverseSnapshot, UniverseStore,
};
use universe_transactions::{
    CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet,
};

const UNIVERSE: UniverseId = UniverseId(0x8000);
const NODE_ATOM: EntityKey = EntityKey(0x8001);
const MAPPING_V1_ATOM: EntityKey = EntityKey(0x8002);
const MAPPING_V2_ATOM: EntityKey = EntityKey(0x8003);
const CHANGESET_ATOM: EntityKey = EntityKey(0x8004);
const ASSET_V1_ATOM: EntityKey = EntityKey(0x8010);
const ASSET_V2_ATOM: EntityKey = EntityKey(0x8011);
const PAYLOAD_ATOM: EntityKey = EntityKey(0x8020);
const SEED_RELATION_BASE: u128 = 0x8200;
const CHANGE_RELATION_BASE: u128 = 0x8210;

const CHANGE_ID: &str = "asset-invalidation-mapping-r2-v0";
const AUTHORITY: &str = "graph_first_invalidation_authority";
const STATUS: &str = "approved_for_invalidation";

const SEED_SYMBOLS: [&str; 7] = [
    "canonical_node",
    "asset_projection_mapping",
    "asset_projection",
    "asset_payload",
    "DERIVED_FROM",
    "USES_MAPPING",
    "HAS_PAYLOAD",
];
const CHANGE_SYMBOLS: [&str; 3] = ["asset_invalidation_changeset", "INVALIDATED_BY", "PART_OF"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleAudit {
    pub current: Vec<EntityKey>,
    pub stale: Vec<EntityKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvalidationReceipt {
    pub kind: String,
    pub change_id: String,
    pub authority: String,
    pub status: String,
    pub universe: UniverseId,
    pub newly_committed: bool,
    pub trigger: String,
    /// Effective lifecycle before the change was applied.
    pub before: LifecycleAudit,
    /// Effective lifecycle after the change was applied.
    pub after: LifecycleAudit,
    /// Assets that transitioned `current` -> `stale` on this run.
    pub transitioned_to_stale: Vec<EntityKey>,
    pub node_preserved: bool,
    pub old_asset_id: String,
    pub new_asset_id: String,
    pub base_revision: u64,
    pub final_revision: u64,
    pub final_snapshot_hash: String,
}

fn node_content() -> Value {
    json!({
        "kind": "canonical_node",
        "stable_id": "node:invalidation-pilot",
        "revision": 1,
        "title": "Invalidation pilot node",
        "summary": "A canonical Node whose Asset is rebuilt when the mapping revision advances.",
    })
}

fn mapping_content(revision: u64) -> Value {
    json!({
        "kind": "asset_projection_mapping",
        "mapping_id": "invalidation-pilot-mapping",
        "revision": revision,
        "output_kind": "generic_content_asset",
        "media_type": "application/json",
    })
}

fn payload_content(node: &Value) -> Value {
    json!({
        "kind": "asset_payload",
        "media_type": "application/json",
        "value": node,
    })
}

/// Content-addressed Asset identity, anchored on the source Node content hash and
/// the mapping revision, so a mapping-revision change yields a different Asset.
fn asset_id(
    node_sha: &str,
    payload_sha: &str,
    mapping_revision: u64,
) -> Result<String, UniverseError> {
    Ok(format!(
        "sha256:{}",
        canonical_hash(&json!({
            "source_node": NODE_ATOM,
            "source_node_content_sha256": node_sha,
            "mapping_revision": mapping_revision,
            "payload_sha256": payload_sha,
        }))?
    ))
}

fn asset_content(
    asset_id: &str,
    version: u64,
    mapping: EntityKey,
    mapping_revision: u64,
    node_sha: &str,
    payload_sha: &str,
) -> Value {
    json!({
        "kind": "asset_projection",
        "asset_id": asset_id,
        "asset_version": version,
        "source_node": NODE_ATOM,
        "source_node_content_sha256": node_sha,
        "mapping": mapping,
        "mapping_revision": mapping_revision,
        "payload_sha256": payload_sha,
        "lifecycle": "current",
        "canonical_node_replaced": false,
    })
}

/// Effective lifecycle derived from the graph: an Asset is `stale` iff it has an
/// outgoing `INVALIDATED_BY` edge (it has been superseded), `current` otherwise.
pub fn audit_lifecycle(snapshot: &UniverseSnapshot) -> LifecycleAudit {
    let asset_symbol = snapshot.symbol_id("asset_projection");
    let invalidated_by = snapshot.symbol_id("INVALIDATED_BY");
    let mut audit = LifecycleAudit::default();
    for entity in &snapshot.entities {
        if Some(entity.symbol) != asset_symbol {
            continue;
        }
        let superseded = invalidated_by.is_some_and(|predicate| {
            snapshot
                .relations
                .iter()
                .any(|relation| relation.source == entity.key && relation.predicate == predicate)
        });
        if superseded {
            audit.stale.push(entity.key);
        } else {
            audit.current.push(entity.key);
        }
    }
    audit.current.sort();
    audit.stale.sort();
    audit
}

fn build_seed() -> Result<(GraphSeed, String, String, String), UniverseError> {
    let node = node_content();
    let node_sha = canonical_hash(&node)?;
    let payload = payload_content(&node);
    let payload_sha = canonical_hash(&payload["value"])?;
    let v1_id = asset_id(&node_sha, &payload_sha, 1)?;

    let entities = vec![
        seed_entity(NODE_ATOM, "canonical_node", node),
        seed_entity(
            MAPPING_V1_ATOM,
            "asset_projection_mapping",
            mapping_content(1),
        ),
        seed_entity(PAYLOAD_ATOM, "asset_payload", payload),
        seed_entity(
            ASSET_V1_ATOM,
            "asset_projection",
            asset_content(&v1_id, 1, MAPPING_V1_ATOM, 1, &node_sha, &payload_sha),
        ),
    ];
    let relations = vec![
        seed_relation(
            SEED_RELATION_BASE,
            ASSET_V1_ATOM,
            NODE_ATOM,
            "DERIVED_FROM",
            None,
        ),
        seed_relation(
            SEED_RELATION_BASE + 1,
            ASSET_V1_ATOM,
            MAPPING_V1_ATOM,
            "USES_MAPPING",
            None,
        ),
        seed_relation(
            SEED_RELATION_BASE + 2,
            ASSET_V1_ATOM,
            PAYLOAD_ATOM,
            "HAS_PAYLOAD",
            None,
        ),
    ];

    let seed = GraphSeed {
        universe: UNIVERSE,
        symbols: SEED_SYMBOLS.iter().map(|s| s.to_string()).collect(),
        entities,
        relations,
    };
    Ok((seed, node_sha, payload_sha, v1_id))
}

pub fn run_invalidation(
    store_root: impl AsRef<Path>,
) -> Result<InvalidationReceipt, UniverseError> {
    let store_root = store_root.as_ref();
    let (seed, node_sha, payload_sha, old_asset_id) = build_seed()?;
    let new_asset_id = asset_id(&node_sha, &payload_sha, 2)?;

    let store = UniverseStore::open(store_root)?;
    if !store_root.join("snapshot.json").exists() {
        store.install_seed(&seed)?;
    }

    // Reopen independently and observe the pre-change lifecycle: v1 is current,
    // nothing is stale yet.
    let pre_store = UniverseStore::open(store_root)?;
    let mut snapshot = pre_store.replay(pre_store.load_snapshot()?)?;
    let before = audit_lifecycle(&snapshot);
    let base_revision = snapshot.revision;

    let already = snapshot.event_keys.contains(CHANGE_ID);
    if !already {
        let plan = snapshot.plan_symbol_interning(
            &CHANGE_SYMBOLS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )?;
        let sym = |name: &str| -> Result<u32, UniverseError> {
            snapshot
                .symbol_id(name)
                .or_else(|| plan.assignments.get(name).copied())
                .ok_or_else(|| validation(format!("symbol {name} was not planned")))
        };

        let mut commands = Vec::new();
        if !plan.additions.is_empty() {
            commands.push(UniverseCommand::InternSymbols {
                symbols: plan.additions.clone(),
            });
        }
        // The authoritative change: a new mapping revision entity.
        commands.push(put_entity(
            MAPPING_V2_ATOM,
            sym("asset_projection_mapping")?,
            &pre_store,
            mapping_content(2),
        )?);
        // The changeset that attributes this bounded invalidation.
        commands.push(put_entity(
            CHANGESET_ATOM,
            sym("asset_invalidation_changeset")?,
            &pre_store,
            json!({
                "kind": "asset_invalidation_changeset",
                "change_id": CHANGE_ID,
                "authority": AUTHORITY,
                "status": STATUS,
                "trigger": "mapping_revision_changed",
                "from_mapping_revision": 1,
                "to_mapping_revision": 2,
                "invalidates": [ASSET_V1_ATOM],
                "rebuilds": [ASSET_V2_ATOM],
            }),
        )?);
        // The rebuilt current Asset under mapping revision 2 (payload unchanged;
        // its identity still differs because the mapping revision changed).
        commands.push(put_entity(
            ASSET_V2_ATOM,
            sym("asset_projection")?,
            &pre_store,
            asset_content(
                &new_asset_id,
                2,
                MAPPING_V2_ATOM,
                2,
                &node_sha,
                &payload_sha,
            ),
        )?);

        let mut relation_key = CHANGE_RELATION_BASE;
        let mut relations = vec![
            (ASSET_V2_ATOM, NODE_ATOM, sym("DERIVED_FROM")?),
            (ASSET_V2_ATOM, MAPPING_V2_ATOM, sym("USES_MAPPING")?),
            (ASSET_V2_ATOM, PAYLOAD_ATOM, sym("HAS_PAYLOAD")?),
            (ASSET_V2_ATOM, CHANGESET_ATOM, sym("PART_OF")?),
            // The invalidation edge: v1 is superseded by v2.
            (ASSET_V1_ATOM, ASSET_V2_ATOM, sym("INVALIDATED_BY")?),
        ];
        for (source, target, predicate) in relations.drain(..) {
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
                commands,
            },
        )?;
        let tick = Tick(snapshot.tick.0 + 1);
        if matches!(
            transaction.commit(&pre_store, &mut snapshot, tick)?,
            CommitReceipt::AlreadyCommitted { .. }
        ) {
            return Err(validation(
                "invalidation change key already present mid-run",
            ));
        }
    }

    // Independent readback: reopen, replay, and observe the post-change lifecycle.
    let readback_store = UniverseStore::open(store_root)?;
    let readback = readback_store.replay(readback_store.load_snapshot()?)?;
    let after = audit_lifecycle(&readback);

    // The transition: Assets that were current before and are stale now.
    let transitioned_to_stale: Vec<EntityKey> = before
        .current
        .iter()
        .filter(|key| after.stale.contains(key))
        .copied()
        .collect();

    // The canonical Node is preserved: still present, content unchanged.
    let node_entity = readback
        .entities
        .iter()
        .find(|entity| entity.key == NODE_ATOM)
        .ok_or_else(|| validation("node vanished after invalidation"))?;
    let node_readback = node_entity
        .content
        .as_ref()
        .ok_or_else(|| validation("node has no content"))
        .and_then(|content| readback_store.read_content(content))?;
    let node_preserved = node_readback == node_content();

    // Structural expectations under readback. The committing run transitions
    // v1 current->stale; an idempotent replay already reflects that end state and
    // transitions nothing further.
    let expected_transition: Vec<EntityKey> = if already {
        Vec::new()
    } else {
        vec![ASSET_V1_ATOM]
    };
    if after.current != vec![ASSET_V2_ATOM]
        || after.stale != vec![ASSET_V1_ATOM]
        || transitioned_to_stale != expected_transition
        || !node_preserved
    {
        return Err(validation(format!(
            "invalidation readback did not resolve to one rebuilt current and one superseded Asset: {after:?}"
        )));
    }

    Ok(InvalidationReceipt {
        kind: "asset_invalidation_receipt".into(),
        change_id: CHANGE_ID.into(),
        authority: AUTHORITY.into(),
        status: STATUS.into(),
        universe: readback.universe,
        newly_committed: !already,
        trigger: "mapping_revision 1 -> 2".into(),
        before,
        after,
        transitioned_to_stale,
        node_preserved,
        old_asset_id,
        new_asset_id,
        base_revision: base_revision.0,
        final_revision: readback.revision.0,
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

fn seed_entity(key: EntityKey, symbol: &str, content: Value) -> SeedEntity {
    SeedEntity {
        key,
        generation: 0,
        symbol: symbol.to_owned(),
        content,
    }
}

fn seed_relation(
    key: u128,
    source: EntityKey,
    target: EntityKey,
    predicate: &str,
    content: Option<Value>,
) -> SeedRelation {
    SeedRelation {
        key: RelationKey(key),
        generation: 0,
        source,
        target,
        predicate: predicate.to_owned(),
        content,
    }
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_asset_transitions_to_stale_on_mapping_revision_change() {
        let temp = tempfile::tempdir().unwrap();
        let receipt = run_invalidation(temp.path().join("store")).unwrap();

        assert!(receipt.newly_committed);
        assert_eq!(receipt.before.current, vec![ASSET_V1_ATOM]);
        assert!(receipt.before.stale.is_empty());
        assert_eq!(receipt.after.current, vec![ASSET_V2_ATOM]);
        assert_eq!(receipt.after.stale, vec![ASSET_V1_ATOM]);
        assert_eq!(receipt.transitioned_to_stale, vec![ASSET_V1_ATOM]);
        assert!(receipt.node_preserved);
        assert_ne!(receipt.old_asset_id, receipt.new_asset_id);
        assert!(receipt.final_revision > receipt.base_revision);
    }

    #[test]
    fn invalidation_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("store");
        let first = run_invalidation(&root).unwrap();
        let second = run_invalidation(&root).unwrap();
        assert!(first.newly_committed);
        assert!(!second.newly_committed);
        assert_eq!(first.final_revision, second.final_revision);
        assert_eq!(first.final_snapshot_hash, second.final_snapshot_hash);
        assert_eq!(second.after.stale, vec![ASSET_V1_ATOM]);
        assert_eq!(second.after.current, vec![ASSET_V2_ATOM]);
    }
}
