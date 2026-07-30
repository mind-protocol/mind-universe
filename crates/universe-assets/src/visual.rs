//! Visual embodiment mapping — the graph-native visual projection authority.
//!
//! The Mind Desktop renderer defines and validates a `visual-embodiment/1`
//! contract (`apps/mind-desktop/src/contracts.ts`) but, until now, the only
//! mapping instance was hard-coded in the app (`avatar-fixture.ts`). That is the
//! graph-first drift this module closes: the mapping catalog becomes a
//! content-addressed Asset in the store, bound to Node semantics by an explicit
//! policy, so the app can consume the authority instead of authoring it.
//!
//! Two layers, deliberately separated:
//! - **Durable** (this module): the reusable `VisualEmbodimentMapping` catalog
//!   materialized as a Node→Asset projection, validated by the SAME budgets the
//!   renderer enforces, and read back independently.
//! - **Live** (render time, in the app): per-entity `EntityEmbodiment` = catalog
//!   ⊕ physics sample ⊕ epistemic state. Never persisted as an Asset (it would be
//!   one Asset per frame); it is composed by the renderer at draw time.
//!
//! Content projection (see `conversion.rs`) copies a Node field into a payload.
//! Visual projection is different: it is a *computed* projection — a function of
//! `semantic_type`, physical residency, and epistemic state — so it is a distinct
//! mapping class (`output_kind = "visual_embodiment"`), not a field copy.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};
use universe_core::{EntityKey, RelationKey, UniverseError, UniverseId};
use universe_store::{canonical_hash, GraphSeed, SeedEntity, SeedRelation, UniverseStore};

const SCHEMA_VERSION: &str = "visual-embodiment/1";
// Native renderer budgets — mirror `apps/mind-desktop/src/embodiment.ts` so the
// graph authority is validated by the SAME limits the renderer enforces.
const MAX_PRIMITIVES: u64 = 12;
const MAX_PARTICLES: u64 = 160;
const ALLOWED_PRIMITIVES: [&str; 5] = ["icosphere", "sphere", "capsule", "points", "fresnel_shell"];
/// Renderer residency LOD keys (`PhysicalResidency` in contracts.ts).
const RESIDENCIES: [&str; 4] = ["hot", "sleeping", "aggregated", "dormant"];
/// The six epistemic states the visual authority must be able to render
/// distinctly (CLAUDE.md discipline + renderer `EpistemicState`).
const EPISTEMIC_STATES: [&str; 6] = [
    "observed",
    "measured",
    "known_absent",
    "unknown",
    "not_measured",
    "measurement_failed",
];

const UNIVERSE: UniverseId = UniverseId(0x7000);
const CONTRACT_ATOM: EntityKey = EntityKey(0x7001);
const CHANGESET_ATOM: EntityKey = EntityKey(0x7002);
const MAPPING_ATOM: EntityKey = EntityKey(0x7010);
const CATALOG_ATOM: EntityKey = EntityKey(0x7011);
const SEMANTIC_BASE: u128 = 0x7100;
const RELATION_BASE: u128 = 0x7200;

const CHANGE_ID: &str = "visual-mapping-materialization-v0";
const CONTRACT_ID: &str = "visual-projection-contract-v0";
const AUTHORITY: &str = "graph_first_visual_authority";
const STATUS: &str = "approved_for_projection";

// ---------------------------------------------------------------------------
// Policy + catalog inputs (graph-declared authority, loaded from fixtures).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VisualCatalog {
    pub authority_id: String,
    /// The `visual-embodiment/1` document, preserved verbatim so the graph
    /// Asset is byte-identical to what the renderer consumes.
    pub mapping: Value,
    pub motion_profile: Value,
}

