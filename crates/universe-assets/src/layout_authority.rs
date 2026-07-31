//! Spatial-layout policy — the graph-native authority governing 3D projection.
//!
//! The layout KERNEL (`layout.rs`) computes positions; this module makes its
//! *governing policy* graph-native, closing the same drift `visual.rs` and
//! `actor_control.rs` closed: the policy that decides how positions are derived
//! (force parameters, the containment predicate, the layer rule, the membrane
//! convention) becomes a content-addressed Asset in the store, validated by the
//! SAME kernel rules, and read back byte-for-byte.
//!
//! Two layers, deliberately separated (mirrors `visual.rs`):
//! - **Durable** (this module): the reusable `spatial-layout/1` POLICY,
//!   materialized as a Node→Asset projection and read back independently.
//! - **Live** (projection time, in the bin/app): the per-node POSITIONS =
//!   policy ⊕ the current graph structure. NEVER persisted as an Asset (it would
//!   be one Asset per frame); it is computed by the layout kernel at draw time.
//!
//! The positions are a *computed* projection (a function of Space containment,
//! link attributes, clusters and layers), so — like the visual embodiment — the
//! Node stays authoritative and the Asset is derived, never a replacement.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use universe_core::{EntityKey, RelationKey, UniverseError, UniverseId};
use universe_store::{canonical_hash, GraphSeed, SeedEntity, SeedRelation, UniverseStore};

use crate::layout::{self, LayoutParams};

const SCHEMA_VERSION: &str = "spatial-layout/1";

const UNIVERSE: UniverseId = UniverseId(0x7500);
const CONTRACT_ATOM: EntityKey = EntityKey(0x7501);
const CHANGESET_ATOM: EntityKey = EntityKey(0x7502);
const MAPPING_ATOM: EntityKey = EntityKey(0x7510);
const CATALOG_ATOM: EntityKey = EntityKey(0x7511);
const RELATION_BASE: u128 = 0x7600;

const CHANGE_ID: &str = "spatial-layout-policy-materialization-v0";
const CONTRACT_ID: &str = "spatial-layout-projection-contract-v0";
const AUTHORITY: &str = "graph_first_spatial_layout_authority";
const STATUS: &str = "approved_for_projection";

// ---------------------------------------------------------------------------
// Policy document (graph-declared authority, loaded from the fixture).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpatialLayoutPolicy {
    pub authority_id: String,
    /// The `spatial-layout/1` document, preserved verbatim so the graph Asset is
    /// byte-identical to what the projection consumes.
    pub document: Value,
}

impl SpatialLayoutPolicy {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, UniverseError> {
        let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
        let document: Value = serde_json::from_slice(&bytes)
            .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
        let authority_id = string_field(&document, "authority_id")?.to_owned();
        Ok(Self {
            authority_id,
            document,
        })
    }

    pub fn policy_id(&self) -> Result<&str, UniverseError> {
        string_field(&self.document, "policy_id")
    }

    /// Parses the force parameters carried by the policy.
    pub fn params(&self) -> Result<LayoutParams, UniverseError> {
        let params = self
            .document
            .get("params")
            .ok_or_else(|| validation("policy has no params"))?;
        serde_json::from_value(params.clone())
            .map_err(|error| UniverseError::CorruptContent(error.to_string()))
    }
}

fn string_field<'a>(document: &'a Value, field: &str) -> Result<&'a str, UniverseError> {
    document
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| validation(format!("policy has no {field}")))
}

// ---------------------------------------------------------------------------
// Validation — the policy is challenged by the SAME kernel rules.
// ---------------------------------------------------------------------------

