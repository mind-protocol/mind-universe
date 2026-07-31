//! Affordance-face authority — the graph-native `AffordanceDefinition` catalog.
//!
//! CLAUDE.md lists `AffordanceDefinition` among the versioned world definitions:
//! the bounded, typed, permission-aware, precondition-explicit action list a
//! situated Citizen or LLM constructor acts from. This module is the **authority
//! half** of that face: a graph-declared catalog of affordance TEMPLATES,
//! validated by a GENERIC native validator, materialized as a content-addressed
//! Asset, then read back independently (reopen cold, replay, compare byte-for-
//! byte) — a file is only a projection of the graph authority.
//!
//! The validator is deliberately generic across every affordance. Policy,
//! metaphor, and justification stay in the fixture (graph data). The native code
//! enforces only structure-independent invariants:
//!
//! - **Justification is mandatory**: an affordance with no non-empty
//!   justification is rejected (an unjustified physical gesture is not a
//!   legitimate interpretation of any semantic operation).
//! - **Kernel boundary**: each template's `expected_semantic_effect` may only
//!   promise a write that compiles to one of the four kernel primitives (or
//!   `none` for a read-only affordance). No 5th write verb; no Rust `match`
//!   dispatches on an ontology-vocabulary string — `kind` and `write_primitive`
//!   are validated by CLOSED-SET membership, exactly like `ALLOWED_PRIMITIVES`.
//! - **Totality on the affordance face**: the catalog must declare its `radius`
//!   (the enumerable config domain it covers). For EVERY config in the radius it
//!   must offer a DEFINED action-set — zero holes. `coverage = defined /
//!   |radius|`; any hole fails NAMING the exact missing `(semantic_type,
//!   target_epistemic_state)` tuple. Honesty ≠ invention: an unproven-target
//!   config (`known_absent / unknown / not_measured / measurement_failed`) has a
//!   DEFINED instruction that is fogged/forbidden — never actionable. A hole is
//!   as faulty as a fabricated value.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use universe_core::{EntityKey, RelationKey, UniverseError, UniverseId};
use universe_store::{canonical_hash, GraphSeed, SeedEntity, SeedRelation, UniverseStore};

const SCHEMA_VERSION: &str = "affordance-catalog/1";

/// The closed affordance verb set. Membership is validated; behaviour is NOT
/// dispatched per verb — the validator treats every kind identically.
const KINDS: [&str; 6] = ["inspect", "place", "connect", "open", "build", "test"];

/// The kernel write boundary. An affordance may promise `none` (read-only) or a
/// single one of the four kernel primitives — never a 5th write verb.
const WRITE_PRIMITIVES: [&str; 5] = [
    "none",
    "InternSymbols",
    "PutEntity",
    "PutRelation",
    "TombstoneRelation",
];

/// The six canonical epistemic states of a target (CLAUDE.md discipline).
const EPISTEMIC_STATES: [&str; 6] = [
    "observed",
    "measured",
    "known_absent",
    "unknown",
    "not_measured",
    "measurement_failed",
];

/// The two states in which a target is proven present. Only these may carry an
/// `actionable` disposition — everything else is an unproven target.
const PROVEN_STATES: [&str; 2] = ["observed", "measured"];

/// The defined dispositions a coverage cell may declare.
const DISPOSITIONS: [&str; 4] = ["actionable", "empty", "fogged", "forbidden"];

const UNIVERSE: UniverseId = UniverseId(0x9000);
const CONTRACT_ATOM: EntityKey = EntityKey(0x9001);
const CHANGESET_ATOM: EntityKey = EntityKey(0x9002);
const DEFINITION_ATOM: EntityKey = EntityKey(0x9010);
const CATALOG_ATOM: EntityKey = EntityKey(0x9011);
const SEMANTIC_BASE: u128 = 0x9100;
const RELATION_BASE: u128 = 0x9200;

const CHANGE_ID: &str = "affordance-catalog-materialization-v0";
const CONTRACT_ID: &str = "affordance-face-contract-v0";
const AUTHORITY: &str = "graph_first_affordance_authority";
const STATUS: &str = "approved_for_projection";

