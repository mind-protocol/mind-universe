//! G2 phase 5 — bounded, inert physics-profile import pilot.
//!
//! Phase 3 (`ontology_pilot`) adapts source *vocabulary* to the ontology; this
//! module adapts source *physical profiles* (per-predicate force descriptors:
//! family, polarity, hierarchy, permanence, mode) to the ontology's
//! `physical_profile` schema — and imports them **inert**. "Inert" is the
//! load-bearing guarantee: an imported physics profile is NEVER bound to the
//! live `universe-physics` simulation. Nothing is materialized, no residency is
//! made `hot`, no `PhysicsCommand` is produced. Calibration and activation are
//! deliberately out of scope and reported `not_measured`.
//!
//! The engine — not the manifest — decides each profile's outcome, and stamps
//! the inertness flags unconditionally. Each source profile reaches exactly one
//! of four outcomes, never a "nearest" physics:
//! - `adapted_inert` — a complete, in-range profile mapped to the schema;
//! - `compatibility` — recognised but missing an optional descriptor (`mode`);
//! - `unresolved` — a required field is absent, preserved as a Problem;
//! - `quarantined` — a present field is out of range, refused.
//!
//! Inertness is enforced three independent ways (mirrors `code_pilot`):
//! 1. every imported profile's content is stamped `physical_mapping_activated`,
//!    `residency_activated`, `materialized_in_simulation` = false (the manifest
//!    has no field that can assert activation);
//! 2. every imported profile is `QUARANTINED_FROM` a single activation barrier;
//! 3. an engine guard [`attempt_physics_activation`] refuses to activate any
//!    `imported_physics_profile`, keyed on the Node kind, not a spoofable flag.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};
use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseError, UniverseId};
use universe_store::{
    EntityRecord, GraphSeed, RelationRecord, SeedEntity, SeedRelation, UniverseSnapshot,
    UniverseStore,
};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

const DECISIONS: [&str; 4] = [
    "adapted_inert",
    "compatibility",
    "unresolved",
    "quarantined",
];

const SYM_SOURCE: &str = "postgres_import_source";
const SYM_PROFILE: &str = "imported_physics_profile";
const SYM_CHANGESET: &str = "physics_import_changeset";
const SYM_BARRIER: &str = "physics_activation_barrier";
const SYM_RECEIPT: &str = "import_receipt";
const SYM_GOVERNED_BY: &str = "GOVERNED_BY";
const SYM_PART_OF: &str = "PART_OF";
const SYM_QUARANTINED_FROM: &str = "QUARANTINED_FROM";
const SYM_HAS_RECEIPT: &str = "HAS_RECEIPT";