pub fn validate_policy(policy: &SpatialLayoutPolicy) -> Result<(), UniverseError> {
    let document = &policy.document;
    if document.get("schema_version").and_then(Value::as_str) != Some(SCHEMA_VERSION) {
        return Err(validation("policy schema_version must be spatial-layout/1"));
    }
    policy.policy_id()?;

    // Containment: at least one predicate builds the Space tree. A policy that
    // names none is a refusal to define a tree, not a silent default.
    let containment = document
        .get("containment_predicates")
        .and_then(Value::as_array)
        .ok_or_else(|| validation("policy has no containment_predicates"))?;
    if containment.is_empty() {
        return Err(validation("policy declares no containment predicate"));
    }
    for predicate in containment {
        if predicate.as_str().map(str::trim).unwrap_or("").is_empty() {
            return Err(validation("containment predicate is empty"));
        }
    }

    // The force parameters must pass the SAME range checks the kernel enforces.
    let params = policy.params()?;
    layout::validate_params(&params)
        .map_err(|error| validation(format!("policy params rejected by kernel: {error}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Materialization + independent readback.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutAuthorityReceipt {
    pub kind: String,
    pub change_id: String,
    pub authority: String,
    pub status: String,
    pub universe: UniverseId,
    pub newly_committed: bool,
    pub authority_id: String,
    pub policy_id: String,
    pub catalog_sha256: String,
    /// The policy read back from the store is byte-identical to the fixture the
    /// projection consumes.
    pub catalog_parity: bool,
    pub nodes_preserved: bool,
    /// The materialized params round-trip through the kernel validator.
    pub params_valid: bool,
    pub final_snapshot_hash: String,
}

fn build_seed(policy: &SpatialLayoutPolicy) -> Result<GraphSeed, UniverseError> {
    validate_policy(policy)?;
    let catalog_sha256 = canonical_hash(&policy.document)?;
    let policy_id = policy.policy_id()?.to_owned();

    let symbols = vec![
        "layout_projection_contract".to_owned(),
        "spatial_layout_mapping".to_owned(),
        "spatial_layout_catalog".to_owned(),
        "layout_projection_changeset".to_owned(),
        "GOVERNED_BY".to_owned(),
        "HAS_PAYLOAD".to_owned(),
        "PART_OF".to_owned(),
    ];

    let entities = vec![
        seed_entity(
            CONTRACT_ATOM,
            "layout_projection_contract",
            json!({
                "kind": "layout_projection_contract",
                "contract_id": CONTRACT_ID,
                "output_kind": "spatial_layout",
                "schema_version": SCHEMA_VERSION,
                "node_remains_authoritative": true,
                "asset_is_derived": true,
                "invalidation_signals": ["policy_revision", "params_hash", "containment_predicate_revision", "cluster_revision"],
            }),
        ),
        seed_entity(
            MAPPING_ATOM,
            "spatial_layout_mapping",
            json!({
                "kind": "spatial_layout_mapping",
                "authority_id": policy.authority_id,
                "policy_id": policy_id,
                "output_kind": "spatial_layout",
                "media_type": "application/vnd.mind.spatial-layout+json",
                "catalog_sha256": catalog_sha256,
                "schema_version": SCHEMA_VERSION,
                "canonical_node_replaced": false,
            }),
        ),
        seed_entity(
            CATALOG_ATOM,
            "spatial_layout_catalog",
            json!({
                "kind": "spatial_layout_catalog",
                "content_address": format!("sha256:{catalog_sha256}"),
                "catalog_sha256": catalog_sha256,
                "value": policy.document,
            }),
        ),
        seed_entity(
            CHANGESET_ATOM,
            "layout_projection_changeset",
            json!({
                "kind": "layout_projection_changeset",
                "change_id": CHANGE_ID,
                "authority": AUTHORITY,
                "status": STATUS,
                "contract": CONTRACT_ATOM,
                "mapping": MAPPING_ATOM,
                "scope": [MAPPING_ATOM],
            }),
        ),
    ];

    let relations = vec![
        seed_relation(RELATION_BASE, MAPPING_ATOM, CONTRACT_ATOM, "GOVERNED_BY", None),
        seed_relation(
            RELATION_BASE + 1,
            CHANGESET_ATOM,
            CONTRACT_ATOM,
            "GOVERNED_BY",
            None,
        ),
        seed_relation(RELATION_BASE + 2, MAPPING_ATOM, CATALOG_ATOM, "HAS_PAYLOAD", None),
        seed_relation(RELATION_BASE + 3, MAPPING_ATOM, CHANGESET_ATOM, "PART_OF", None),
    ];

    Ok(GraphSeed {
        universe: UNIVERSE,
        symbols,
        entities,
        relations,
    })
}

pub fn materialize(
    store_root: impl AsRef<Path>,
    policy: &SpatialLayoutPolicy,
) -> Result<LayoutAuthorityReceipt, UniverseError> {
    let store_root = store_root.as_ref();
    let seed = build_seed(policy)?;
    let catalog_sha256 = canonical_hash(&policy.document)?;
    let policy_id = policy.policy_id()?.to_owned();

    let store = UniverseStore::open(store_root)?;
    let newly_committed = !store_root.join("snapshot.json").exists();
    if newly_committed {
        store.install_seed(&seed)?;
    }

    // Independent readback: reopen, replay, verify the policy Asset is present,
    // byte-identical to the fixture, and its params still pass the kernel.
    let readback_store = UniverseStore::open(store_root)?;
    let readback = readback_store.replay(readback_store.load_snapshot()?)?;

    let mapping_entity = readback
        .entities
        .iter()
        .find(|entity| entity.key == MAPPING_ATOM)
        .ok_or_else(|| validation("layout mapping Asset missing after reopen"))?;
    let mapping_content = mapping_entity
        .content
        .as_ref()
        .ok_or_else(|| validation("layout mapping Asset has no content"))
        .and_then(|content| readback_store.read_content(content))?;
    let nodes_preserved =
        mapping_content.get("canonical_node_replaced") == Some(&Value::Bool(false));

    let catalog_entity = readback
        .entities
        .iter()
        .find(|entity| entity.key == CATALOG_ATOM)
        .ok_or_else(|| validation("layout policy payload missing after reopen"))?;
    let catalog_content = catalog_entity
        .content
        .as_ref()
        .ok_or_else(|| validation("layout policy payload has no content"))
        .and_then(|content| readback_store.read_content(content))?;
    let read_document = catalog_content
        .get("value")
        .ok_or_else(|| validation("policy payload has no value"))?;
    let catalog_parity = canonical_hash(read_document)? == catalog_sha256;
    if !catalog_parity {
        return Err(validation("read-back policy differs from the fixture"));
    }

    // The materialized params round-trip through the kernel validator.
    let read_policy = SpatialLayoutPolicy {
        authority_id: policy.authority_id.clone(),
        document: read_document.clone(),
    };
    let params_valid = read_policy
        .params()
        .and_then(|params| {
            layout::validate_params(&params)
                .map_err(|error| validation(format!("read-back params invalid: {error}")))
        })
        .is_ok();
    if !params_valid {
        return Err(validation("read-back params failed kernel validation"));
    }

    Ok(LayoutAuthorityReceipt {
        kind: "spatial_layout_policy_materialization_receipt".into(),
        change_id: CHANGE_ID.into(),
        authority: AUTHORITY.into(),
        status: STATUS.into(),
        universe: readback.universe,
        newly_committed,
        authority_id: policy.authority_id.clone(),
        policy_id,
        catalog_sha256,
        catalog_parity,
        nodes_preserved,
        params_valid,
        final_snapshot_hash: readback.canonical_hash()?,
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

    fn policy() -> SpatialLayoutPolicy {
        SpatialLayoutPolicy::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/assets/spatial-layout-policy.json"),
        )
        .unwrap()
    }

    #[test]
    fn materializes_policy_with_independent_parity() {
        let temp = tempfile::tempdir().unwrap();
        let receipt = materialize(temp.path().join("store"), &policy()).unwrap();
        assert!(receipt.newly_committed);
        assert!(receipt.catalog_parity);
        assert!(receipt.nodes_preserved);
        assert!(receipt.params_valid);
        assert_eq!(receipt.policy_id, "universe-spatial-layout-v1");
    }

    #[test]
    fn rematerialization_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("store");
        let first = materialize(&store, &policy()).unwrap();
        let second = materialize(&store, &policy()).unwrap();
        assert!(first.newly_committed && !second.newly_committed);
        assert_eq!(first.final_snapshot_hash, second.final_snapshot_hash);
    }

    #[test]
    fn out_of_range_params_are_rejected_by_kernel_rules() {
        let mut policy = policy();
        policy.document["params"]["scale_per_descent"] = json!(5.0); // > 1
        assert!(matches!(
            validate_policy(&policy),
            Err(UniverseError::Validation(message)) if message.contains("scale_per_descent")
        ));
    }

    #[test]
    fn a_policy_with_no_containment_is_rejected() {
        let mut policy = policy();
        policy.document["containment_predicates"] = json!([]);
        assert!(matches!(
            validate_policy(&policy),
            Err(UniverseError::Validation(message)) if message.contains("no containment predicate")
        ));
    }
}
