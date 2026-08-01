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
// Closed renderer primitive palette. The celestial five (icosphere..fresnel_shell)
// dress luminous constructs (Sky); the hard-edged six (box..tube) dress figurative
// constructs whose form is a MATERIALIZED affordance (Appearance toolkit — a cup, a
// module, a room). Extending this set is an attributable renderer change, like a new
// opcode; an authored form reaching outside it is refused, never silently redrawn.
const ALLOWED_PRIMITIVES: [&str; 11] = [
    "icosphere",
    "sphere",
    "capsule",
    "points",
    "fresnel_shell",
    "box",
    "cylinder",
    "cone",
    "torus",
    "plane",
    "tube",
];
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

/// One toolkit's claim on appearance: the construct whose products it dresses,
/// and WHERE that appearance is held. A `binding_member` names a node carried AS
/// a member of the construct — the toolkit's own appearance, resolvable from the
/// graph. A `binding_authority` names a standalone catalog instead.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolkitVisualBinding {
    /// The producing construct, e.g. `space:l2:mind-universe:underground-toolkit-v0`.
    pub toolkit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_member: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_authority: Option<String>,
    pub justification: String,
}

/// The resolution policy. It loads BOTH authored shapes: the v0 table keyed by
/// `semantic_type` (`bindings`), and the v1 provenance shape keyed by producing
/// toolkit (`toolkit_bindings`). Before this, the v1 document could not be
/// deserialized at all — `bindings` was required and its `epistemic_modulation`
/// carries a prose `note` — so the world declared one policy and ran another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VisualPolicy {
    pub policy_id: String,
    #[serde(default)]
    pub bindings: Vec<VisualBinding>,
    #[serde(default)]
    pub toolkit_bindings: Vec<ToolkitVisualBinding>,
    /// The honest terminal case: what a node with no reachable binding looks like.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_presence: Option<Value>,
    #[serde(deserialize_with = "modulation_table")]
    pub epistemic_modulation: BTreeMap<String, Modulation>,
}

/// Reads the epistemic-modulation table, keeping the prose keys authors write
/// alongside the states (a `note` explaining the grammar). A string entry is
/// documentation and is skipped; anything else that is not a modulation is a
/// malformed state and still fails loudly.
fn modulation_table<'de, D>(deserializer: D) -> Result<BTreeMap<String, Modulation>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let raw = BTreeMap::<String, Value>::deserialize(deserializer)?;
    let mut table = BTreeMap::new();
    for (key, value) in raw {
        if value.is_string() {
            continue;
        }
        let modulation: Modulation = serde_json::from_value(value)
            .map_err(|error| D::Error::custom(format!("epistemic state {key}: {error}")))?;
        table.insert(key, modulation);
    }
    Ok(table)
}

impl VisualPolicy {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, UniverseError> {
        let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| UniverseError::CorruptContent(error.to_string()))
    }

    /// The visual mapping authority a semantic type resolves to: its explicit
    /// per-toolkit binding, or `None`. There is NO universal default — an
    /// unbound type resolves to nothing (the caller renders the honest
    /// fallback/fog), never a default form dressing every node the same.
    pub fn authority_for(&self, semantic_type: &str) -> Option<&str> {
        self.bindings
            .iter()
            .find(|binding| binding.semantic_type == semantic_type)
            .map(|binding| binding.authority_id.as_str())
    }

    /// The graph node carrying the appearance for a producing construct — the
    /// toolkit's own visual binding, held AS one of its members. `None` means this
    /// toolkit registered no binding, and its products stay honestly unbound.
    pub fn binding_member_for(&self, toolkit: &str) -> Option<&str> {
        self.toolkit_bindings
            .iter()
            .find(|binding| binding.toolkit == toolkit)
            .and_then(|binding| binding.binding_member.as_deref())
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
    if let Some(dynamics) = mapping.get("dynamics") {
        validate_dynamics(dynamics)?;
    }
    Ok(())
}