const MEMBERSHIP_RELATION_OFFSET: u128 = 0x100;
const QUARANTINE_RELATION_OFFSET: u128 = 0x200;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PilotSource {
    pub atom: EntityKey,
    pub authority_id: String,
    pub source_graph_scope: Vec<String>,
    pub observed_at: String,
    pub mapping_version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApprovedChangeSet {
    pub atom: EntityKey,
    pub change_id: String,
    pub authority: String,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceProvenance {
    pub source_id: String,
    pub source_revision: u64,
    pub import_batch: String,
    pub content_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourcePhysicsProfile {
    pub atom: EntityKey,
    pub source_predicate: String,
    pub source_graph_scope: Vec<String>,
    pub provenance: SourceProvenance,
    /// The raw source physics fields, in the ontology `physical_profile` shape.
    pub profile: Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhysicsPilotManifest {
    pub contract_version: u16,
    pub universe: UniverseId,
    pub source: PilotSource,
    pub changeset: ApprovedChangeSet,
    pub profiles: Vec<SourcePhysicsProfile>,
    pub receipt_atom: EntityKey,
    pub receipt_relation: RelationKey,
    pub relation_key_start: RelationKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhysicsPilotEvidence {
    pub change_id: String,
    pub universe: UniverseId,
    pub total_profiles: usize,
    pub adapted_inert: usize,
    pub compatibility: usize,
    pub unresolved: usize,
    pub quarantined: usize,
    /// Imported (adapted_inert + compatibility) profiles that are ChangeSet
    /// members and barrier-quarantined.
    pub changeset_members: usize,
    pub barrier_quarantined: usize,
    /// Load-bearing zeros: nothing was bound to the live simulation.
    pub physical_mapping_activated: usize,
    pub residency_activated: usize,
    pub materialized_in_simulation: usize,
    /// Every imported profile refused activation through the engine guard.
    pub activation_refusals: usize,
    pub provenance_complete: usize,
    pub pre_receipt_snapshot_hash: String,
    pub final_snapshot_hash: String,
    pub final_revision: Revision,
    pub final_tick: Tick,
    pub content_records_read_back: usize,
    pub receipt_atom: EntityKey,
}

pub fn load_manifest(path: impl AsRef<Path>) -> Result<PhysicsPilotManifest, UniverseError> {
    let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| UniverseError::CorruptContent(error.to_string()))
}

/// Ordered state-machine transitions a profile passes through, by outcome. The
/// downstream stages (calibration, simulation binding, activation) are never
/// reached here — they stay `not_measured`.
fn transitions(decision: &str) -> Vec<&'static str> {
    match decision {
        "adapted_inert" => vec!["physics_classified", "schema_adapted", "imported_inert"],
        "compatibility" => vec!["physics_classified", "compatibility_recorded"],
        "unresolved" => vec!["physics_classified", "unresolved"],
        "quarantined" => vec!["physics_classified", "quarantined"],
        _ => Vec::new(),
    }
}

/// True when a profile is imported into the ontology (a ChangeSet member and
/// barrier-quarantined): the complete and the recognised-but-partial ones.
fn is_imported(decision: &str) -> bool {
    decision == "adapted_inert" || decision == "compatibility"
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Adaptation {
    decision: String,
    adapted_profile: Value,
    missing_required: Vec<String>,
    out_of_range: Vec<String>,
}

fn in_unit(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

/// The engine's adaptation gate. It reads the raw source profile and derives the
/// outcome structurally — never trusting a manifest-declared decision, never
/// inventing a missing field. A field that is present but out of the ontology's
/// range quarantines the profile; a required field that is absent leaves it
/// unresolved; a valid profile missing only the optional `mode` is compatibility.
fn adapt_profile(raw: &Value) -> Adaptation {
    let mut missing_required = Vec::new();
    let mut out_of_range = Vec::new();

    let family = raw.get("family").and_then(Value::as_str);
    if family.map(str::trim).unwrap_or("").is_empty() {
        missing_required.push("family".to_owned());
    }

    // hierarchy and permanence: required scalars in [0, 1].
    for field in ["hierarchy", "permanence"] {
        match raw.get(field) {
            None => missing_required.push(field.to_owned()),
            Some(value) => match value.as_f64() {
                Some(number) if in_unit(number) => {}
                _ => out_of_range.push(field.to_owned()),
            },
        }
    }

    // polarity: required 2-vector, each component in [0, 1].
    match raw.get("polarity") {
        None => missing_required.push("polarity".to_owned()),
        Some(Value::Array(components)) if components.len() == 2 => {
            let valid = components
                .iter()
                .all(|component| component.as_f64().is_some_and(in_unit));
            if !valid {
                out_of_range.push("polarity".to_owned());
            }
        }
        Some(_) => out_of_range.push("polarity".to_owned()),
    }

    let mode_present = raw
        .get("mode")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(|mode| !mode.is_empty())
        .unwrap_or(false);

    let decision = if !out_of_range.is_empty() {
        "quarantined"
    } else if !missing_required.is_empty() {
        "unresolved"
    } else if !mode_present {
        "compatibility"
    } else {
        "adapted_inert"
    }
    .to_owned();

    // The adapted profile echoes only the validated fields, and is only carried
    // for imported outcomes; otherwise it stays null (nothing invented).
    let adapted_profile = if is_imported(&decision) {
        let mut adapted = json!({
            "family": family.unwrap_or_default(),
            "hierarchy": raw.get("hierarchy").cloned().unwrap_or(Value::Null),
            "permanence": raw.get("permanence").cloned().unwrap_or(Value::Null),
            "polarity": raw.get("polarity").cloned().unwrap_or(Value::Null),
            "calibration_status": "prototype_not_calibrated",
        });
        if let Some(collision) = raw.get("collisionGroup") {
            adapted["collisionGroup"] = collision.clone();
        }
        if mode_present {
            adapted["mode"] = raw.get("mode").cloned().unwrap_or(Value::Null);
        }
        adapted
    } else {
        Value::Null
    };

    Adaptation {
        decision,
        adapted_profile,
        missing_required,
        out_of_range,
    }
}

/// Engine guard: activating an imported physics profile is refused. "Activation"
/// would bind the profile to the live simulation (materialize a body, make a
/// residency `hot`). Keyed on the Node kind so a content flag cannot spoof it.
pub fn attempt_physics_activation(content: &Value) -> Result<(), UniverseError> {
    if content.get("kind").and_then(Value::as_str) == Some(SYM_PROFILE) {
        return Err(UniverseError::Validation(
            "imported physics profile is inert and cannot be activated into the simulation".into(),
        ));
    }
    Ok(())
}

pub fn validate_manifest(manifest: &PhysicsPilotManifest) -> Result<(), UniverseError> {
    if manifest.contract_version != 0 {
        return Err(UniverseError::UnsupportedVersion(manifest.contract_version));
    }
    if !manifest.changeset.status.starts_with("approved") {
        return Err(validation("physics import ChangeSet is not approved"));
    }
    if manifest.source.source_graph_scope.is_empty() || manifest.source.mapping_version.is_empty() {
        return Err(validation(
            "physics pilot source scope or mapping version is missing",
        ));
    }
    if manifest.profiles.is_empty() {
        return Err(validation("physics pilot declares no profile"));
    }
    let mut atoms = BTreeSet::new();
    let reserved = [
        manifest.source.atom,
        manifest.changeset.atom,
        manifest.receipt_atom,
        barrier_atom(manifest),
    ];
    for profile in &manifest.profiles {
        if !atoms.insert(profile.atom) {
            return Err(validation("physics profile identity is duplicated"));
        }
        if reserved.contains(&profile.atom) {
            return Err(validation("physics profile reuses a reserved atom"));
        }
        if profile.provenance.source_id.trim().is_empty()
            || profile.provenance.import_batch.trim().is_empty()
            || profile.provenance.content_sha256.trim().is_empty()
            || profile.provenance.source_revision == 0
        {
            return Err(validation(format!(
                "physics profile {} has incomplete provenance",
                profile.source_predicate
            )));
        }
        if profile.source_graph_scope.is_empty() {
            return Err(validation(format!(
                "physics profile {} declares no source graph scope",
                profile.source_predicate
            )));
        }
    }
    Ok(())
}

/// The activation barrier lives at a fixed offset from the source atom so it
/// never collides with a profile atom.
fn barrier_atom(manifest: &PhysicsPilotManifest) -> EntityKey {
    EntityKey(manifest.source.atom.0 + 1)
}

pub fn materialize_seed(manifest: &PhysicsPilotManifest) -> Result<GraphSeed, UniverseError> {
    validate_manifest(manifest)?;
    let symbols = vec![
        SYM_SOURCE.to_owned(),
        SYM_PROFILE.to_owned(),
        SYM_CHANGESET.to_owned(),
        SYM_BARRIER.to_owned(),
        SYM_RECEIPT.to_owned(),
        SYM_GOVERNED_BY.to_owned(),
        SYM_PART_OF.to_owned(),
        SYM_QUARANTINED_FROM.to_owned(),
        SYM_HAS_RECEIPT.to_owned(),
    ];

    let mut entities = vec![
        entity(
            manifest.source.atom,
            SYM_SOURCE,
            json!({
                "kind": "postgres_import_source",
                "authority_id": manifest.source.authority_id,
                "source_graph_scope": manifest.source.source_graph_scope,
                "observed_at": manifest.source.observed_at,
                "mapping_version": manifest.source.mapping_version,
                "read_only": true,
                "credentials_stored": false,
            }),
        ),
        entity(
            barrier_atom(manifest),
            SYM_BARRIER,
            json!({
                "kind": "physics_activation_barrier",
                "guarantee": "Imported physics profiles are quarantined from the live simulation until a separate approved calibration+activation ChangeSet.",
            }),
        ),
        entity(
            manifest.changeset.atom,
            SYM_CHANGESET,
            json!({
                "kind": "physics_import_changeset",
                "change_id": manifest.changeset.change_id,
                "authority": manifest.changeset.authority,
                "status": manifest.changeset.status,
                "activation": "graph_scoped_inert_physics_import",
            }),
        ),
    ];

    for profile in &manifest.profiles {
        let adaptation = adapt_profile(&profile.profile);
        entities.push(entity(
            profile.atom,
            SYM_PROFILE,
            json!({
                "kind": "imported_physics_profile",
                "source_predicate": profile.source_predicate,
                "source_graph_scope": profile.source_graph_scope,
                "provenance": profile.provenance,
                "raw_profile": profile.profile,
                "adapted_profile": adaptation.adapted_profile,
                "decision": adaptation.decision,
                "missing_required": adaptation.missing_required,
                "out_of_range": adaptation.out_of_range,
                "transitions": transitions(&adaptation.decision),
                // Load-bearing inertness — stamped by the engine, unconditionally.
                "physical_mapping_activated": false,
                "residency_activated": false,
                "materialized_in_simulation": false,
                "executable": false,
                "calibration_status": "prototype_not_calibrated",
            }),
        ));
    }

    let mut next = manifest.relation_key_start.0;
    let mut relations = vec![relation(
        &mut next,
        manifest.changeset.atom,
        manifest.source.atom,
        SYM_GOVERNED_BY,
    )];
    for profile in &manifest.profiles {
        relations.push(relation(
            &mut next,
            profile.atom,
            manifest.source.atom,
            SYM_GOVERNED_BY,
        ));
    }

    Ok(GraphSeed {
        universe: manifest.universe,
        symbols,
        entities,
        relations,
    })
}

pub fn run_physics_pilot(
    manifest: &PhysicsPilotManifest,
    output: impl AsRef<Path>,
) -> Result<PhysicsPilotEvidence, UniverseError> {
    let store_root = output.as_ref();
    let store = UniverseStore::open(store_root)?;
    let installed = if store_root.join("snapshot.json").exists() {
        store.replay(store.load_snapshot()?)?
    } else {
        store.install_seed(&materialize_seed(manifest)?)?
    };
    let pre_receipt_snapshot_hash = installed.canonical_hash()?;

    let independent_store = UniverseStore::open(store_root)?;
    let mut independent = independent_store.replay(independent_store.load_snapshot()?)?;

    let counts = OutcomeCounts::observe(manifest);
    let imported: Vec<&SourcePhysicsProfile> = manifest
        .profiles
        .iter()
        .filter(|profile| is_imported(&adapt_profile(&profile.profile).decision))
        .collect();

    let receipt_content = json!({
        "kind": "adaptation_receipt",
        "phase": "g2_phase5_inert_physics_import",
        "change_id": manifest.changeset.change_id,
        "status": "measured_inert_physics_import",
        "information_status": "measured",
        "physics_imported_inert": true,
        "physical_mapping_activated": false,
        "residency_activated": false,
        "materialized_in_simulation": false,
        "calibrated": false,
        "outcomes": {
            "adapted_inert": counts.adapted_inert,
            "compatibility": counts.compatibility,
            "unresolved": counts.unresolved,
            "quarantined": counts.quarantined,
        },
        "not_measured": ["calibration", "simulation_binding", "residency_activation"],
        "profiles": manifest.profiles.iter().map(|profile| {
            let decision = adapt_profile(&profile.profile).decision;
            json!({
                "profile": profile.atom,
                "source_predicate": profile.source_predicate,
                "decision": decision,
                "transitions": transitions(&decision),
            })
        }).collect::<Vec<_>>(),
    });

    let import_key = format!("{}:import", manifest.changeset.change_id);
    if !independent.event_keys.contains(&import_key) {
        let part_of = symbol(&independent, SYM_PART_OF)?;
        let quarantined_from = symbol(&independent, SYM_QUARANTINED_FROM)?;
        let has_receipt = symbol(&independent, SYM_HAS_RECEIPT)?;
        let receipt_symbol = symbol(&independent, SYM_RECEIPT)?;
        let receipt_ref = independent_store.append_content(&receipt_content)?;
        let barrier = barrier_atom(manifest);

        let mut commands = Vec::new();
        let mut membership_key = manifest.relation_key_start.0 + MEMBERSHIP_RELATION_OFFSET;
        let mut quarantine_key = manifest.relation_key_start.0 + QUARANTINE_RELATION_OFFSET;
        for profile in &imported {
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(membership_key),
                    generation: 0,
                    source: profile.atom,
                    target: manifest.changeset.atom,
                    predicate: part_of,
                    content: Some(independent_store.append_content(&json!({
                        "kind": "import_relation",
                        "role": "changeset_membership",
                        "justification": "Approved, scoped, inert physics profile imported by the physics import ChangeSet."
                    }))?),
                },
            });
            membership_key += 1;
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(quarantine_key),
                    generation: 0,
                    source: profile.atom,
                    target: barrier,
                    predicate: quarantined_from,
                    content: Some(independent_store.append_content(&json!({
                        "kind": "import_relation",
                        "role": "activation_quarantine",
                        "justification": "Imported physics profile is quarantined from the live simulation until separate approved calibration+activation."
                    }))?),
                },
            });
            quarantine_key += 1;
        }
        commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: manifest.receipt_atom,
                generation: 0,
                symbol: receipt_symbol,
                content: Some(receipt_ref),
            },
        });
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: manifest.receipt_relation,
                generation: 0,
                source: manifest.changeset.atom,
                target: manifest.receipt_atom,
                predicate: has_receipt,
                content: Some(independent_store.append_content(&json!({
                    "kind": "import_relation",
                    "justification": "Independent readback produced this measured inert physics import receipt."
                }))?),
            },
        });

        let transaction = UniverseTransaction::prepare(
            &independent,
            UniverseWriteSet {
                base_revision: independent.revision,
                idempotency_key: import_key,
                causal_ancestry: vec![manifest.changeset.change_id.clone()],
                commands,
            },
        )?;
        let tick = Tick(independent.tick.0 + 1);
        transaction.commit(&independent_store, &mut independent, tick)?;
    }

    // Final independent replay and verification.
    let final_store = UniverseStore::open(store_root)?;
    let final_snapshot = final_store.replay(final_store.load_snapshot()?)?;
    let final_part_of = symbol(&final_snapshot, SYM_PART_OF)?;
    let final_quarantined = symbol(&final_snapshot, SYM_QUARANTINED_FROM)?;
    let barrier = barrier_atom(manifest);

    let mut changeset_members = 0usize;
    let mut barrier_quarantined = 0usize;
    let mut physical_mapping_activated = 0usize;
    let mut residency_activated = 0usize;
    let mut materialized_in_simulation = 0usize;
    let mut activation_refusals = 0usize;
    let mut provenance_complete = 0usize;

    for profile in &manifest.profiles {
        let expected_import = is_imported(&adapt_profile(&profile.profile).decision);

        let member = final_snapshot.relations.iter().any(|relation| {
            relation.source == profile.atom
                && relation.target == manifest.changeset.atom
                && relation.predicate == final_part_of
        });
        let quarantined = final_snapshot.relations.iter().any(|relation| {
            relation.source == profile.atom
                && relation.target == barrier
                && relation.predicate == final_quarantined
        });
        if member {
            changeset_members += 1;
        }
        if quarantined {
            barrier_quarantined += 1;
        }
        if member != expected_import || quarantined != expected_import {
            return Err(UniverseError::CorruptContent(format!(
                "physics profile {} membership/quarantine disagrees with its decision after replay",
                profile.source_predicate
            )));
        }

        // Read the persisted profile back and measure inertness from the store.
        let content = final_snapshot
            .entities
            .iter()
            .find(|entity| entity.key == profile.atom)
            .and_then(|entity| entity.content.as_ref())
            .ok_or_else(|| validation("physics profile Atom missing during readback"))?;
        let stored = final_store.read_content(content)?;
        if stored.get("physical_mapping_activated") == Some(&Value::Bool(true)) {
            physical_mapping_activated += 1;
        }
        if stored.get("residency_activated") == Some(&Value::Bool(true)) {
            residency_activated += 1;
        }
        if stored.get("materialized_in_simulation") == Some(&Value::Bool(true)) {
            materialized_in_simulation += 1;
        }
        // The engine guard refuses to activate the profile read back from the store.
        if attempt_physics_activation(&stored).is_err() {
            activation_refusals += 1;
        }
        let provenance = &stored["provenance"];
        if provenance
            .get("source_id")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
            && provenance
                .get("import_batch")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
            && provenance
                .get("content_sha256")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
            && provenance
                .get("source_revision")
                .and_then(Value::as_u64)
                .is_some_and(|r| r > 0)
        {
            provenance_complete += 1;
        }
    }

    if changeset_members != imported.len() || barrier_quarantined != imported.len() {
        return Err(UniverseError::CorruptContent(
            "imported physics profile count differs from ChangeSet membership/quarantine".into(),
        ));
    }
    // Load-bearing inertness: nothing bound to the simulation, and every imported
    // profile refuses activation.
    if physical_mapping_activated != 0
        || residency_activated != 0
        || materialized_in_simulation != 0
    {
        return Err(UniverseError::CorruptContent(
            "an imported physics profile was bound to the live simulation".into(),
        ));
    }
    if activation_refusals != manifest.profiles.len() {
        return Err(UniverseError::CorruptContent(
            "an imported physics profile did not refuse activation".into(),
        ));
    }
    if provenance_complete != manifest.profiles.len() {
        return Err(UniverseError::CorruptContent(
            "an imported physics profile lost its provenance after replay".into(),
        ));
    }

    let receipt_entity = final_snapshot
        .entities
        .iter()
        .find(|entity| entity.key == manifest.receipt_atom)
        .and_then(|entity| entity.content.as_ref())
        .ok_or_else(|| validation("physics import receipt is missing after replay"))?;
    if final_store.read_content(receipt_entity)? != receipt_content {
        return Err(UniverseError::CorruptContent(
            "physics import receipt differs after replay".into(),
        ));
    }

    let final_counts = OutcomeCounts::observe_store(&final_store, &final_snapshot, manifest)?;
    if final_counts != counts {
        return Err(UniverseError::CorruptContent(
            "physics pilot replay changed measured outcomes".into(),
        ));
    }

    let content_records_read_back = read_all_content(&final_store, &final_snapshot)?;
    Ok(PhysicsPilotEvidence {
        change_id: manifest.changeset.change_id.clone(),
        universe: final_snapshot.universe,
        total_profiles: manifest.profiles.len(),
        adapted_inert: counts.adapted_inert,
        compatibility: counts.compatibility,
        unresolved: counts.unresolved,
        quarantined: counts.quarantined,
        changeset_members,
        barrier_quarantined,
        physical_mapping_activated,
        residency_activated,
        materialized_in_simulation,
        activation_refusals,
        provenance_complete,
        pre_receipt_snapshot_hash,
        final_snapshot_hash: final_snapshot.canonical_hash()?,
        final_revision: final_snapshot.revision,
        final_tick: final_snapshot.tick,
        content_records_read_back,
        receipt_atom: manifest.receipt_atom,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OutcomeCounts {
    adapted_inert: usize,
    compatibility: usize,
    unresolved: usize,
    quarantined: usize,
}

impl OutcomeCounts {
    fn empty() -> Self {
        Self {
            adapted_inert: 0,
            compatibility: 0,
            unresolved: 0,
            quarantined: 0,
        }
    }

    fn observe(manifest: &PhysicsPilotManifest) -> Self {
        let mut counts = Self::empty();
        for profile in &manifest.profiles {
            counts.add(&adapt_profile(&profile.profile).decision);
        }
        counts
    }

    /// Re-derives outcomes by reading each profile Atom's persisted decision, so
    /// the counts are measured from the store, not trusted.
    fn observe_store(
        store: &UniverseStore,
        snapshot: &UniverseSnapshot,
        manifest: &PhysicsPilotManifest,
    ) -> Result<Self, UniverseError> {
        let mut counts = Self::empty();
        for profile in &manifest.profiles {
            let content = snapshot
                .entities
                .iter()
                .find(|entity| entity.key == profile.atom)
                .and_then(|entity| entity.content.as_ref())
                .ok_or_else(|| validation("profile Atom missing during readback"))?;
            let decision = store
                .read_content(content)?
                .get("decision")
                .and_then(Value::as_str)
                .ok_or_else(|| validation("profile Atom has no decision"))?
                .to_owned();
            counts.add(&decision);
        }
        Ok(counts)
    }

    fn add(&mut self, decision: &str) {
        match decision {
            "adapted_inert" => self.adapted_inert += 1,
            "compatibility" => self.compatibility += 1,
            "unresolved" => self.unresolved += 1,
            "quarantined" => self.quarantined += 1,
            _ => {}
        }
    }
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

fn entity(key: EntityKey, symbol: &str, content: Value) -> SeedEntity {
    SeedEntity {
        key,
        generation: 0,
        symbol: symbol.to_owned(),
        content,
    }
}

fn relation(
    next: &mut u128,
    source: EntityKey,
    target: EntityKey,
    predicate: &str,
) -> SeedRelation {
    let relation = SeedRelation {
        key: RelationKey(*next),
        generation: 0,
        source,
        target,
        predicate: predicate.to_owned(),
        content: None,
    };
    *next += 1;
    relation
}

fn symbol(snapshot: &UniverseSnapshot, name: &str) -> Result<u32, UniverseError> {
    snapshot
        .symbol_id(name)
        .ok_or_else(|| validation(format!("symbol {name} is absent")))
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PhysicsPilotManifest {
        load_manifest(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/import/postgres-physics-pilot.json"),
        )
        .unwrap()
    }

    #[test]
    fn adaptation_gate_classifies_each_form() {
        assert_eq!(
            adapt_profile(&json!({"family":"enablement","hierarchy":0.2,"permanence":0.6,"polarity":[0.9,0.2],"mode":"composite"})).decision,
            "adapted_inert"
        );
        assert_eq!(
            adapt_profile(&json!({"family":"containment","hierarchy":0.4,"permanence":0.7,"polarity":[0.3,0.8]})).decision,
            "compatibility"
        );
        assert_eq!(
            adapt_profile(
                &json!({"hierarchy":0.4,"permanence":0.7,"polarity":[0.3,0.8],"mode":"composite"})
            )
            .decision,
            "unresolved"
        );
        assert_eq!(
            adapt_profile(&json!({"family":"flux","hierarchy":0.4,"permanence":0.7,"polarity":[1.7,0.8],"mode":"composite"})).decision,
            "quarantined"
        );
    }

    #[test]
    fn profiles_import_inert_with_provenance_and_read_back() {
        let temp = tempfile::tempdir().unwrap();
        let evidence = run_physics_pilot(&manifest(), temp.path()).unwrap();
        assert_eq!(evidence.total_profiles, 4);
        assert_eq!(evidence.adapted_inert, 1);
        assert_eq!(evidence.compatibility, 1);
        assert_eq!(evidence.unresolved, 1);
        assert_eq!(evidence.quarantined, 1);
        // Only the two imported profiles are ChangeSet members + barrier-quarantined.
        assert_eq!(evidence.changeset_members, 2);
        assert_eq!(evidence.barrier_quarantined, 2);
        // Load-bearing inertness.
        assert_eq!(evidence.physical_mapping_activated, 0);
        assert_eq!(evidence.residency_activated, 0);
        assert_eq!(evidence.materialized_in_simulation, 0);
        assert_eq!(evidence.activation_refusals, 4);
        assert_eq!(evidence.provenance_complete, 4);
    }

    #[test]
    fn rerun_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let first = run_physics_pilot(&manifest(), temp.path()).unwrap();
        let second = run_physics_pilot(&manifest(), temp.path()).unwrap();
        assert_eq!(first.final_snapshot_hash, second.final_snapshot_hash);
        assert_eq!(first.final_revision, second.final_revision);
        assert_eq!(second.changeset_members, 2);
    }

    #[test]
    fn activation_of_an_imported_profile_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = manifest();
        run_physics_pilot(&manifest, temp.path()).unwrap();

        // Reopen the store independently, read a profile back, and prove the
        // engine guard refuses to activate it into the simulation.
        let store = UniverseStore::open(temp.path()).unwrap();
        let snapshot = store.replay(store.load_snapshot().unwrap()).unwrap();
        let profile = &manifest.profiles[0];
        let content = snapshot
            .entities
            .iter()
            .find(|entity| entity.key == profile.atom)
            .and_then(|entity| entity.content.as_ref())
            .unwrap();
        let stored = store.read_content(content).unwrap();
        assert!(attempt_physics_activation(&stored).is_err());
    }

    #[test]
    fn activation_guard_refuses_even_if_content_claims_active() {
        // Inertness is keyed on the Node kind, not a spoofable flag.
        let spoofed = json!({
            "kind": "imported_physics_profile",
            "physical_mapping_activated": true,
            "materialized_in_simulation": true,
        });
        assert!(attempt_physics_activation(&spoofed).is_err());
    }
}