// ---------------------------------------------------------------------------
// Catalog input (graph-declared authority, loaded from a fixture).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AffordanceCatalog {
    pub authority_id: String,
    /// The `affordance-catalog/1` document, preserved verbatim so the graph
    /// Asset is byte-identical to the authority the face consumes.
    pub catalog: Value,
}

impl AffordanceCatalog {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, UniverseError> {
        let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| UniverseError::CorruptContent(error.to_string()))
    }

    pub fn catalog_id(&self) -> Result<&str, UniverseError> {
        self.catalog
            .get("catalog_id")
            .and_then(Value::as_str)
            .ok_or_else(|| validation("catalog has no catalog_id"))
    }
}

// ---------------------------------------------------------------------------
// Coverage report — the totality result of the affordance face.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoverageReport {
    /// `|radius|` = |semantic_types| × |epistemic_states|.
    pub radius_size: usize,
    /// Configs with a defined action-set (must equal `radius_size` to pass).
    pub defined: usize,
    /// `defined / radius_size` — 1.0 when the face is a total function.
    pub coverage: f64,
    pub templates: usize,
    pub actionable_cells: usize,
    pub fogged_cells: usize,
    pub forbidden_cells: usize,
    pub empty_cells: usize,
    /// No unproven-target config resolved to an actionable disposition.
    pub honesty_held: bool,
}

// ---------------------------------------------------------------------------
// Generic validation — structure-independent, no per-verb dispatch.
// ---------------------------------------------------------------------------