impl VisualCatalog {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, UniverseError> {
        let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| UniverseError::CorruptContent(error.to_string()))
    }

    pub fn mapping_id(&self) -> Result<&str, UniverseError> {
        self.mapping
            .get("mapping_id")
            .and_then(Value::as_str)
            .ok_or_else(|| validation("catalog mapping has no mapping_id"))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VisualBinding {
    pub semantic_type: String,
    pub authority_id: String,
    pub justification: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Modulation {
    /// Whether this epistemic state may be rendered as a confident presence.
    pub confident: bool,
    pub emissive_scale: f64,
    pub opacity_scale: f64,
    pub desaturate: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VisualPolicy {
    pub policy_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_authority: Option<String>,
    pub bindings: Vec<VisualBinding>,
    pub epistemic_modulation: BTreeMap<String, Modulation>,
}

impl VisualPolicy {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, UniverseError> {
        let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| UniverseError::CorruptContent(error.to_string()))
    }

    /// The visual mapping authority a semantic type resolves to: an explicit
    /// binding, else the declared fallback. Never an invented default.
    pub fn authority_for(&self, semantic_type: &str) -> Option<&str> {
        self.bindings
            .iter()
            .find(|binding| binding.semantic_type == semantic_type)
            .map(|binding| binding.authority_id.as_str())
            .or(self.default_authority.as_deref())
    }
}

// ---------------------------------------------------------------------------
// Validation — mirrors the renderer's `validateEmbodimentMapping`.
// ---------------------------------------------------------------------------

pub fn validate_catalog(catalog: &VisualCatalog) -> Result<(), UniverseError> {
    let mapping = &catalog.mapping;
    if mapping.get("schema_version").and_then(Value::as_str) != Some(SCHEMA_VERSION) {
        return Err(validation(
            "catalog schema_version must be visual-embodiment/1",
        ));
    }
    let primitive_budget = mapping
        .get("primitive_budget")
        .and_then(Value::as_u64)
        .ok_or_else(|| validation("primitive_budget must be an integer"))?;
    if !(1..=MAX_PRIMITIVES).contains(&primitive_budget) {
        return Err(validation("primitive_budget out of renderer range"));
    }
    let particle_budget = mapping
        .get("particle_budget")
        .and_then(Value::as_u64)
        .ok_or_else(|| validation("particle_budget must be an integer"))?;
    if particle_budget > MAX_PARTICLES {
        return Err(validation("particle_budget exceeds renderer range"));
    }
    let forms = mapping
        .get("forms")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("catalog has no forms"))?;
    let fallback = mapping
        .get("fallback_form")
        .and_then(Value::as_str)
        .ok_or_else(|| validation("catalog has no fallback_form"))?;
    if !forms.contains_key(fallback) {
        return Err(validation("fallback_form is not a defined form"));
    }
    for (name, form) in forms {
        let primitives = form
            .as_array()
            .ok_or_else(|| validation(format!("form {name} is not a list")))?;
        if primitives.len() as u64 > primitive_budget {
            return Err(validation(format!("form {name} exceeds primitive_budget")));
        }
        let mut particles = 0u64;
        for primitive in primitives {
            let tuple = primitive
                .as_array()
                .ok_or_else(|| validation(format!("form {name} primitive is not a tuple")))?;
            if tuple.len() != 8 {
                return Err(validation(format!(
                    "form {name} primitive tuple arity is not 8"
                )));
            }
            let kind = tuple[0]
                .as_str()
                .ok_or_else(|| validation("primitive kind must be a string"))?;
            if !ALLOWED_PRIMITIVES.contains(&kind) {
                return Err(validation(format!("primitive kind {kind} is not allowed")));
            }
            check_vec3(&tuple[3], true)?; // offset
            check_vec3(&tuple[4], true)?; // rotation
            check_vec3(&tuple[5], false)?; // scale — must be positive
            if kind == "points" {
                particles += tuple[6].as_u64().unwrap_or(0);
            }
        }
        if particles > particle_budget {
            return Err(validation(format!("form {name} exceeds particle_budget")));
        }
    }
    let lod = mapping
        .get("lod_states")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("catalog has no lod_states"))?;
    for residency in RESIDENCIES {
        let form = lod
            .get(residency)
            .and_then(Value::as_str)
            .ok_or_else(|| validation(format!("lod_states misses residency {residency}")))?;
        if !forms.contains_key(form) {
            return Err(validation(format!(
                "lod_states[{residency}] points at an undefined form"
            )));
        }
    }
    Ok(())
}

