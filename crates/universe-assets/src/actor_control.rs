//! Actor-control bounds — the graph-native authority for avatar piloting.
//!
//! The Mind Desktop renderer defines and validates an `actor-control/1` bounds
//! contract (`apps/mind-desktop/src/actor-control.ts`) but, until now, the only
//! instance was a fixture the app read directly. This module closes that
//! graph-first drift the same way `visual.rs` did for the visual embodiment
//! mapping: the bounds document becomes a content-addressed Asset in the store,
//! bound to the piloted Actor by an explicit `PROJECTS_AS` relation, validated
//! by the SAME rules the renderer enforces, and read back independently.
//!
//! Two layers, deliberately separated:
//! - **Durable** (this module): the reusable motion-bounds contract materialized
//!   as a Node→Asset projection, validated and read back byte-for-byte.
//! - **Live** (render time, in the app): per-frame gated intent = bounds ⊕ the
//!   ControlState gate ⊕ the camera basis. Never persisted (it would be one
//!   Asset per keystroke); it is composed by ActorControls at draw time.
//!
//! The fixture stays the single source of truth: the renderer consumes exactly
//! the bytes this materialization projects.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use universe_core::{EntityKey, RelationKey, UniverseError, UniverseId};
use universe_store::{canonical_hash, GraphSeed, SeedEntity, SeedRelation, UniverseStore};

const SCHEMA_VERSION: &str = "actor-control/1";
/// The only gate rule the runtime implements: motion is authorized only for a
/// `granted` control over the bound Actor. Mirrors `gateIntent` in the renderer.
const ALLOWED_GATE_RULES: [&str; 1] = ["granted_bound_actor_only"];

const UNIVERSE: UniverseId = UniverseId(0x7300);
const CONTRACT_ATOM: EntityKey = EntityKey(0x7301);
const CHANGESET_ATOM: EntityKey = EntityKey(0x7302);
const MAPPING_ATOM: EntityKey = EntityKey(0x7310);
const CATALOG_ATOM: EntityKey = EntityKey(0x7311);
const ACTOR_ATOM: EntityKey = EntityKey(0x7320);
const RELATION_BASE: u128 = 0x7400;

const CHANGE_ID: &str = "actor-control-bounds-materialization-v0";
const CONTRACT_ID: &str = "actor-control-projection-contract-v0";
const AUTHORITY: &str = "graph_first_actor_control_authority";
const STATUS: &str = "approved_for_projection";

// ---------------------------------------------------------------------------
// Bounds document (graph-declared authority, loaded from the fixture).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActorControlBounds {
    pub authority_id: String,
    /// The `actor-control/1` document, preserved verbatim so the graph Asset is
    /// byte-identical to what the renderer consumes.
    pub document: Value,
}

impl ActorControlBounds {
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

    pub fn bounds_id(&self) -> Result<&str, UniverseError> {
        string_field(&self.document, "bounds_id")
    }

    pub fn bound_actor(&self) -> Result<&str, UniverseError> {
        string_field(&self.document, "bound_actor")
    }

    pub fn gate_rule(&self) -> Result<&str, UniverseError> {
        string_field(&self.document, "gate_rule")
    }
}

fn string_field<'a>(document: &'a Value, field: &str) -> Result<&'a str, UniverseError> {
    document
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| validation(format!("bounds document has no {field}")))
}

// ---------------------------------------------------------------------------
// Validation — mirrors the renderer's `validateMotionBounds`.
// ---------------------------------------------------------------------------