/// Validates a graph-declared affordance catalog and returns its coverage
/// report. Every failure names the exact offending config or template.
pub fn validate_affordance_catalog(
    catalog: &AffordanceCatalog,
) -> Result<CoverageReport, UniverseError> {
    let doc = &catalog.catalog;
    if doc.get("schema_version").and_then(Value::as_str) != Some(SCHEMA_VERSION) {
        return Err(validation(
            "catalog schema_version must be affordance-catalog/1",
        ));
    }

    let templates = validate_templates(doc)?;

    // The radius is the declared, enumerable config domain. Without it,
    // "totality over all configs" is undefined — reject.
    let radius = doc
        .get("radius")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("catalog declares no radius; its config domain is undefined"))?;
    let semantic_types = radius
        .get("semantic_types")
        .and_then(Value::as_array)
        .ok_or_else(|| validation("radius has no semantic_types list"))?;
    if semantic_types.is_empty() {
        return Err(validation("radius semantic_types is empty"));
    }
    let declared_states = radius
        .get("target_epistemic_states")
        .and_then(Value::as_array)
        .ok_or_else(|| validation("radius has no target_epistemic_states list"))?;
    // The epistemic axis is fixed kernel discipline: the radius must declare
    // exactly the six canonical states, in the canonical set.
    let declared: Vec<&str> = declared_states
        .iter()
        .filter_map(Value::as_str)
        .collect();
    if declared.len() != EPISTEMIC_STATES.len()
        || !EPISTEMIC_STATES
            .iter()
            .all(|state| declared.contains(state))
    {
        return Err(validation(
            "radius target_epistemic_states must be exactly the six canonical epistemic states",
        ));
    }

    let coverage_map = doc
        .get("coverage")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("catalog has no coverage map"))?;

    let semantic_types: Vec<&str> = semantic_types
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| validation("radius semantic_type is not a string"))
        })
        .collect::<Result<_, _>>()?;

    let radius_size = semantic_types.len() * EPISTEMIC_STATES.len();
    let mut defined = 0usize;
    let mut actionable_cells = 0usize;
    let mut fogged_cells = 0usize;
    let mut forbidden_cells = 0usize;
    let mut empty_cells = 0usize;
    let honesty_held = true;

    // Totality: enumerate the FULL radius product; every tuple must resolve to a
    // defined cell. A missing tuple is a hole named exactly.
    for semantic_type in &semantic_types {
        let per_type = coverage_map.get(*semantic_type).and_then(Value::as_object);
        for state in EPISTEMIC_STATES {
            let cell = per_type
                .and_then(|per| per.get(state))
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    validation(format!(
                        "coverage hole: no defined action-set for config \
                         (semantic_type={semantic_type}, target_epistemic_state={state})"
                    ))
                })?;
            let disposition = cell
                .get("disposition")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    validation(format!(
                        "coverage cell (semantic_type={semantic_type}, \
                         target_epistemic_state={state}) has no disposition"
                    ))
                })?;
            if !DISPOSITIONS.contains(&disposition) {
                return Err(validation(format!(
                    "coverage cell (semantic_type={semantic_type}, \
                     target_epistemic_state={state}) has unknown disposition {disposition}"
                )));
            }
            let actions = cell
                .get("actions")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    validation(format!(
                        "coverage cell (semantic_type={semantic_type}, \
                         target_epistemic_state={state}) has no actions list"
                    ))
                })?;

            let proven = PROVEN_STATES.contains(&state);
            match disposition {
                "actionable" => {
                    // Honesty: an actionable disposition is only legitimate over
                    // a proven target. An unproven target that resolves to
                    // actionable is a fabricated confidence — reject.
                    if !proven {
                        // Honesty violation is fatal: an unproven target can
                        // never carry a confident action. `honesty_held` in the
                        // report is therefore only ever observed as `true`.
                        return Err(validation(format!(
                            "dishonest affordance: unproven target config \
                             (semantic_type={semantic_type}, target_epistemic_state={state}) \
                             resolves to an actionable disposition"
                        )));
                    }
                    if actions.is_empty() {
                        return Err(validation(format!(
                            "actionable cell (semantic_type={semantic_type}, \
                             target_epistemic_state={state}) offers no actions"
                        )));
                    }
                    // Each action must reference a template that applies to this
                    // semantic type — an affordance cannot offer an action its
                    // template does not legitimize.
                    for action in actions {
                        let action_id = action.as_str().ok_or_else(|| {
                            validation(format!(
                                "action id in cell (semantic_type={semantic_type}, \
                                 target_epistemic_state={state}) is not a string"
                            ))
                        })?;
                        let template = templates
                            .iter()
                            .find(|template| template.id == action_id)
                            .ok_or_else(|| {
                                validation(format!(
                                    "cell (semantic_type={semantic_type}, \
                                     target_epistemic_state={state}) references unknown \
                                     template {action_id}"
                                ))
                            })?;
                        if !template.applies_to.contains(&(*semantic_type).to_owned()) {
                            return Err(validation(format!(
                                "template {action_id} does not apply to semantic_type \
                                 {semantic_type} but is offered in its actionable cell"
                            )));
                        }
                    }
                    actionable_cells += 1;
                }
                // A non-actionable disposition is the honest "no confident
                // action" instruction. It IS defined (totality), and it must NOT
                // smuggle confident actions in.
                other => {
                    if !actions.is_empty() {
                        return Err(validation(format!(
                            "non-actionable ({other}) cell (semantic_type={semantic_type}, \
                             target_epistemic_state={state}) must offer no confident actions"
                        )));
                    }
                    match other {
                        "fogged" => fogged_cells += 1,
                        "forbidden" => forbidden_cells += 1,
                        "empty" => {
                            // "nothing legitimate here" is only honest over a
                            // proven target; over an unproven one the honest
                            // instruction is fog/forbidden, not silence.
                            if !proven {
                                return Err(validation(format!(
                                    "unproven target config (semantic_type={semantic_type}, \
                                     target_epistemic_state={state}) must be fogged/forbidden, \
                                     not empty"
                                )));
                            }
                            empty_cells += 1;
                        }
                        _ => unreachable!("disposition already checked against DISPOSITIONS"),
                    }
                }
            }
            defined += 1;
        }
    }

    let coverage = if radius_size == 0 {
        0.0
    } else {
        defined as f64 / radius_size as f64
    };

    Ok(CoverageReport {
        radius_size,
        defined,
        coverage,
        templates: templates.len(),
        actionable_cells,
        fogged_cells,
        forbidden_cells,
        empty_cells,
        honesty_held,
    })
}

/// A minimal, validated view of one affordance template.
#[derive(Clone, Debug)]
struct Template {
    id: String,
    applies_to: Vec<String>,
}