pub fn validate_policy(policy: &VisualPolicy) -> Result<(), UniverseError> {
    for state in EPISTEMIC_STATES {
        let modulation = policy
            .epistemic_modulation
            .get(state)
            .ok_or_else(|| validation(format!("epistemic state {state} is not modulated")))?;
        // Epistemic-honesty invariant: a non-confident state must NEVER be
        // rendered as a confident presence — no emission, and visibly reduced
        // opacity. This is the visual analogue of "never treat unknown as zero".
        if !modulation.confident {
            if modulation.emissive_scale != 0.0 {
                return Err(validation(format!(
                    "non-confident epistemic state {state} must not emit as if confident"
                )));
            }
            if !(modulation.opacity_scale < 1.0) {
                return Err(validation(format!(
                    "non-confident epistemic state {state} must be visually distinct"
                )));
            }
        }
    }
    if policy.bindings.is_empty() && policy.default_authority.is_none() {
        return Err(validation(
            "visual policy binds nothing and declares no fallback",
        ));
    }
    for binding in &policy.bindings {
        if binding.authority_id.trim().is_empty() || binding.justification.trim().is_empty() {
            return Err(validation(format!(
                "binding for {} is unjustified or unbound",
                binding.semantic_type
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Deterministic visual derivation (the computed projection).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedEmbodiment {
    pub semantic_type: String,
    pub authority_id: String,
    pub residency: String,
    pub epistemic_state: String,
    pub form_name: String,
    pub primitive_count: usize,
    pub material: Value,
    pub confident: bool,
}

/// Resolves the form name for a residency, falling back to the declared fallback
/// form when the residency's LOD form is undefined — never inventing a form.
fn resolve_form_name<'a>(mapping: &'a Value, residency: &str) -> Result<&'a str, UniverseError> {
    let forms = mapping
        .get("forms")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("catalog has no forms"))?;
    let fallback = mapping
        .get("fallback_form")
        .and_then(Value::as_str)
        .ok_or_else(|| validation("catalog has no fallback_form"))?;
    let requested = mapping
        .get("lod_states")
        .and_then(Value::as_object)
        .and_then(|lod| lod.get(residency))
        .and_then(Value::as_str);
    match requested {
        Some(form) if forms.contains_key(form) => Ok(form),
        _ => Ok(fallback),
    }
}

/// Derives the modulated material for an entity of `semantic_type` at a given
/// residency and epistemic state. Returns `None` when no authority resolves
/// (unbound and no fallback) — an honest "unknown", never a default form.
pub fn derive(
    policy: &VisualPolicy,
    catalog: &VisualCatalog,
    semantic_type: &str,
    residency: &str,
    epistemic_state: &str,
) -> Result<Option<ResolvedEmbodiment>, UniverseError> {
    let Some(authority) = policy.authority_for(semantic_type) else {
        return Ok(None);
    };
    if authority != catalog.authority_id {
        // The selected authority's catalog is not the one loaded; the caller
        // holds a single catalog, so this resolves to unknown rather than a
        // misapplied form.
        return Ok(None);
    }
    let modulation = policy
        .epistemic_modulation
        .get(epistemic_state)
        .ok_or_else(|| {
            validation(format!(
                "epistemic state {epistemic_state} is not modulated"
            ))
        })?;
    let form_name = resolve_form_name(&catalog.mapping, residency)?.to_owned();
    let primitive_count = catalog
        .mapping
        .get("forms")
        .and_then(|forms| forms.get(&form_name))
        .and_then(Value::as_array)
        .map(|form| form.len())
        .unwrap_or(0);

    let material = &catalog.mapping["material"];
    let palette = &catalog.mapping["palette"];
    let core_opacity = material
        .get("core_opacity")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let core_emissive = material
        .get("core_emissive_intensity")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let base_color = palette
        .get("core")
        .and_then(Value::as_str)
        .unwrap_or("#ffffff");
    let emissive_color = palette
        .get("emissive")
        .and_then(Value::as_str)
        .unwrap_or("#ffffff");
    let color = modulation
        .hue
        .clone()
        .unwrap_or_else(|| base_color.to_owned());

    let modulated = json!({
        "color": color,
        "emissive": emissive_color,
        "emissiveIntensity": core_emissive * modulation.emissive_scale,
        "opacity": core_opacity * modulation.opacity_scale,
        "desaturate": modulation.desaturate,
        "confident": modulation.confident,
    });

    Ok(Some(ResolvedEmbodiment {
        semantic_type: semantic_type.to_owned(),
        authority_id: authority.to_owned(),
        residency: residency.to_owned(),
        epistemic_state: epistemic_state.to_owned(),
        form_name,
        primitive_count,
        material: modulated,
        confident: modulation.confident,
    }))
}

// ---------------------------------------------------------------------------
// Materialization + independent readback.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VisualReceipt {
    pub kind: String,
    pub change_id: String,
    pub authority: String,
    pub status: String,
    pub universe: UniverseId,
    pub newly_committed: bool,
    pub authority_id: String,
    pub mapping_id: String,
    pub catalog_sha256: String,
    /// The catalog read back from the store is byte-identical to the fixture the
    /// renderer consumes.
    pub catalog_parity: bool,
    pub nodes_preserved: bool,
    pub forms_validated: usize,
    pub bindings: usize,
    pub resolutions_checked: usize,
    /// Every non-confident epistemic resolution emitted zero light and reduced
    /// opacity — the honesty invariant held under readback.
    pub honesty_invariant_held: bool,
    pub final_snapshot_hash: String,
}

fn build_seed(catalog: &VisualCatalog, policy: &VisualPolicy) -> Result<GraphSeed, UniverseError> {
    validate_catalog(catalog)?;
    validate_policy(policy)?;
    let catalog_sha256 = canonical_hash(&catalog.mapping)?;
    let mapping_id = catalog.mapping_id()?.to_owned();

    let symbols = vec![
        "visual_projection_contract".to_owned(),
        "visual_embodiment_mapping".to_owned(),
        "visual_embodiment_catalog".to_owned(),
        "visual_projection_changeset".to_owned(),
        "semantic_type".to_owned(),
        "GOVERNED_BY".to_owned(),
        "HAS_PAYLOAD".to_owned(),
        "PROJECTS_AS".to_owned(),
        "PART_OF".to_owned(),
    ];

    let mut entities = vec![
        seed_entity(
            CONTRACT_ATOM,
            "visual_projection_contract",
            json!({
                "kind": "visual_projection_contract",
                "contract_id": CONTRACT_ID,
                "output_kind": "visual_embodiment",
                "schema_version": SCHEMA_VERSION,
                "node_remains_authoritative": true,
                "asset_is_derived": true,
                "invalidation_signals": ["mapping_revision", "form_catalog_hash", "semantic_type_revision"],
            }),
        ),
        seed_entity(
            MAPPING_ATOM,
            "visual_embodiment_mapping",
            json!({
                "kind": "visual_embodiment_mapping",
                "authority_id": catalog.authority_id,
                "mapping_id": mapping_id,
                "output_kind": "visual_embodiment",
                "media_type": "application/vnd.mind.visual-embodiment+json",
                "catalog_sha256": catalog_sha256,
                "schema_version": SCHEMA_VERSION,
                "canonical_node_replaced": false,
            }),
        ),
        seed_entity(
            CATALOG_ATOM,
            "visual_embodiment_catalog",
            json!({
                "kind": "visual_embodiment_catalog",
                "content_address": format!("sha256:{catalog_sha256}"),
                "catalog_sha256": catalog_sha256,
                "value": catalog.mapping,
                "motion_profile": catalog.motion_profile,
            }),
        ),
        seed_entity(
            CHANGESET_ATOM,
            "visual_projection_changeset",
            json!({
                "kind": "visual_projection_changeset",
                "change_id": CHANGE_ID,
                "authority": AUTHORITY,
                "status": STATUS,
                "contract": CONTRACT_ATOM,
                "mapping": MAPPING_ATOM,
                "scope": [MAPPING_ATOM],
            }),
        ),
    ];

    let mut relations = vec![
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
    ];

    let mut relation_key = RELATION_BASE + 4;
    for (index, binding) in policy.bindings.iter().enumerate() {
        let semantic_atom = EntityKey(SEMANTIC_BASE + index as u128);
        entities.push(seed_entity(
            semantic_atom,
            "semantic_type",
            json!({
                "kind": "semantic_type",
                "canonical_id": binding.semantic_type,
            }),
        ));
        relations.push(seed_relation(
            relation_key,
            semantic_atom,
            MAPPING_ATOM,
            "PROJECTS_AS",
            Some(json!({
                "authority_id": binding.authority_id,
                "justification": binding.justification,
            })),
        ));
        relation_key += 1;
    }

    Ok(GraphSeed {
        universe: UNIVERSE,
        symbols,
        entities,
        relations,
    })
}

pub fn materialize(
    store_root: impl AsRef<Path>,
    catalog: &VisualCatalog,
    policy: &VisualPolicy,
) -> Result<VisualReceipt, UniverseError> {
    let store_root = store_root.as_ref();
    let seed = build_seed(catalog, policy)?;
    let catalog_sha256 = canonical_hash(&catalog.mapping)?;
    let mapping_id = catalog.mapping_id()?.to_owned();

    let store = UniverseStore::open(store_root)?;
    let newly_committed = !store_root.join("snapshot.json").exists();
    if newly_committed {
        store.install_seed(&seed)?;
    }

    // Independent readback: reopen, replay, and verify the catalog Asset is
    // present, byte-identical to the fixture, and its provenance links resolve.
    let readback_store = UniverseStore::open(store_root)?;
    let readback = readback_store.replay(readback_store.load_snapshot()?)?;

    let mapping_entity = readback
        .entities
        .iter()
        .find(|entity| entity.key == MAPPING_ATOM)
        .ok_or_else(|| validation("visual mapping Asset missing after reopen"))?;
    let mapping_content = mapping_entity
        .content
        .as_ref()
        .ok_or_else(|| validation("visual mapping Asset has no content"))
        .and_then(|content| readback_store.read_content(content))?;
    let nodes_preserved =
        mapping_content.get("canonical_node_replaced") == Some(&Value::Bool(false));

    let catalog_entity = readback
        .entities
        .iter()
        .find(|entity| entity.key == CATALOG_ATOM)
        .ok_or_else(|| validation("visual catalog payload missing after reopen"))?;
    let catalog_content = catalog_entity
        .content
        .as_ref()
        .ok_or_else(|| validation("visual catalog payload has no content"))
        .and_then(|content| readback_store.read_content(content))?;
    let read_mapping = catalog_content
        .get("value")
        .ok_or_else(|| validation("catalog payload has no value"))?;
    let catalog_parity = canonical_hash(read_mapping)? == catalog_sha256;
    if !catalog_parity {
        return Err(validation("read-back catalog differs from the fixture"));
    }

    let projects_as = readback
        .symbol_id("PROJECTS_AS")
        .ok_or_else(|| validation("PROJECTS_AS symbol absent"))?;
    let bindings = readback
        .relations
        .iter()
        .filter(|relation| relation.predicate == projects_as && relation.target == MAPPING_ATOM)
        .count();
    if bindings != policy.bindings.len() {
        return Err(validation("PROJECTS_AS bindings do not match the policy"));
    }

    // Exercise the computed projection across every binding × residency ×
    // epistemic state, and prove the honesty invariant on the readback path.
    let mut resolutions_checked = 0usize;
    let mut honesty_invariant_held = true;
    for binding in &policy.bindings {
        for residency in RESIDENCIES {
            for epistemic in EPISTEMIC_STATES {
                let Some(resolution) = derive(
                    policy,
                    catalog,
                    &binding.semantic_type,
                    residency,
                    epistemic,
                )?
                else {
                    continue;
                };
                resolutions_checked += 1;
                if !resolution.confident {
                    let emits = resolution
                        .material
                        .get("emissiveIntensity")
                        .and_then(Value::as_f64)
                        != Some(0.0);
                    let opaque = resolution
                        .material
                        .get("opacity")
                        .and_then(Value::as_f64)
                        .zip(
                            catalog.mapping["material"]
                                .get("core_opacity")
                                .and_then(Value::as_f64),
                        )
                        .is_some_and(|(shown, base)| shown >= base);
                    if emits || opaque {
                        honesty_invariant_held = false;
                    }
                }
            }
        }
    }
    if !honesty_invariant_held {
        return Err(validation(
            "a non-confident epistemic resolution rendered as confident",
        ));
    }

    let forms_validated = catalog
        .mapping
        .get("forms")
        .and_then(Value::as_object)
        .map(|forms| forms.len())
        .unwrap_or(0);

    Ok(VisualReceipt {
        kind: "visual_mapping_materialization_receipt".into(),
        change_id: CHANGE_ID.into(),
        authority: AUTHORITY.into(),
        status: STATUS.into(),
        universe: readback.universe,
        newly_committed,
        authority_id: catalog.authority_id.clone(),
        mapping_id,
        catalog_sha256,
        catalog_parity,
        nodes_preserved,
        forms_validated,
        bindings,
        resolutions_checked,
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

fn check_vec3(value: &Value, allow_nonpositive: bool) -> Result<(), UniverseError> {
    let components = value
        .as_array()
        .ok_or_else(|| validation("vector must be an array"))?;
    if components.len() != 3 {
        return Err(validation("vector must have length 3"));
    }
    for component in components {
        let number = component
            .as_f64()
            .ok_or_else(|| validation("vector component must be a number"))?;
        if !number.is_finite() {
            return Err(validation("vector component must be finite"));
        }
        if !allow_nonpositive && !(number > 0.0) {
            return Err(validation("scale component must be positive"));
        }
    }
    Ok(())
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> VisualCatalog {
        VisualCatalog::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/assets/visual-embodiment-catalog.json"),
        )
        .unwrap()
    }

    fn policy() -> VisualPolicy {
        VisualPolicy::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/assets/visual-projection-policy.json"),
        )
        .unwrap()
    }

    #[test]
    fn materializes_catalog_with_independent_parity() {
        let temp = tempfile::tempdir().unwrap();
        let receipt = materialize(temp.path().join("store"), &catalog(), &policy()).unwrap();
        assert!(receipt.newly_committed);
        assert!(receipt.catalog_parity);
        assert!(receipt.nodes_preserved);
        assert!(receipt.honesty_invariant_held);
        assert_eq!(receipt.forms_validated, 2);
        assert_eq!(receipt.bindings, 1);
        // 1 binding × 4 residencies × 6 epistemic states.
        assert_eq!(receipt.resolutions_checked, 24);
        assert_eq!(receipt.mapping_id, "citizen-energy-semi-humanoid-v1");
    }

    #[test]
    fn hot_residency_is_humanoid_lower_is_orb() {
        let (catalog, policy) = (catalog(), policy());
        let hot = derive(&policy, &catalog, "actor", "hot", "measured")
            .unwrap()
            .unwrap();
        assert_eq!(hot.form_name, "semi_humanoid");
        assert!(hot.confident);
        let dormant = derive(&policy, &catalog, "actor", "dormant", "measured")
            .unwrap()
            .unwrap();
        assert_eq!(dormant.form_name, "energy_orb");
    }

    #[test]
    fn unknown_state_never_emits_and_stays_distinct() {
        let (catalog, policy) = (catalog(), policy());
        let unknown = derive(&policy, &catalog, "actor", "hot", "unknown")
            .unwrap()
            .unwrap();
        assert!(!unknown.confident);
        assert_eq!(
            unknown
                .material
                .get("emissiveIntensity")
                .and_then(Value::as_f64),
            Some(0.0)
        );
        let opacity = unknown
            .material
            .get("opacity")
            .and_then(Value::as_f64)
            .unwrap();
        let base = catalog.mapping["material"]["core_opacity"]
            .as_f64()
            .unwrap();
        assert!(opacity < base);
    }

    #[test]
    fn unbound_type_without_binding_resolves_to_unknown_not_a_default() {
        let catalog = catalog();
        let mut policy = policy();
        policy.default_authority = None; // remove the fallback
        assert!(derive(
            &policy,
            &catalog,
            "thing_without_binding",
            "hot",
            "measured"
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn dishonest_modulation_is_rejected() {
        let mut policy = policy();
        // Force `unknown` to emit as if confident — must be refused.
        let modulation = policy.epistemic_modulation.get_mut("unknown").unwrap();
        modulation.emissive_scale = 1.0;
        assert!(matches!(
            validate_policy(&policy),
            Err(UniverseError::Validation(message)) if message.contains("must not emit as if confident")
        ));
    }

    #[test]
    fn budget_violation_is_rejected() {
        let mut catalog = catalog();
        catalog.mapping["primitive_budget"] = json!(99);
        assert!(matches!(
            validate_catalog(&catalog),
            Err(UniverseError::Validation(message)) if message.contains("primitive_budget out of renderer range")
        ));
    }
}