pub fn validate_bounds(bounds: &ActorControlBounds) -> Result<(), UniverseError> {
    let document = &bounds.document;
    if document.get("schema_version").and_then(Value::as_str) != Some(SCHEMA_VERSION) {
        return Err(validation("bounds schema_version must be actor-control/1"));
    }
    // A non-empty bound Actor — an unbound contract is a refusal to operate.
    bounds.bound_actor()?;

    let max_speed = document
        .get("max_speed")
        .and_then(Value::as_f64)
        .ok_or_else(|| validation("max_speed must be a number"))?;
    if !max_speed.is_finite() || max_speed <= 0.0 {
        return Err(validation("max_speed must be positive and finite"));
    }
    let max_tick = document
        .get("max_tick_displacement")
        .and_then(Value::as_f64)
        .ok_or_else(|| validation("max_tick_displacement must be a number"))?;
    if !max_tick.is_finite() || max_tick <= 0.0 {
        return Err(validation(
            "max_tick_displacement must be positive and finite",
        ));
    }

    let axes = document
        .get("axes")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("bounds document has no axes"))?;
    let permitted = ["forward", "right", "up"]
        .iter()
        .filter(|axis| axes.get(**axis) == Some(&Value::Bool(true)))
        .count();
    if permitted == 0 {
        // The control analogue of "never treat unknown as zero": a contract that
        // permits no axis is refused, not silently corrected into one.
        return Err(validation("at least one motion axis must be permitted"));
    }

    let gate_rule = bounds.gate_rule()?;
    if !ALLOWED_GATE_RULES.contains(&gate_rule) {
        return Err(validation(format!(
            "gate_rule {gate_rule} is not implemented"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Materialization + independent readback.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActorControlReceipt {
    pub kind: String,
    pub change_id: String,
    pub authority: String,
    pub status: String,
    pub universe: UniverseId,
    pub newly_committed: bool,
    pub authority_id: String,
    pub bounds_id: String,
    pub bound_actor: String,
    pub gate_rule: String,
    pub catalog_sha256: String,
    /// The bounds document read back from the store is byte-identical to the
    /// fixture the renderer consumes.
    pub catalog_parity: bool,
    pub nodes_preserved: bool,
    /// Exactly one Actor is bound to the mapping by `PROJECTS_AS`.
    pub bindings: usize,
    pub final_snapshot_hash: String,
}

fn build_seed(bounds: &ActorControlBounds) -> Result<GraphSeed, UniverseError> {
    validate_bounds(bounds)?;
    let catalog_sha256 = canonical_hash(&bounds.document)?;
    let bounds_id = bounds.bounds_id()?.to_owned();
    let bound_actor = bounds.bound_actor()?.to_owned();
    let gate_rule = bounds.gate_rule()?.to_owned();

    let symbols = vec![
        "actor_control_contract".to_owned(),
        "actor_control_bounds_mapping".to_owned(),
        "actor_control_bounds_catalog".to_owned(),
        "actor_control_changeset".to_owned(),
        "bound_actor".to_owned(),
        "GOVERNED_BY".to_owned(),
        "HAS_PAYLOAD".to_owned(),
        "PROJECTS_AS".to_owned(),
        "PART_OF".to_owned(),
    ];

    let entities = vec![
        seed_entity(
            CONTRACT_ATOM,
            "actor_control_contract",
            json!({
                "kind": "actor_control_contract",
                "contract_id": CONTRACT_ID,
                "output_kind": "actor_control_bounds",
                "schema_version": SCHEMA_VERSION,
                "node_remains_authoritative": true,
                "asset_is_derived": true,
                "invalidation_signals": ["bounds_revision", "bounds_catalog_hash", "bound_actor_revision"],
            }),
        ),
        seed_entity(
            MAPPING_ATOM,
            "actor_control_bounds_mapping",
            json!({
                "kind": "actor_control_bounds_mapping",
                "authority_id": bounds.authority_id,
                "bounds_id": bounds_id,
                "output_kind": "actor_control_bounds",
                "media_type": "application/vnd.mind.actor-control+json",
                "catalog_sha256": catalog_sha256,
                "schema_version": SCHEMA_VERSION,
                "gate_rule": gate_rule,
                "canonical_node_replaced": false,
            }),
        ),
        seed_entity(
            CATALOG_ATOM,
            "actor_control_bounds_catalog",
            json!({
                "kind": "actor_control_bounds_catalog",
                "content_address": format!("sha256:{catalog_sha256}"),
                "catalog_sha256": catalog_sha256,
                "value": bounds.document,
            }),
        ),
        seed_entity(
            CHANGESET_ATOM,
            "actor_control_changeset",
            json!({
                "kind": "actor_control_changeset",
                "change_id": CHANGE_ID,
                "authority": AUTHORITY,
                "status": STATUS,
                "contract": CONTRACT_ATOM,
                "mapping": MAPPING_ATOM,
                "scope": [MAPPING_ATOM],
            }),
        ),
        seed_entity(
            ACTOR_ATOM,
            "bound_actor",
            json!({
                "kind": "bound_actor",
                "canonical_id": bound_actor,
            }),
        ),
    ];

    let relations = vec![
        seed_relation(
            RELATION_BASE,
            MAPPING_ATOM,
            CONTRACT_ATOM,
            "GOVERNED_BY",
            None,
        ),
        seed_relation(
            RELATION_BASE + 1,
            CHANGESET_ATOM,
            CONTRACT_ATOM,
            "GOVERNED_BY",
            None,
        ),
        seed_relation(
            RELATION_BASE + 2,
            MAPPING_ATOM,
            CATALOG_ATOM,
            "HAS_PAYLOAD",
            None,
        ),
        seed_relation(
            RELATION_BASE + 3,
            MAPPING_ATOM,
            CHANGESET_ATOM,
            "PART_OF",
            None,
        ),
        seed_relation(
            RELATION_BASE + 4,
            ACTOR_ATOM,
            MAPPING_ATOM,
            "PROJECTS_AS",
            Some(json!({
                "authority_id": bounds.authority_id,
                "gate_rule": gate_rule,
            })),
        ),
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
    bounds: &ActorControlBounds,
) -> Result<ActorControlReceipt, UniverseError> {
    let store_root = store_root.as_ref();
    let seed = build_seed(bounds)?;
    let catalog_sha256 = canonical_hash(&bounds.document)?;
    let bounds_id = bounds.bounds_id()?.to_owned();
    let bound_actor = bounds.bound_actor()?.to_owned();
    let gate_rule = bounds.gate_rule()?.to_owned();

    let store = UniverseStore::open(store_root)?;
    let newly_committed = !store_root.join("snapshot.json").exists();
    if newly_committed {
        store.install_seed(&seed)?;
    }

    // Independent readback: reopen, replay, and verify the bounds Asset is
    // present, byte-identical to the fixture, and its provenance links resolve.
    let readback_store = UniverseStore::open(store_root)?;
    let readback = readback_store.replay(readback_store.load_snapshot()?)?;

    let mapping_entity = readback
        .entities
        .iter()
        .find(|entity| entity.key == MAPPING_ATOM)
        .ok_or_else(|| validation("actor-control mapping Asset missing after reopen"))?;
    let mapping_content = mapping_entity
        .content
        .as_ref()
        .ok_or_else(|| validation("actor-control mapping Asset has no content"))
        .and_then(|content| readback_store.read_content(content))?;
    let nodes_preserved =
        mapping_content.get("canonical_node_replaced") == Some(&Value::Bool(false));
    let gate_rule_read_back =
        mapping_content.get("gate_rule").and_then(Value::as_str) == Some(gate_rule.as_str());
    if !gate_rule_read_back {
        return Err(validation("read-back gate_rule differs from the fixture"));
    }

    let catalog_entity = readback
        .entities
        .iter()
        .find(|entity| entity.key == CATALOG_ATOM)
        .ok_or_else(|| validation("actor-control bounds payload missing after reopen"))?;
    let catalog_content = catalog_entity
        .content
        .as_ref()
        .ok_or_else(|| validation("actor-control bounds payload has no content"))
        .and_then(|content| readback_store.read_content(content))?;
    let read_document = catalog_content
        .get("value")
        .ok_or_else(|| validation("bounds payload has no value"))?;
    let catalog_parity = canonical_hash(read_document)? == catalog_sha256;
    if !catalog_parity {
        return Err(validation("read-back bounds differ from the fixture"));
    }

    let projects_as = readback
        .symbol_id("PROJECTS_AS")
        .ok_or_else(|| validation("PROJECTS_AS symbol absent"))?;
    let bindings = readback
        .relations
        .iter()
        .filter(|relation| relation.predicate == projects_as && relation.target == MAPPING_ATOM)
        .count();
    if bindings != 1 {
        return Err(validation(
            "exactly one Actor must be bound to the bounds mapping",
        ));
    }

    Ok(ActorControlReceipt {
        kind: "actor_control_bounds_materialization_receipt".into(),
        change_id: CHANGE_ID.into(),
        authority: AUTHORITY.into(),
        status: STATUS.into(),
        universe: readback.universe,
        newly_committed,
        authority_id: bounds.authority_id.clone(),
        bounds_id,
        bound_actor,
        gate_rule,
        catalog_sha256,
        catalog_parity,
        nodes_preserved,
        bindings,
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

    fn bounds() -> ActorControlBounds {
        ActorControlBounds::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/desktop-control/actor-control-bounds.json"),
        )
        .unwrap()
    }

    #[test]
    fn materializes_bounds_with_independent_parity() {
        let temp = tempfile::tempdir().unwrap();
        let receipt = materialize(temp.path().join("store"), &bounds()).unwrap();
        assert!(receipt.newly_committed);
        assert!(receipt.catalog_parity);
        assert!(receipt.nodes_preserved);
        assert_eq!(receipt.bindings, 1);
        assert_eq!(receipt.bounds_id, "citizen-avatar-motion-bounds-v0");
        assert_eq!(receipt.bound_actor, "fixture:actor:citizen-energy-avatar");
        assert_eq!(receipt.gate_rule, "granted_bound_actor_only");
    }

    #[test]
    fn a_contract_permitting_no_axis_is_rejected() {
        let mut bounds = bounds();
        bounds.document["axes"] = json!({ "forward": false, "right": false, "up": false });
        assert!(matches!(
            validate_bounds(&bounds),
            Err(UniverseError::Validation(message)) if message.contains("at least one motion axis")
        ));
    }

    #[test]
    fn a_non_positive_speed_is_rejected_not_defaulted() {
        let mut bounds = bounds();
        bounds.document["max_speed"] = json!(0);
        assert!(matches!(
            validate_bounds(&bounds),
            Err(UniverseError::Validation(message)) if message.contains("max_speed must be positive")
        ));
    }

    #[test]
    fn an_unimplemented_gate_rule_is_rejected() {
        let mut bounds = bounds();
        bounds.document["gate_rule"] = json!("anyone_can_move");
        assert!(matches!(
            validate_bounds(&bounds),
            Err(UniverseError::Validation(message)) if message.contains("is not implemented")
        ));
    }
}