fn validate_templates(doc: &Value) -> Result<Vec<Template>, UniverseError> {
    let templates = doc
        .get("templates")
        .and_then(Value::as_array)
        .ok_or_else(|| validation("catalog has no templates list"))?;
    if templates.is_empty() {
        return Err(validation("catalog declares no templates"));
    }
    let mut seen = Vec::new();
    let mut result = Vec::new();
    for template in templates {
        let id = template
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| validation("template has no id"))?;
        if seen.contains(&id.to_owned()) {
            return Err(validation(format!("duplicate template id {id}")));
        }
        seen.push(id.to_owned());

        let kind = template
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| validation(format!("template {id} has no kind")))?;
        if !KINDS.contains(&kind) {
            return Err(validation(format!(
                "template {id} kind {kind} is not in the closed affordance verb set"
            )));
        }

        // Justification is mandatory and non-empty.
        let justification = template
            .get("justification")
            .and_then(Value::as_str)
            .unwrap_or("");
        if justification.trim().is_empty() {
            return Err(validation(format!(
                "template {id} has no justification; an unjustified affordance is rejected"
            )));
        }

        // Preconditions must be present.
        let preconditions = template
            .get("preconditions")
            .and_then(Value::as_array)
            .ok_or_else(|| validation(format!("template {id} has no preconditions list")))?;
        if preconditions.is_empty() {
            return Err(validation(format!("template {id} has empty preconditions")));
        }

        // Capability must be present and non-empty.
        let capability = template
            .get("capability")
            .and_then(Value::as_str)
            .unwrap_or("");
        if capability.trim().is_empty() {
            return Err(validation(format!("template {id} has no capability")));
        }

        // Bounds (fuel + radius budget) must be present.
        let bounds = template
            .get("bounds")
            .and_then(Value::as_object)
            .ok_or_else(|| validation(format!("template {id} has no bounds")))?;
        if bounds.get("fuel").and_then(Value::as_u64).is_none() {
            return Err(validation(format!("template {id} bounds has no fuel budget")));
        }
        if bounds.get("radius").and_then(Value::as_u64).is_none() {
            return Err(validation(format!(
                "template {id} bounds has no radius budget"
            )));
        }

        // Expected semantic effect must be present and stay within the kernel
        // write boundary (one of the four primitives, or read-only `none`).
        let effect = template
            .get("expected_semantic_effect")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                validation(format!("template {id} has no expected_semantic_effect"))
            })?;
        let write_primitive = effect
            .get("write_primitive")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                validation(format!(
                    "template {id} expected_semantic_effect has no write_primitive"
                ))
            })?;
        if !WRITE_PRIMITIVES.contains(&write_primitive) {
            return Err(validation(format!(
                "template {id} promises write_primitive {write_primitive}, which is not a \
                 kernel primitive (a 5th write verb is forbidden)"
            )));
        }

        let applies_to = template
            .get("applies_to")
            .and_then(|value| value.get("semantic_types"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                validation(format!("template {id} has no applies_to.semantic_types"))
            })?;
        if applies_to.is_empty() {
            return Err(validation(format!(
                "template {id} applies to no semantic type"
            )));
        }
        let applies_to: Vec<String> = applies_to
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| validation(format!("template {id} applies_to entry is not a string")))
            })
            .collect::<Result<_, _>>()?;

        result.push(Template {
            id: id.to_owned(),
            applies_to,
        });
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Materialization + independent readback.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AffordanceReceipt {
    pub kind: String,
    pub change_id: String,
    pub authority: String,
    pub status: String,
    pub universe: UniverseId,
    pub newly_committed: bool,
    pub authority_id: String,
    pub catalog_id: String,
    pub catalog_sha256: String,
    /// The catalog read back from the store is byte-identical to the fixture.
    pub catalog_parity: bool,
    /// The canonical node (the definition) was preserved, not replaced by the
    /// derived Asset.
    pub nodes_preserved: bool,
    pub radius_size: usize,
    pub defined: usize,
    pub coverage: f64,
    pub templates: usize,
    pub semantic_types_bound: usize,
    /// The totality + honesty invariants held when re-validated under readback.
    pub honesty_invariant_held: bool,
    pub final_snapshot_hash: String,
}