/// Validates the optional per-node `dynamics` block: the graph-declared bounds
/// the renderer uses to modulate a base form by a node's live signals — energy
/// (emission), weight/poids (scale), and embedding (orientation + micro-variation).
/// The block is authority for HOW those signals project; keeping the bounds sane
/// here means the renderer only ever derives within a validated envelope.
fn validate_dynamics(dynamics: &Value) -> Result<(), UniverseError> {
    let block = dynamics
        .as_object()
        .ok_or_else(|| validation("dynamics must be an object"))?;
    // Each mapBounded tuple is [inMin, inMax, outMin, outMax]: the input span must
    // be non-degenerate and the output (a multiplier) non-negative and finite.
    // A channel ABSENT from the block is not a structural fault: WHICH channels a
    // kit must declare for its radius is a coverage/totality question that
    // `compute_coverage` owns (it names the missing config). Here we validate only
    // the shape of a channel that IS present — so removing a channel surfaces as a
    // named coverage hole, not a generic structural error that pre-empts it.
    for key in ["energy_to_emissive", "weight_to_scale"] {
        let Some(value) = block.get(key) else {
            continue;
        };
        let tuple = value
            .as_array()
            .ok_or_else(|| validation(format!("dynamics.{key} must be a 4-tuple")))?;
        if tuple.len() != 4 {
            return Err(validation(format!("dynamics.{key} arity is not 4")));
        }
        let values: Vec<f64> = tuple
            .iter()
            .map(|v| v.as_f64().filter(|n| n.is_finite()))
            .collect::<Option<_>>()
            .ok_or_else(|| validation(format!("dynamics.{key} has a non-finite component")))?;
        if !(values[1] > values[0]) {
            return Err(validation(format!(
                "dynamics.{key} input span must be non-degenerate"
            )));
        }
        if values[2] < 0.0 || values[3] < 0.0 {
            return Err(validation(format!(
                "dynamics.{key} output multiplier must be non-negative"
            )));
        }
    }
    if let Some(value) = block.get("embedding_orientation_max_rad") {
        value
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .ok_or_else(|| {
                validation(
                    "dynamics.embedding_orientation_max_rad must be a finite, non-negative number",
                )
            })?;
    }
    if let Some(value) = block.get("embedding_microvariation") {
        let microvariation = value
            .as_f64()
            .ok_or_else(|| validation("dynamics.embedding_microvariation must be a number"))?;
        if !microvariation.is_finite() || !(0.0..=1.0).contains(&microvariation) {
            return Err(validation(
                "dynamics.embedding_microvariation must be a fraction in [0, 1]",
            ));
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
    if policy.bindings.is_empty() && policy.toolkit_bindings.is_empty() {
        return Err(validation("visual policy binds nothing"));
    }
    for binding in &policy.bindings {
        if binding.authority_id.trim().is_empty() || binding.justification.trim().is_empty() {
            return Err(validation(format!(
                "binding for {} is unjustified or unbound",
                binding.semantic_type
            )));
        }
    }
    // A toolkit binding must name a producing construct, say WHERE the appearance
    // is held, and justify why that dress is a legitimate reading of what the
    // toolkit makes. An unjustified binding is an assertion, not an authority.
    for binding in &policy.toolkit_bindings {
        if binding.toolkit.trim().is_empty() || binding.justification.trim().is_empty() {
            return Err(validation(format!(
                "toolkit binding for {} is unjustified or unbound",
                binding.toolkit
            )));
        }
        if binding.binding_member.is_none() && binding.binding_authority.is_none() {
            return Err(validation(format!(
                "toolkit binding for {} names no binding member or authority",
                binding.toolkit
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
// Totality / coverage — the kit as a total function on its declared radius.
// ---------------------------------------------------------------------------
//
// STRUCTURE (`validate_catalog`) proves the kit is well-formed. TOTALITY proves
// the central missing property: for EVERY configuration in its declared domain,
// the kit emits exactly one defined render instruction — zero holes. A config in
// the radius that produces nothing is a hole and is as faulty as a fabricated
// value. A kit MUST declare its radius; without it, "all configs" is undefined,
// so the observer rejects the kit rather than converting a missing check into
// success ("fog stays fog").

/// The closed 5-role axis from ALIGN.md §5b — what a node does with energy. A
/// node's form derives from its role, not its content_kind. The role name is the
/// `semantic_type` the render mapping is keyed by.
const ROLE_AXIS: [&str; 5] = ["space", "actor", "narrative", "moment", "thing"];
/// The live per-node signals a node kit may read (catalog `dynamics`).
const SIGNALS: [&str; 3] = ["energy", "weight", "embedding"];
/// The tri-state each signal takes across the coverage radius. `measured`
/// displays the channel; `absent`/`not_measured` MUST stay at identity/fog —
/// emitting the honest instruction of the unknown case is required (totality),
/// but rendering it as a confident value is fabrication (equally a failure).
const SIGNAL_STATES: [&str; 3] = ["measured", "absent", "not_measured"];
/// The `dynamics` mapBounded key each signal channel is displayed through when
/// `measured`. A `measured` signal with no such binding cannot be rendered.
const SIGNAL_DYNAMICS: [(&str, &str); 3] = [
    ("energy", "energy_to_emissive"),
    ("weight", "weight_to_scale"),
    ("embedding", "embedding_orientation_max_rad"),
];

/// A kit's declared coverage domain. Configs outside it are explicit
/// out-of-scope, never silently dropped.
#[derive(Clone, Debug, PartialEq)]
struct RadiusDecl {
    role_axis: Vec<String>,
    signals: Vec<String>,
}

/// One configuration in the radius for which the kit failed to emit exactly one
/// honest render instruction — named exactly so the fault is inspectable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoverageHole {
    pub role: String,
    pub lod: String,
    pub signal: String,
    pub signal_state: String,
    pub reason: String,
}

impl CoverageHole {
    /// The exact config tuple, e.g. `{role: narrative, lod: hot, signal: energy, state: not_measured}`.
    pub fn config(&self) -> String {
        format!(
            "{{role: {}, lod: {}, signal: {}, state: {}}}",
            self.role, self.lod, self.signal, self.signal_state
        )
    }
}

/// The measured coverage of a kit over its declared radius.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoverageReport {
    /// `|radius|` = |role_axis| × 4 lod × |signals| × 3 signal-states.
    pub radius_size: usize,
    pub defined: usize,
    /// `defined / |radius|`.
    pub coverage: f64,
    /// The named list of holes — empty iff the kit is total on its radius.
    pub holes: Vec<CoverageHole>,
    /// Roles of the closed axis the kit does NOT cover — honest out-of-scope,
    /// reported rather than dropped.
    pub out_of_radius_roles: Vec<String>,
    pub role_axis: Vec<String>,
    pub signals: Vec<String>,
}

impl CoverageReport {
    pub fn is_total(&self) -> bool {
        self.holes.is_empty()
    }
}

fn parse_str_set(
    value: Option<&Value>,
    allowed: &[&str],
    field: &str,
) -> Result<Vec<String>, UniverseError> {
    let array = value
        .and_then(Value::as_array)
        .ok_or_else(|| validation(format!("radius {field} must be an array of strings")))?;
    let mut out: Vec<String> = Vec::new();
    for item in array {
        let entry = item
            .as_str()
            .ok_or_else(|| validation(format!("radius {field} entries must be strings")))?;
        if !allowed.contains(&entry) {
            return Err(validation(format!(
                "radius {field} value '{entry}' is outside the closed axis {allowed:?}"
            )));
        }
        if out.iter().any(|existing| existing == entry) {
            return Err(validation(format!(
                "radius {field} lists '{entry}' more than once"
            )));
        }
        out.push(entry.to_owned());
    }
    Ok(out)
}

/// Reads the kit's declared radius. A kit that omits `radius` is rejected — the
/// observer never treats "all configs" as an implicit, undefined domain.
fn parse_radius(mapping: &Value) -> Result<RadiusDecl, UniverseError> {
    let radius = mapping.get("radius").and_then(Value::as_object).ok_or_else(|| {
        validation(
            "kit declares no radius; totality over 'all configs' is undefined \u{2014} a kit without a declared radius is rejected, not passed",
        )
    })?;
    let role_axis = parse_str_set(radius.get("role_axis"), &ROLE_AXIS, "role_axis")?;
    if role_axis.is_empty() {
        return Err(validation(
            "radius role_axis is empty; a kit must declare at least one role it covers",
        ));
    }
    let signals = parse_str_set(radius.get("signals"), &SIGNALS, "signals")?;
    if signals.is_empty() {
        return Err(validation(
            "radius declares no signals; the v0 node-kit radius reads at least one of energy/weight/embedding",
        ));
    }
    Ok(RadiusDecl { role_axis, signals })
}

/// The epistemic state a signal tri-state folds into on the derive/honesty path.
fn epistemic_for_signal_state(signal_state: &str) -> &'static str {
    match signal_state {
        "measured" => "measured",
        "absent" => "known_absent",
        // `not_measured` and any other non-confident case: identity / fog.
        _ => "not_measured",
    }
}

fn dynamics_key_for(signal: &str) -> Option<&'static str> {
    SIGNAL_DYNAMICS
        .iter()
        .find(|(name, _)| *name == signal)
        .map(|(_, key)| *key)
}

fn has_dynamics_binding(mapping: &Value, signal: &str) -> bool {
    dynamics_key_for(signal)
        .and_then(|key| mapping.get("dynamics").and_then(|dynamics| dynamics.get(key)))
        .is_some()
}

fn emissive_intensity(resolution: &ResolvedEmbodiment) -> Option<f64> {
    resolution
        .material
        .get("emissiveIntensity")
        .and_then(Value::as_f64)
}

/// Enumerates the cartesian product of the declared radius, runs the kit's
/// render mapping on each point, and requires exactly one defined honest
/// instruction per point. Errors only when the radius itself is undefined; a kit
/// that is structurally valid but leaves holes returns `Ok` with the named holes
/// so the caller can measure `coverage` — `validate_coverage` turns any hole into
/// a rejection.
pub fn compute_coverage(
    policy: &VisualPolicy,
    catalog: &VisualCatalog,
) -> Result<CoverageReport, UniverseError> {
    let radius = parse_radius(&catalog.mapping)?;
    let mut holes = Vec::new();
    let mut radius_size = 0usize;

    for role in &radius.role_axis {
        // ALIGN §5b: form derives from the role; the role name is the semantic
        // type the policy binding keys the render authority by.
        for lod in RESIDENCIES {
            for signal in &radius.signals {
                for signal_state in SIGNAL_STATES {
                    radius_size += 1;
                    let epistemic = epistemic_for_signal_state(signal_state);
                    let instruction = derive(policy, catalog, role, lod, epistemic)?;
                    match instruction {
                        None => holes.push(CoverageHole {
                            role: role.clone(),
                            lod: lod.to_owned(),
                            signal: signal.clone(),
                            signal_state: signal_state.to_owned(),
                            reason: format!(
                                "role '{role}' has no render mapping (unbound in policy and no fallback) \u{2014} the kit emits nothing for this config"
                            ),
                        }),
                        Some(resolution) => {
                            if signal_state == "measured" {
                                // The signal channel is displayed only through a
                                // dynamics binding; without it the kit cannot
                                // render this measured signal.
                                if !has_dynamics_binding(&catalog.mapping, signal) {
                                    let key = dynamics_key_for(signal).unwrap_or("<unknown>");
                                    holes.push(CoverageHole {
                                        role: role.clone(),
                                        lod: lod.to_owned(),
                                        signal: signal.clone(),
                                        signal_state: signal_state.to_owned(),
                                        reason: format!(
                                            "signal '{signal}' is measured but the kit declares no dynamics binding ('{key}') to render it"
                                        ),
                                    });
                                }
                            } else {
                                // absent / not_measured MUST be identity/fog: no
                                // emission, not confident. Rendering it as a
                                // measured value is fabrication.
                                let emits = emissive_intensity(&resolution) != Some(0.0);
                                if resolution.confident || emits {
                                    holes.push(CoverageHole {
                                        role: role.clone(),
                                        lod: lod.to_owned(),
                                        signal: signal.clone(),
                                        signal_state: signal_state.to_owned(),
                                        reason: format!(
                                            "signal '{signal}' is {signal_state} but the kit renders it as a confident measured value (fabrication) instead of identity/fog"
                                        ),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let defined = radius_size - holes.len();
    let coverage = if radius_size == 0 {
        0.0
    } else {
        defined as f64 / radius_size as f64
    };
    let out_of_radius_roles = ROLE_AXIS
        .iter()
        .filter(|role| !radius.role_axis.iter().any(|declared| declared == *role))
        .map(|role| role.to_string())
        .collect();

    Ok(CoverageReport {
        radius_size,
        defined,
        coverage,
        holes,
        out_of_radius_roles,
        role_axis: radius.role_axis,
        signals: radius.signals,
    })
}

/// The toolkit validator loop's observer: STRUCTURE first (a kit that is not
/// well-formed cannot be a total function), then TOTALITY. Any structural fault
/// or any coverage hole is a rejection that names the exact failing config; the
/// observer never converts a missing radius or a hole into success.
pub fn validate_coverage(
    policy: &VisualPolicy,
    catalog: &VisualCatalog,
) -> Result<CoverageReport, UniverseError> {
    validate_catalog(catalog)?;
    validate_policy(policy)?;
    let report = compute_coverage(policy, catalog)?;
    if !report.holes.is_empty() {
        let named = report
            .holes
            .iter()
            .map(|hole| format!("{} \u{2014} {}", hole.config(), hole.reason))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(validation(format!(
            "kit is not total on its declared radius: {}/{} configs covered, {} hole(s): {named}",
            report.defined,
            report.radius_size,
            report.holes.len()
        )));
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// Validation chrome — the honesty palette for a Construct's self-report.
// ---------------------------------------------------------------------------
//
// The `validation_chrome` block is DATA (authored in the visual-embodiment
// catalog): it names, per axis-value, a palette ROLE (never a hex). The kit
// palette resolves a role to a colour; this module only looks the value up in
// the block and hands back the role. The honesty invariant lives in the DATA:
// every `not_measured`/`unknown` axis value maps to the `fog` role, so the
// compiler cannot paint confidence it was not given.
//
// TOTALITY mirrors the coverage discipline above: the block must cover EVERY
// enum value of each of the four axes. A missing value is a coverage hole and
// is rejected — the observer never treats an unmapped value as an implicit
// pass.

/// The four honesty axes and the closed enum each ranges over. A
/// `validation_chrome` block must map every value of every axis to a role.
const CHROME_CORRECTNESS: [&str; 5] =
    ["correct", "chained", "logical_error", "incomplete", "unknown"];
const CHROME_TRUST: [&str; 4] = ["strong", "adequate", "weak", "not_measured"];
const CHROME_HEALTH: [&str; 6] = [
    "healthy",
    "degraded",
    "stale",
    "measurement_failed",
    "unknown",
    "not_measured",
];
const CHROME_EPISTEMIC: [&str; 6] = [
    "observed",
    "measured",
    "known_absent",
    "measurement_failed",
    "unknown",
    "not_measured",
];
const CHROME_AXES: [(&str, &[&str]); 4] = [
    ("correctness", &CHROME_CORRECTNESS),
    ("trust", &CHROME_TRUST),
    ("health", &CHROME_HEALTH),
    ("epistemic", &CHROME_EPISTEMIC),
];
/// The closed set of palette roles a block may name. The kit palette resolves a
/// role to an actual colour; the DATA never names a hex.
const CHROME_ROLES: [&str; 5] = ["good", "warn", "bad", "neutral", "fog"];

/// Validates a `validation_chrome` block: it must cover every enum value of each
/// of the four axes (a missing value is a coverage hole → hard error, mirroring
/// the totality discipline of `compute_coverage`); every mapped role must be
/// from the allowed set; `fog_alpha ∈ [0, 1]`; and every emissive/brightness
/// scalar must be finite and non-negative. No axis-value→role knowledge is
/// hard-coded here — only the shape and totality of the block are checked.
pub fn validate_validation_chrome(block: &Value) -> Result<(), UniverseError> {
    let obj = block
        .as_object()
        .ok_or_else(|| validation("validation_chrome must be an object"))?;
    let axis_palette = obj
        .get("axis_palette")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("validation_chrome has no axis_palette"))?;
    for (axis, values) in CHROME_AXES {
        let palette = axis_palette
            .get(axis)
            .and_then(Value::as_object)
            .ok_or_else(|| validation(format!("axis_palette misses axis {axis}")))?;
        for value in values {
            let role = palette
                .get(*value)
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    validation(format!(
                        "axis_palette[{axis}] coverage hole: enum value '{value}' is unmapped"
                    ))
                })?;
            if !CHROME_ROLES.contains(&role) {
                return Err(validation(format!(
                    "axis_palette[{axis}][{value}] role '{role}' is not an allowed palette role {CHROME_ROLES:?}"
                )));
            }
        }
    }
    let fog_alpha = obj
        .get("fog_alpha")
        .and_then(Value::as_f64)
        .ok_or_else(|| validation("validation_chrome.fog_alpha must be a number"))?;
    if !fog_alpha.is_finite() || !(0.0..=1.0).contains(&fog_alpha) {
        return Err(validation("validation_chrome.fog_alpha must be in [0, 1]"));
    }
    let liveness = obj
        .get("liveness_emissive")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("validation_chrome has no liveness_emissive"))?;
    for key in ["cold", "pulsing"] {
        let value = liveness
            .get(key)
            .and_then(Value::as_f64)
            .ok_or_else(|| validation(format!("liveness_emissive.{key} must be a number")))?;
        if !value.is_finite() || value < 0.0 {
            return Err(validation(format!(
                "liveness_emissive.{key} must be finite and non-negative"
            )));
        }
    }
    let orb = obj
        .get("receipt_orb")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("validation_chrome has no receipt_orb"))?;
    for key in ["fresh_brightness", "stale_brightness"] {
        let value = orb
            .get(key)
            .and_then(Value::as_f64)
            .ok_or_else(|| validation(format!("receipt_orb.{key} must be a number")))?;
        if !value.is_finite() || value < 0.0 {
            return Err(validation(format!(
                "receipt_orb.{key} must be finite and non-negative"
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ConstructChrome → visual instructions (the compile, generic).
// ---------------------------------------------------------------------------
//
// FROZEN INPUT (produced by `bin/construct_chrome.rs`): one ConstructChrome
// per construct. The compiler maps it to render instructions using ONLY the
// `validation_chrome` block — a generic block lookup + role resolution, with NO
// state→role/colour logic in Rust. The block is the authority for what a state
// looks like; this code only reads it.

/// The four resolved honesty axes of a Construct (the block keys them by value).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChromeAxes {
    pub correctness: String,
    pub trust: String,
    pub health: String,
    pub epistemic: String,
}

/// One receipt the Construct carries — an orb whose brightness reads freshness.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChromeReceipt {
    pub kind: String,
    pub id: String,
    pub fresh: bool,
}

/// The frozen input shape: one Construct's self-report, ready to be dressed by
/// the block. Nothing here names a colour or a role — only measured/axis state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConstructChrome {
    pub construct: String,
    pub name: String,
    pub lifecycle: String,
    pub axes: ChromeAxes,
    pub fog: bool,
    pub liveness: String,
    #[serde(default)]
    pub needs_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_need: Option<String>,
    #[serde(default)]
    pub receipts: Vec<ChromeReceipt>,
}

/// One ring: an honesty axis resolved to its palette role.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RingRole {
    pub axis: String,
    pub role: String,
}

/// One receipt orb, its brightness resolved from the block's fresh/stale scalars.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReceiptOrb {
    pub id: String,
    pub brightness: f64,
}

/// The render instructions the compiler emits for one Construct — the whole of
/// what the renderer needs to dress it, every value read from the block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChromeInstructions {
    pub ring_roles: Vec<RingRole>,
    pub body_alpha: f64,
    pub emissive: f64,
    pub receipt_orbs: Vec<ReceiptOrb>,
}

/// Compiles one ConstructChrome into visual instructions by pure block lookup:
/// each axis value is resolved to a role through `axis_palette`; body alpha is
/// the block's `fog_alpha` when the Construct is fogged (else fully present);
/// emissive is the block's `liveness_emissive` for the Construct's liveness; and
/// each receipt orb takes the block's fresh/stale brightness. No state→role or
/// state→colour mapping is authored in Rust — the block is the only authority.
pub fn compile_chrome(
    block: &Value,
    chrome: &ConstructChrome,
) -> Result<ChromeInstructions, UniverseError> {
    validate_validation_chrome(block)?;
    let axis_palette = block
        .get("axis_palette")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("validation_chrome has no axis_palette"))?;

    let axis_values = [
        ("correctness", chrome.axes.correctness.as_str()),
        ("trust", chrome.axes.trust.as_str()),
        ("health", chrome.axes.health.as_str()),
        ("epistemic", chrome.axes.epistemic.as_str()),
    ];
    let mut ring_roles = Vec::with_capacity(axis_values.len());
    for (axis, value) in axis_values {
        let role = axis_palette
            .get(axis)
            .and_then(Value::as_object)
            .and_then(|palette| palette.get(value))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                validation(format!(
                    "construct axis {axis} value '{value}' is not a defined enum value in the block"
                ))
            })?;
        ring_roles.push(RingRole {
            axis: axis.to_owned(),
            role: role.to_owned(),
        });
    }

    let fog_alpha = block
        .get("fog_alpha")
        .and_then(Value::as_f64)
        .ok_or_else(|| validation("validation_chrome.fog_alpha must be a number"))?;
    let body_alpha = if chrome.fog { fog_alpha } else { 1.0 };

    let emissive = block
        .get("liveness_emissive")
        .and_then(Value::as_object)
        .and_then(|liveness| liveness.get(chrome.liveness.as_str()))
        .and_then(Value::as_f64)
        .ok_or_else(|| {
            validation(format!(
                "liveness '{}' is not a defined liveness_emissive key in the block",
                chrome.liveness
            ))
        })?;

    let orb = block
        .get("receipt_orb")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("validation_chrome has no receipt_orb"))?;
    let fresh_brightness = orb
        .get("fresh_brightness")
        .and_then(Value::as_f64)
        .ok_or_else(|| validation("receipt_orb.fresh_brightness must be a number"))?;
    let stale_brightness = orb
        .get("stale_brightness")
        .and_then(Value::as_f64)
        .ok_or_else(|| validation("receipt_orb.stale_brightness must be a number"))?;
    let receipt_orbs = chrome
        .receipts
        .iter()
        .map(|receipt| ReceiptOrb {
            id: receipt.id.clone(),
            brightness: if receipt.fresh {
                fresh_brightness
            } else {
                stale_brightness
            },
        })
        .collect();

    Ok(ChromeInstructions {
        ring_roles,
        body_alpha,
        emissive,
        receipt_orbs,
    })
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
    // This path materializes the semantic-type table into the graph. A policy that
    // binds only by producing toolkit has nothing for it to seed — and seeding an
    // empty table would report success for a materialization that did not happen.
    if policy.bindings.is_empty() {
        return Err(validation(
            "policy declares only toolkit bindings; the graph seed path needs semantic-type bindings",
        ));
    }
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
    fn the_declared_resolution_policy_is_a_policy_this_authority_can_run() {
        // The v1 document could not be deserialized at all: `bindings` was a
        // required field it does not carry, and its `epistemic_modulation` holds a
        // prose `note`. The world declared one policy and the pipe ran another.
        let policy = VisualPolicy::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/assets/visual-resolution-policy-v1.json"),
        )
        .expect("the declared resolution policy must load");
        assert_eq!(policy.policy_id, "visual-resolution-policy-v1");
        validate_policy(&policy).expect("the declared resolution policy must validate");
        // It binds by producing toolkit, not by semantic type.
        assert!(policy.bindings.is_empty());
        assert_eq!(
            policy.binding_member_for("space:l2:mind-universe:underground-toolkit-v0"),
            Some("visual_binding:l2:mind-universe:underground-toolkit-v0")
        );
        // A toolkit that registered no binding is not given someone else's dress.
        assert_eq!(policy.binding_member_for("space:l2:lumina-prime:energy-pen-v0"), None);
        // The prose note is documentation, not a state; the six states survive it.
        assert_eq!(policy.epistemic_modulation.len(), EPISTEMIC_STATES.len());
        assert!(policy.fallback_presence.is_some());
    }

    #[test]
    fn a_toolkit_only_policy_cannot_silently_seed_an_empty_semantic_type_table() {
        let policy = VisualPolicy::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/assets/visual-resolution-policy-v1.json"),
        )
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let error = materialize(temp.path().join("store"), &catalog(), &policy).unwrap_err();
        assert!(error.to_string().contains("semantic-type bindings"));
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
        // No universal default exists any more: an unbound type resolves to None.
        let policy = policy();
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
    fn dynamics_block_validates_and_is_present_in_the_authority() {
        let catalog = catalog();
        // The shipped authority declares the per-node modulation envelope.
        assert!(catalog.mapping.get("dynamics").is_some());
        validate_catalog(&catalog).unwrap();
    }

    #[test]
    fn degenerate_dynamics_span_is_rejected() {
        let mut catalog = catalog();
        // inMax must exceed inMin, else the projection has no envelope to derive in.
        catalog.mapping["dynamics"]["weight_to_scale"] = json!([100, 100, 0.85, 1.7]);
        assert!(matches!(
            validate_catalog(&catalog),
            Err(UniverseError::Validation(message))
                if message.contains("input span must be non-degenerate")
        ));
    }

    #[test]
    fn microvariation_out_of_unit_range_is_rejected() {
        let mut catalog = catalog();
        catalog.mapping["dynamics"]["embedding_microvariation"] = json!(1.5);
        assert!(matches!(
            validate_catalog(&catalog),
            Err(UniverseError::Validation(message))
                if message.contains("must be a fraction in [0, 1]")
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

    // -----------------------------------------------------------------------
    // Totality / coverage (T-K1).
    // -----------------------------------------------------------------------

    /// (c) The real fixture, once it declares a valid radius, is a total function
    /// on that radius: every one of its 36 configs emits an honest instruction.
    #[test]
    fn real_catalog_is_total_on_its_declared_radius() {
        let report = validate_coverage(&policy(), &catalog()).unwrap();
        assert!(report.is_total());
        assert!(report.holes.is_empty());
        // 1 role (actor) × 4 lod × 3 signals × 3 signal-states.
        assert_eq!(report.radius_size, 36);
        assert_eq!(report.defined, 36);
        assert_eq!(report.coverage, 1.0);
        assert_eq!(report.role_axis, vec!["actor".to_owned()]);
        // The four uncovered roles are reported as honest out-of-scope, not holes.
        assert_eq!(
            report.out_of_radius_roles,
            vec![
                "space".to_owned(),
                "narrative".to_owned(),
                "moment".to_owned(),
                "thing".to_owned()
            ]
        );
    }

    /// (d) A kit that declares no radius is rejected — "all configs" is undefined,
    /// and the observer never converts a missing check into success.
    #[test]
    fn kit_without_a_declared_radius_is_rejected() {
        let mut catalog = catalog();
        catalog.mapping.as_object_mut().unwrap().remove("radius");
        assert!(matches!(
            validate_coverage(&policy(), &catalog),
            Err(UniverseError::Validation(message)) if message.contains("declares no radius")
        ));
    }

    /// (b), form 1 — a declared role with no render mapping is a named hole.
    #[test]
    fn coverage_hole_from_unbound_role_names_the_missing_config() {
        let mut catalog = catalog();
        // Declare a role the policy cannot resolve...
        catalog.mapping["radius"]["role_axis"] = json!(["actor", "narrative"]);
        // `narrative` has no binding, and there is no universal default, so it
        // resolves to nothing — a named hole.
        let policy = policy();

        let report = compute_coverage(&policy, &catalog).unwrap();
        assert!(!report.is_total());
        // narrative × 4 lod × 3 signals × 3 states = 36 holes; actor stays total.
        assert_eq!(report.holes.len(), 36);
        assert_eq!(report.defined, 36);
        assert_eq!(report.radius_size, 72);
        assert!(report.holes.iter().all(|hole| hole.role == "narrative"));
        assert!(report
            .holes
            .iter()
            .any(|hole| hole.config().contains("role: narrative")
                && hole.reason.contains("no render mapping")));

        // The observer turns that into a rejection naming the config.
        let error = validate_coverage(&policy, &catalog).unwrap_err();
        let UniverseError::Validation(message) = error else {
            panic!("expected a validation error");
        };
        assert!(message.contains("not total on its declared radius"));
        assert!(message.contains("role: narrative"));
    }

    /// (b), form 2 — a measured signal the kit cannot render (no dynamics
    /// binding) is a named hole, and only for the `measured` tri-state.
    #[test]
    fn coverage_hole_from_unrenderable_measured_signal() {
        let mut catalog = catalog();
        // Remove the channel that displays a measured `energy` signal.
        catalog.mapping["dynamics"]
            .as_object_mut()
            .unwrap()
            .remove("energy_to_emissive");

        let report = compute_coverage(&policy(), &catalog).unwrap();
        assert!(!report.is_total());
        // Only energy × measured × 4 lod = 4 holes (weight/embedding unaffected;
        // absent/not_measured energy stay at identity/fog, no channel needed).
        assert_eq!(report.holes.len(), 4);
        assert!(report.holes.iter().all(|hole| hole.signal == "energy"
            && hole.signal_state == "measured"
            && hole.reason.contains("no dynamics binding")));

        assert!(matches!(
            validate_coverage(&policy(), &catalog),
            Err(UniverseError::Validation(message)) if message.contains("signal: energy") && message.contains("state: measured")
        ));
    }

    /// (a) The coverage observer composes with the structural rules: five
    /// structurally invalid catalogs are each rejected, one per rule, before
    /// totality is even measured.
    #[test]
    fn coverage_composes_with_five_structural_rejections() {
        // Rule 1: primitive kind out of the allowed set. (`torus` and the other
        // hard-edged primitives are now IN the palette; use one that is not.)
        let mut c1 = catalog();
        c1.mapping["forms"]["energy_orb"][0][0] = json!("hypercube");
        assert!(matches!(
            validate_coverage(&policy(), &c1),
            Err(UniverseError::Validation(m)) if m.contains("is not allowed")
        ));

        // Rule 2: primitive tuple not arity-8.
        let mut c2 = catalog();
        c2.mapping["forms"]["energy_orb"][0] = json!(["icosphere", "core", "core"]);
        assert!(matches!(
            validate_coverage(&policy(), &c2),
            Err(UniverseError::Validation(m)) if m.contains("tuple arity is not 8")
        ));

        // Rule 3: primitive_budget exceeds the renderer range.
        let mut c3 = catalog();
        c3.mapping["primitive_budget"] = json!(99);
        assert!(matches!(
            validate_coverage(&policy(), &c3),
            Err(UniverseError::Validation(m)) if m.contains("primitive_budget out of renderer range")
        ));

        // Rule 4: a LOD state points at an undefined form.
        let mut c4 = catalog();
        c4.mapping["lod_states"]["hot"] = json!("no_such_form");
        assert!(matches!(
            validate_coverage(&policy(), &c4),
            Err(UniverseError::Validation(m)) if m.contains("points at an undefined form")
        ));

        // Rule 5: a degenerate dynamics span (inMin >= inMax).
        let mut c5 = catalog();
        c5.mapping["dynamics"]["energy_to_emissive"] = json!([5, 5, 1.0, 2.2]);
        assert!(matches!(
            validate_coverage(&policy(), &c5),
            Err(UniverseError::Validation(m)) if m.contains("degenerate")
        ));
    }

    // -----------------------------------------------------------------------
    // Validation chrome — ConstructChrome → visual instructions.
    // -----------------------------------------------------------------------

    /// The frozen `validation_chrome` block, copied inline so the compile test
    /// is pinned to the honesty palette independent of the catalog fixture.
    fn frozen_chrome_block() -> Value {
        json!({
            "axis_palette": {
                "correctness": { "correct": "good", "chained": "warn", "logical_error": "bad", "incomplete": "neutral", "unknown": "fog" },
                "trust":       { "strong": "good", "adequate": "warn", "weak": "bad", "not_measured": "fog" },
                "health":      { "healthy": "good", "degraded": "warn", "stale": "warn", "measurement_failed": "bad", "unknown": "fog", "not_measured": "fog" },
                "epistemic":   { "observed": "good", "measured": "good", "known_absent": "neutral", "measurement_failed": "bad", "unknown": "fog", "not_measured": "fog" }
            },
            "fog_alpha": 0.42,
            "liveness_emissive": { "cold": 0.0, "pulsing": 1.0 },
            "receipt_orb": { "fresh_brightness": 1.0, "stale_brightness": 0.3 }
        })
    }

    /// A written-but-not-run Construct: correctness is `chained` (warn), and the
    /// three unmeasured axes fold to the `fog` role. The compiler paints no
    /// confidence it was not given — fogged body alpha, cold (zero) emissive.
    #[test]
    fn written_not_run_construct_compiles_to_honest_chrome() {
        let block = frozen_chrome_block();
        let chrome = ConstructChrome {
            construct: "self_verifying_loop".into(),
            name: "written but not run".into(),
            lifecycle: "written".into(),
            axes: ChromeAxes {
                correctness: "chained".into(),
                trust: "not_measured".into(),
                health: "not_measured".into(),
                epistemic: "not_measured".into(),
            },
            fog: true,
            liveness: "cold".into(),
            needs_count: 0,
            top_need: None,
            receipts: vec![],
        };
        let out = compile_chrome(&block, &chrome).unwrap();
        assert_eq!(
            out.ring_roles,
            vec![
                RingRole { axis: "correctness".into(), role: "warn".into() },
                RingRole { axis: "trust".into(), role: "fog".into() },
                RingRole { axis: "health".into(), role: "fog".into() },
                RingRole { axis: "epistemic".into(), role: "fog".into() },
            ]
        );
        assert_eq!(out.body_alpha, 0.42);
        assert_eq!(out.emissive, 0.0);
    }

    /// A correct + healthy + strongly-trusted + measured + pulsing Construct: all
    /// four rings resolve to `good`, the body is fully present, and it emits.
    #[test]
    fn correct_healthy_construct_compiles_to_confident_chrome() {
        let block = frozen_chrome_block();
        let chrome = ConstructChrome {
            construct: "self_verifying_loop".into(),
            name: "green loop".into(),
            lifecycle: "running".into(),
            axes: ChromeAxes {
                correctness: "correct".into(),
                trust: "strong".into(),
                health: "healthy".into(),
                epistemic: "measured".into(),
            },
            fog: false,
            liveness: "pulsing".into(),
            needs_count: 0,
            top_need: None,
            receipts: vec![ChromeReceipt {
                kind: "EffectReceipt".into(),
                id: "r1".into(),
                fresh: true,
            }],
        };
        let out = compile_chrome(&block, &chrome).unwrap();
        let roles: Vec<&str> = out.ring_roles.iter().map(|r| r.role.as_str()).collect();
        assert_eq!(roles, vec!["good", "good", "good", "good"]);
        assert_eq!(out.emissive, 1.0);
        assert_eq!(out.body_alpha, 1.0);
        assert_eq!(
            out.receipt_orbs,
            vec![ReceiptOrb { id: "r1".into(), brightness: 1.0 }]
        );
    }

    /// A `validation_chrome` block missing one enum value of an axis is a
    /// coverage hole and is rejected — the same totality discipline the coverage
    /// observer applies to a kit's radius.
    #[test]
    fn validation_chrome_with_a_coverage_hole_is_rejected() {
        let mut block = frozen_chrome_block();
        // Drop the `stale` value from the health axis — a coverage hole.
        block["axis_palette"]["health"]
            .as_object_mut()
            .unwrap()
            .remove("stale");
        assert!(matches!(
            validate_validation_chrome(&block),
            Err(UniverseError::Validation(m)) if m.contains("coverage hole") && m.contains("stale")
        ));
        // The compiler validates the block first, so it refuses it too.
        let chrome = ConstructChrome {
            construct: "x".into(),
            name: "x".into(),
            lifecycle: "written".into(),
            axes: ChromeAxes {
                correctness: "correct".into(),
                trust: "strong".into(),
                health: "healthy".into(),
                epistemic: "measured".into(),
            },
            fog: false,
            liveness: "cold".into(),
            needs_count: 0,
            top_need: None,
            receipts: vec![],
        };
        assert!(compile_chrome(&block, &chrome).is_err());
    }
}