fn build_seed(catalog: &AffordanceCatalog) -> Result<(GraphSeed, CoverageReport), UniverseError> {
    let report = validate_affordance_catalog(catalog)?;
    let catalog_sha256 = canonical_hash(&catalog.catalog)?;
    let catalog_id = catalog.catalog_id()?.to_owned();

    let semantic_types: Vec<String> = catalog
        .catalog
        .get("radius")
        .and_then(|radius| radius.get("semantic_types"))
        .and_then(Value::as_array)
        .map(|types| {
            types
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();

    let symbols = vec![
        "affordance_face_contract".to_owned(),
        "affordance_definition".to_owned(),
        "affordance_catalog".to_owned(),
        "affordance_changeset".to_owned(),
        "semantic_type".to_owned(),
        "GOVERNED_BY".to_owned(),
        "HAS_PAYLOAD".to_owned(),
        "AFFORDS".to_owned(),
        "PART_OF".to_owned(),
    ];

    let mut entities = vec![
        seed_entity(
            CONTRACT_ATOM,
            "affordance_face_contract",
            json!({
                "kind": "affordance_face_contract",
                "contract_id": CONTRACT_ID,
                "output_kind": "affordance_face",
                "schema_version": SCHEMA_VERSION,
                "node_remains_authoritative": true,
                "asset_is_derived": true,
                "kernel_write_primitives": WRITE_PRIMITIVES,
                "affordance_kinds": KINDS,
                "invalidation_signals": ["catalog_revision", "radius_hash", "semantic_type_revision"],
            }),
        ),
        seed_entity(
            DEFINITION_ATOM,
            "affordance_definition",
            json!({
                "kind": "affordance_definition",
                "authority_id": catalog.authority_id,
                "catalog_id": catalog_id,
                "output_kind": "affordance_face",
                "media_type": "application/vnd.mind.affordance-catalog+json",
                "catalog_sha256": catalog_sha256,
                "schema_version": SCHEMA_VERSION,
                "radius_size": report.radius_size,
                "coverage": report.coverage,
                "canonical_node_replaced": false,
            }),
        ),
        seed_entity(
            CATALOG_ATOM,
            "affordance_catalog",
            json!({
                "kind": "affordance_catalog",
                "content_address": format!("sha256:{catalog_sha256}"),
                "catalog_sha256": catalog_sha256,
                "value": catalog.catalog,
            }),
        ),
        seed_entity(
            CHANGESET_ATOM,
            "affordance_changeset",
            json!({
                "kind": "affordance_changeset",
                "change_id": CHANGE_ID,
                "authority": AUTHORITY,
                "status": STATUS,
                "contract": CONTRACT_ATOM,
                "definition": DEFINITION_ATOM,
                "scope": [DEFINITION_ATOM],
            }),
        ),
    ];

    let mut relations = vec![
        seed_relation(
            RELATION_BASE,
            DEFINITION_ATOM,
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
            DEFINITION_ATOM,
            CATALOG_ATOM,
            "HAS_PAYLOAD",
            None,
        ),
        seed_relation(
            RELATION_BASE + 3,
            DEFINITION_ATOM,
            CHANGESET_ATOM,
            "PART_OF",
            None,
        ),
    ];

    let mut relation_key = RELATION_BASE + 4;
    for (index, semantic_type) in semantic_types.iter().enumerate() {
        let semantic_atom = EntityKey(SEMANTIC_BASE + index as u128);
        entities.push(seed_entity(
            semantic_atom,
            "semantic_type",
            json!({
                "kind": "semantic_type",
                "canonical_id": semantic_type,
            }),
        ));
        relations.push(seed_relation(
            relation_key,
            semantic_atom,
            DEFINITION_ATOM,
            "AFFORDS",
            Some(json!({
                "authority_id": catalog.authority_id,
                "covers_epistemic_states": EPISTEMIC_STATES,
            })),
        ));
        relation_key += 1;
    }

    Ok((
        GraphSeed {
            universe: UNIVERSE,
            symbols,
            entities,
            relations,
        },
        report,
    ))
}

pub fn materialize(
    store_root: impl AsRef<Path>,
    catalog: &AffordanceCatalog,
) -> Result<AffordanceReceipt, UniverseError> {
    let store_root = store_root.as_ref();
    let (seed, report) = build_seed(catalog)?;
    let catalog_sha256 = canonical_hash(&catalog.catalog)?;
    let catalog_id = catalog.catalog_id()?.to_owned();

    let store = UniverseStore::open(store_root)?;
    let newly_committed = !store_root.join("snapshot.json").exists();
    if newly_committed {
        store.install_seed(&seed)?;
    }

    // Independent readback: reopen cold, replay, and verify the catalog Asset is
    // present, byte-identical to the fixture, and its provenance links resolve.
    let readback_store = UniverseStore::open(store_root)?;
    let readback = readback_store.replay(readback_store.load_snapshot()?)?;

    let definition_entity = readback
        .entities
        .iter()
        .find(|entity| entity.key == DEFINITION_ATOM)
        .ok_or_else(|| validation("affordance definition Asset missing after reopen"))?;
    let definition_content = definition_entity
        .content
        .as_ref()
        .ok_or_else(|| validation("affordance definition Asset has no content"))
        .and_then(|content| readback_store.read_content(content))?;
    let nodes_preserved =
        definition_content.get("canonical_node_replaced") == Some(&Value::Bool(false));

    let catalog_entity = readback
        .entities
        .iter()
        .find(|entity| entity.key == CATALOG_ATOM)
        .ok_or_else(|| validation("affordance catalog payload missing after reopen"))?;
    let catalog_content = catalog_entity
        .content
        .as_ref()
        .ok_or_else(|| validation("affordance catalog payload has no content"))
        .and_then(|content| readback_store.read_content(content))?;
    let read_catalog = catalog_content
        .get("value")
        .ok_or_else(|| validation("catalog payload has no value"))?;
    let catalog_parity = canonical_hash(read_catalog)? == catalog_sha256;
    if !catalog_parity {
        return Err(validation("read-back catalog differs from the fixture"));
    }

    // Re-run the generic validator on the INDEPENDENTLY read-back catalog: the
    // totality (zero holes) and honesty (no unproven config actionable)
    // invariants must still hold on the store path, not just on the fixture.
    let readback_catalog = AffordanceCatalog {
        authority_id: catalog.authority_id.clone(),
        catalog: read_catalog.clone(),
    };
    let readback_report = validate_affordance_catalog(&readback_catalog)?;
    let honesty_invariant_held =
        readback_report.honesty_held && readback_report.defined == readback_report.radius_size;
    if !honesty_invariant_held {
        return Err(validation(
            "totality or honesty invariant failed under readback",
        ));
    }

    let affords = readback
        .symbol_id("AFFORDS")
        .ok_or_else(|| validation("AFFORDS symbol absent"))?;
    let semantic_types_bound = readback
        .relations
        .iter()
        .filter(|relation| relation.predicate == affords && relation.target == DEFINITION_ATOM)
        .count();

    Ok(AffordanceReceipt {
        kind: "affordance_catalog_materialization_receipt".into(),
        change_id: CHANGE_ID.into(),
        authority: AUTHORITY.into(),
        status: STATUS.into(),
        universe: readback.universe,
        newly_committed,
        authority_id: catalog.authority_id.clone(),
        catalog_id,
        catalog_sha256,
        catalog_parity,
        nodes_preserved,
        radius_size: report.radius_size,
        defined: report.defined,
        coverage: report.coverage,
        templates: report.templates,
        semantic_types_bound,
        honesty_invariant_held,
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

    fn catalog() -> AffordanceCatalog {
        AffordanceCatalog::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/affordance/affordance-catalog.json"),
        )
        .unwrap()
    }

    #[test]
    fn accepts_the_real_catalog_with_full_coverage() {
        let report = validate_affordance_catalog(&catalog()).unwrap();
        // 5 semantic types × 6 epistemic states = 30 configs, zero holes.
        assert_eq!(report.radius_size, 30);
        assert_eq!(report.defined, 30);
        assert_eq!(report.coverage, 1.0);
        assert_eq!(report.templates, 6);
        // 2 proven states × 5 types = 10 actionable; 4 unproven states → fog or
        // forbidden. known_absent(5) = forbidden; the other 3 unproven ×5 = 15
        // fogged.
        assert_eq!(report.actionable_cells, 10);
        assert_eq!(report.forbidden_cells, 5);
        assert_eq!(report.fogged_cells, 15);
        assert!(report.honesty_held);
    }

    #[test]
    fn materializes_catalog_with_independent_parity() {
        let temp = tempfile::tempdir().unwrap();
        let receipt = materialize(temp.path().join("store"), &catalog()).unwrap();
        assert!(receipt.newly_committed);
        assert!(receipt.catalog_parity);
        assert!(receipt.nodes_preserved);
        assert!(receipt.honesty_invariant_held);
        assert_eq!(receipt.radius_size, 30);
        assert_eq!(receipt.defined, 30);
        assert_eq!(receipt.coverage, 1.0);
        assert_eq!(receipt.templates, 6);
        assert_eq!(receipt.semantic_types_bound, 5);
        assert_eq!(receipt.catalog_id, "construction-affordance-catalog-v1");
    }

    #[test]
    fn rejects_a_template_with_empty_justification() {
        let mut catalog = catalog();
        catalog.catalog["templates"][0]["justification"] = json!("   ");
        assert!(matches!(
            validate_affordance_catalog(&catalog),
            Err(UniverseError::Validation(message))
                if message.contains("has no justification")
        ));
    }

    #[test]
    fn rejects_a_template_with_missing_justification() {
        let mut catalog = catalog();
        catalog.catalog["templates"][1]
            .as_object_mut()
            .unwrap()
            .remove("justification");
        assert!(matches!(
            validate_affordance_catalog(&catalog),
            Err(UniverseError::Validation(message))
                if message.contains("has no justification")
        ));
    }

    #[test]
    fn rejects_a_coverage_hole_and_names_the_missing_config() {
        let mut catalog = catalog();
        // Punch a deliberate hole: drop the (machine, unknown) cell.
        catalog.catalog["coverage"]["machine"]
            .as_object_mut()
            .unwrap()
            .remove("unknown");
        let error = validate_affordance_catalog(&catalog).unwrap_err();
        let UniverseError::Validation(message) = error else {
            panic!("expected a validation error, got {error:?}");
        };
        assert!(message.contains("coverage hole"), "message: {message}");
        assert!(
            message.contains("semantic_type=machine")
                && message.contains("target_epistemic_state=unknown"),
            "hole message must name the exact missing tuple: {message}"
        );
    }

    #[test]
    fn rejects_a_catalog_with_no_declared_radius() {
        let mut catalog = catalog();
        catalog.catalog.as_object_mut().unwrap().remove("radius");
        assert!(matches!(
            validate_affordance_catalog(&catalog),
            Err(UniverseError::Validation(message))
                if message.contains("declares no radius")
        ));
    }

    #[test]
    fn rejects_an_unproven_target_that_is_actionable() {
        let mut catalog = catalog();
        // Make (thing, unknown) confidently actionable — a fabricated
        // confidence the honesty invariant must refuse.
        catalog.catalog["coverage"]["thing"]["unknown"] = json!({
            "disposition": "actionable",
            "actions": ["inspect"]
        });
        assert!(matches!(
            validate_affordance_catalog(&catalog),
            Err(UniverseError::Validation(message))
                if message.contains("dishonest affordance") && message.contains("target_epistemic_state=unknown")
        ));
    }

    #[test]
    fn rejects_a_fifth_write_verb() {
        let mut catalog = catalog();
        catalog.catalog["templates"][4]["expected_semantic_effect"]["write_primitive"] =
            json!("MoveThing");
        assert!(matches!(
            validate_affordance_catalog(&catalog),
            Err(UniverseError::Validation(message))
                if message.contains("not a kernel primitive")
        ));
    }

    #[test]
    fn rejects_a_kind_outside_the_closed_verb_set() {
        let mut catalog = catalog();
        catalog.catalog["templates"][0]["kind"] = json!("teleport");
        assert!(matches!(
            validate_affordance_catalog(&catalog),
            Err(UniverseError::Validation(message))
                if message.contains("closed affordance verb set")
        ));
    }
}
