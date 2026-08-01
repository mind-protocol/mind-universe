//! G2 phase 3 — bounded ontology-adaptation pilot.
//!
//! The identity pilot imports PostgreSQL rows as inert Atoms. This module takes
//! the *observed source vocabulary* (node types, subtypes, relation types) and
//! adapts it to the universe ontology through one **approved, versioned,
//! source-graph-scoped ChangeSet**. Each vocabulary term reaches exactly one of
//! four distinct outcomes and never a "nearest meaning":
//!
//! - `activated` — a validated binding authorized by the approved ChangeSet;
//! - `compatibility` — recognised but bound only as compatibility evidence;
//! - `unresolved` — preserved as a Problem, never guessed (all code subtypes
//!   stay here until phase 4 migration);
//! - `quarantined` — endpoint/direction/family validation failed.
//!
//! Activation here means "activated for later execution" of the *mapping*: it
//! makes no PostgreSQL row executable, migrates no code, and activates no
//! physics. Spelling equality alone is insufficient — a binding is refused
//! unless it declares an exact source-graph scope and mapping/ontology revision.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};
use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseError, UniverseId};
use universe_store::{
    EntityRecord, GraphSeed, RelationRecord, SeedEntity, SeedRelation, UniverseSnapshot,
    UniverseStore,
};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

const UNIVERSAL_TYPES: [&str; 5] = ["actor", "moment", "narrative", "space", "thing"];
const BINDING_KINDS: [&str; 3] = ["universal_type", "semantic_type", "predicate"];
const DECISIONS: [&str; 4] = ["activated", "compatibility", "unresolved", "quarantined"];

const SYM_SOURCE: &str = "postgres_import_source";
const SYM_BINDING: &str = "ontology_binding";
const SYM_CHANGESET: &str = "ontology_activation_changeset";
const SYM_RECEIPT: &str = "import_receipt";
const SYM_GOVERNED_BY: &str = "GOVERNED_BY";
const SYM_PART_OF: &str = "PART_OF";
const SYM_HAS_RECEIPT: &str = "HAS_RECEIPT";

/// Offset from the seed relation range to the activation (membership) range so
/// the two never collide.
const ACTIVATION_RELATION_OFFSET: u128 = 0x100;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PilotSource {
    pub atom: EntityKey,
    pub authority_id: String,
    pub source_graph_scope: Vec<String>,
    pub observed_at: String,
    pub ontology_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApprovedChangeSet {
    pub atom: EntityKey,
    pub change_id: String,
    pub authority: String,
    pub status: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BindingValidation {
    #[serde(default)]
    pub universal_type: Option<String>,
    #[serde(default)]
    pub stored_node_type: Option<String>,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub source_endpoint_type: Option<String>,
    #[serde(default)]
    pub target_endpoint_type: Option<String>,
    #[serde(default)]
    pub cardinality: Option<String>,
    #[serde(default)]
    pub family: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VocabularyBinding {
    pub atom: EntityKey,
    pub source_label: String,
    pub binding_kind: String,
    pub source_graph_scope: Vec<String>,
    pub mapping_revision: u64,
    pub ontology_revision: u64,
    pub target: String,
    #[serde(default)]
    pub source_is_code: bool,
    #[serde(default)]
    pub validation: BindingValidation,
    pub decision: String,
    pub justification: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OntologyPilotManifest {
    pub contract_version: u16,
    pub universe: UniverseId,
    pub source: PilotSource,
    pub changeset: ApprovedChangeSet,
    pub bindings: Vec<VocabularyBinding>,
    pub receipt_atom: EntityKey,
    pub receipt_relation: RelationKey,
    pub relation_key_start: RelationKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OntologyPilotEvidence {
    pub change_id: String,
    pub universe: UniverseId,
    pub total_bindings: usize,
    pub activated: usize,
    pub compatibility: usize,
    pub unresolved: usize,
    pub quarantined: usize,
    /// Activated bindings are activated for *later* mapping execution only.
    pub activated_for_later_execution: usize,
    /// Must be zero: no code subtype and no source row is made executable.
    pub code_bindings_activated: usize,
    pub source_rows_executable: usize,
    pub changeset_members: usize,
    pub pre_receipt_snapshot_hash: String,
    pub final_snapshot_hash: String,
    pub final_revision: Revision,
    pub final_tick: Tick,
    pub content_records_read_back: usize,
    pub receipt_atom: EntityKey,
}

pub fn load_manifest(path: impl AsRef<Path>) -> Result<OntologyPilotManifest, UniverseError> {
    let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| UniverseError::CorruptContent(error.to_string()))
}

/// Ordered state-machine transitions a binding passes through, by outcome. The
/// transitions are recorded in the receipt as measured evidence.
fn transitions(decision: &str) -> Vec<&'static str> {
    match decision {
        "activated" => vec![
            "ontology_classified",
            "validated",
            "approved_changeset",
            "activated_for_later_execution",
        ],
        "compatibility" => vec!["ontology_classified", "validated", "compatibility_recorded"],
        "unresolved" => vec!["ontology_classified", "unresolved"],
        "quarantined" => vec!["ontology_classified", "quarantined"],
        _ => Vec::new(),
    }
}

pub fn validate_manifest(manifest: &OntologyPilotManifest) -> Result<(), UniverseError> {
    if manifest.contract_version != 0 {
        return Err(UniverseError::UnsupportedVersion(manifest.contract_version));
    }
    if !manifest.changeset.status.starts_with("approved") {
        return Err(validation("ontology activation ChangeSet is not approved"));
    }
    if manifest.source.source_graph_scope.is_empty() || manifest.source.ontology_revision == 0 {
        return Err(validation(
            "ontology pilot source scope or revision is missing",
        ));
    }
    if manifest.bindings.is_empty() {
        return Err(validation("ontology pilot declares no binding"));
    }

    let mut atoms = BTreeSet::new();
    let source_scope: BTreeSet<_> = manifest.source.source_graph_scope.iter().collect();
    let mut activated = 0usize;
    for binding in &manifest.bindings {
        if !atoms.insert(binding.atom) {
            return Err(validation("ontology binding identity is duplicated"));
        }
        if !BINDING_KINDS.contains(&binding.binding_kind.as_str()) {
            return Err(validation(format!(
                "binding {} has unknown kind {}",
                binding.source_label, binding.binding_kind
            )));
        }
        if !DECISIONS.contains(&binding.decision.as_str()) {
            return Err(validation(format!(
                "binding {} has unknown decision {}",
                binding.source_label, binding.decision
            )));
        }
        if binding.justification.trim().is_empty() {
            return Err(validation(format!(
                "binding {} is not justified",
                binding.source_label
            )));
        }
        // Exact source-graph scope is mandatory for every binding: spelling
        // equality across graphs is never sufficient to bind.
        if binding.source_graph_scope.is_empty()
            || binding.mapping_revision == 0
            || binding.ontology_revision == 0
        {
            return Err(validation(format!(
                "binding {} lacks exact scope or mapping/ontology revision",
                binding.source_label
            )));
        }
        if !binding
            .source_graph_scope
            .iter()
            .all(|graph| source_scope.contains(graph))
        {
            return Err(validation(format!(
                "binding {} scopes a graph outside the declared source scope",
                binding.source_label
            )));
        }
        if binding.ontology_revision != manifest.source.ontology_revision {
            return Err(validation(format!(
                "binding {} targets a different ontology revision than the source",
                binding.source_label
            )));
        }

        if binding.decision == "activated" {
            // Code subtypes must never activate as canonical ontology; they wait
            // for phase 4 migration.
            if binding.source_is_code {
                return Err(validation(format!(
                    "code binding {} cannot be activated as ontology; it must stay unresolved",
                    binding.source_label
                )));
            }
            validate_activated(binding)?;
            activated += 1;
        }
    }
    if activated == 0 {
        return Err(validation(
            "approved ChangeSet activates no binding; nothing to authorize",
        ));
    }
    Ok(())
}

fn validate_activated(binding: &VocabularyBinding) -> Result<(), UniverseError> {
    match binding.binding_kind.as_str() {
        "universal_type" => {
            if !UNIVERSAL_TYPES.contains(&binding.target.as_str())
                || binding.validation.universal_type.as_deref() != Some(binding.target.as_str())
            {
                return Err(validation(format!(
                    "activated universal-type binding {} is not a validated universal type",
                    binding.source_label
                )));
            }
        }
        "semantic_type" => {
            if binding.validation.stored_node_type.is_none() {
                return Err(validation(format!(
                    "activated semantic-type binding {} has no stored node type",
                    binding.source_label
                )));
            }
        }
        "predicate" => {
            let v = &binding.validation;
            if v.direction.is_none()
                || v.source_endpoint_type.is_none()
                || v.target_endpoint_type.is_none()
                || v.cardinality.is_none()
                || v.family.is_none()
            {
                return Err(validation(format!(
                    "activated predicate binding {} is missing direction/endpoint/cardinality/family validation",
                    binding.source_label
                )));
            }
        }
        _ => unreachable!("binding kind validated earlier"),
    }
    Ok(())
}

pub fn materialize_seed(manifest: &OntologyPilotManifest) -> Result<GraphSeed, UniverseError> {
    validate_manifest(manifest)?;
    let symbols = vec![
        SYM_SOURCE.to_owned(),
        SYM_BINDING.to_owned(),
        SYM_CHANGESET.to_owned(),
        SYM_RECEIPT.to_owned(),
        SYM_GOVERNED_BY.to_owned(),
        SYM_PART_OF.to_owned(),
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
                "ontology_revision": manifest.source.ontology_revision,
                "read_only": true,
                "credentials_stored": false,
            }),
        ),
        entity(
            manifest.changeset.atom,
            SYM_CHANGESET,
            json!({
                "kind": "ontology_activation_changeset",
                "change_id": manifest.changeset.change_id,
                "authority": manifest.changeset.authority,
                "status": manifest.changeset.status,
                "activation": "graph_scoped_ontology_adaptation",
            }),
        ),
    ];
    for binding in &manifest.bindings {
        entities.push(entity(
            binding.atom,
            SYM_BINDING,
            json!({
                "kind": "ontology_binding",
                "source_label": binding.source_label,
                "binding_kind": binding.binding_kind,
                "source_graph_scope": binding.source_graph_scope,
                "mapping_revision": binding.mapping_revision,
                "ontology_revision": binding.ontology_revision,
                "target": binding.target,
                "source_is_code": binding.source_is_code,
                "validation": binding.validation,
                "decision": binding.decision,
                "justification": binding.justification,
                "transitions": transitions(&binding.decision),
                "executable": false,
                "activated_for_later_execution": binding.decision == "activated",
            }),
        ));
    }

    // Seed installs bindings classified and GOVERNED_BY the source. ChangeSet
    // membership (the activation act) is committed later by run_ontology_pilot.
    let mut next = manifest.relation_key_start.0;
    let mut relations = vec![relation(
        &mut next,
        manifest.changeset.atom,
        manifest.source.atom,
        SYM_GOVERNED_BY,
    )];
    for binding in &manifest.bindings {
        relations.push(relation(
            &mut next,
            binding.atom,
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

pub fn run_ontology_pilot(
    manifest: &OntologyPilotManifest,
    output: impl AsRef<Path>,
) -> Result<OntologyPilotEvidence, UniverseError> {
    let store_root = output.as_ref();
    let store = UniverseStore::open(store_root)?;
    // Idempotent bootstrap: install the seed only on the first run; a later run
    // resumes the already-installed store and reconstructs it by replay.
    let installed = if store_root.join("snapshot.json").exists() {
        store.replay(store.load_snapshot()?)?
    } else {
        store.install_seed(&materialize_seed(manifest)?)?
    };
    let pre_receipt_snapshot_hash = installed.canonical_hash()?;

    // Independent reopen for readback, exactly like the identity pilot.
    let independent_store = UniverseStore::open(store_root)?;
    let mut independent = independent_store.replay(independent_store.load_snapshot()?)?;

    let counts = OutcomeCounts::observe(manifest);
    let activated: Vec<&VocabularyBinding> = manifest
        .bindings
        .iter()
        .filter(|binding| binding.decision == "activated")
        .collect();

    // Deterministic receipt: it carries no live snapshot hash, so re-running
    // produces byte-identical content and the change stays idempotent.
    let receipt_content = json!({
        "kind": "adaptation_receipt",
        "change_id": manifest.changeset.change_id,
        "status": "measured_ontology_activation",
        "information_status": "measured",
        "ontology_activated": true,
        "physics_activated": false,
        "code_activated": false,
        "source_rows_executable": 0,
        "outcomes": {
            "activated": counts.activated,
            "compatibility": counts.compatibility,
            "unresolved": counts.unresolved,
            "quarantined": counts.quarantined,
        },
        "bindings": manifest.bindings.iter().map(|binding| json!({
            "binding": binding.atom,
            "source_label": binding.source_label,
            "decision": binding.decision,
            "transitions": transitions(&binding.decision),
        })).collect::<Vec<_>>(),
    });

    // The approved ChangeSet is applied exactly once; a resumed run finds its
    // key already present and re-issues no membership or receipt command.
    let activate_key = format!("{}:activate", manifest.changeset.change_id);
    if !independent.event_keys.contains(&activate_key) {
        let part_of = symbol(&independent, SYM_PART_OF)?;
        let has_receipt = symbol(&independent, SYM_HAS_RECEIPT)?;
        let receipt_symbol = symbol(&independent, SYM_RECEIPT)?;
        let receipt_ref = independent_store.append_content(&receipt_content)?;

        let mut commands = Vec::new();
        let mut membership_key = manifest.relation_key_start.0 + ACTIVATION_RELATION_OFFSET;
        for binding in &activated {
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(membership_key),
                    generation: 0,
                    source: binding.atom,
                    target: manifest.changeset.atom,
                    predicate: part_of,
                    content: Some(independent_store.append_content(&json!({
                        "kind": "import_relation",
                        "role": "changeset_membership",
                        "justification": "Approved, scoped, revision-pinned binding authorized by the ontology activation ChangeSet."
                    }))?),
                },
            });
            membership_key += 1;
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
                    "justification": "Independent readback produced this measured ontology activation receipt."
                }))?),
            },
        });

        let transaction = UniverseTransaction::prepare(
            &independent,
            UniverseWriteSet {
                base_revision: independent.revision,
                idempotency_key: activate_key,
                commands,
            },
        )?;
        let tick = Tick(independent.tick.0 + 1);
        transaction.commit(&independent_store, &mut independent, tick)?;
    }

    // Final independent replay and verification.
    let final_store = UniverseStore::open(output.as_ref())?;
    let final_snapshot = final_store.replay(final_store.load_snapshot()?)?;
    let final_part_of = symbol(&final_snapshot, SYM_PART_OF)?;

    // Every activated binding is a ChangeSet member; nothing else is.
    let mut changeset_members = 0;
    for binding in &manifest.bindings {
        let member = final_snapshot.relations.iter().any(|relation| {
            relation.source == binding.atom
                && relation.target == manifest.changeset.atom
                && relation.predicate == final_part_of
        });
        if member {
            changeset_members += 1;
        }
        if member != (binding.decision == "activated") {
            return Err(UniverseError::CorruptContent(format!(
                "binding {} membership disagrees with its decision after replay",
                binding.source_label
            )));
        }
    }
    if changeset_members != activated.len() {
        return Err(UniverseError::CorruptContent(
            "activated binding count differs from ChangeSet membership after replay".into(),
        ));
    }

    let receipt_entity = final_snapshot
        .entities
        .iter()
        .find(|entity| entity.key == manifest.receipt_atom)
        .and_then(|entity| entity.content.as_ref())
        .ok_or_else(|| validation("ontology activation receipt is missing after replay"))?;
    if final_store.read_content(receipt_entity)? != receipt_content {
        return Err(UniverseError::CorruptContent(
            "ontology activation receipt differs after replay".into(),
        ));
    }

    let final_counts = OutcomeCounts::observe_store(&final_store, &final_snapshot, manifest)?;
    if final_counts != counts {
        return Err(UniverseError::CorruptContent(
            "ontology pilot replay changed measured outcomes".into(),
        ));
    }
    // No activated binding is a code subtype — the load-bearing inertness check.
    let code_bindings_activated = manifest
        .bindings
        .iter()
        .filter(|binding| binding.decision == "activated" && binding.source_is_code)
        .count();
    if code_bindings_activated != 0 {
        return Err(UniverseError::CorruptContent(
            "a code subtype was activated as ontology".into(),
        ));
    }

    let content_records_read_back = read_all_content(&final_store, &final_snapshot)?;
    Ok(OntologyPilotEvidence {
        change_id: manifest.changeset.change_id.clone(),
        universe: final_snapshot.universe,
        total_bindings: manifest.bindings.len(),
        activated: counts.activated,
        compatibility: counts.compatibility,
        unresolved: counts.unresolved,
        quarantined: counts.quarantined,
        activated_for_later_execution: counts.activated,
        code_bindings_activated: 0,
        source_rows_executable: 0,
        changeset_members,
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
    activated: usize,
    compatibility: usize,
    unresolved: usize,
    quarantined: usize,
}

impl OutcomeCounts {
    fn observe(manifest: &OntologyPilotManifest) -> Self {
        let mut counts = Self {
            activated: 0,
            compatibility: 0,
            unresolved: 0,
            quarantined: 0,
        };
        for binding in &manifest.bindings {
            counts.add(&binding.decision);
        }
        counts
    }

    /// Re-derives outcome counts by reading each binding Atom's persisted
    /// decision, so the counts are measured from the store, not trusted.
    fn observe_store(
        store: &UniverseStore,
        snapshot: &UniverseSnapshot,
        manifest: &OntologyPilotManifest,
    ) -> Result<Self, UniverseError> {
        let mut counts = Self {
            activated: 0,
            compatibility: 0,
            unresolved: 0,
            quarantined: 0,
        };
        for binding in &manifest.bindings {
            let content = snapshot
                .entities
                .iter()
                .find(|entity| entity.key == binding.atom)
                .and_then(|entity| entity.content.as_ref())
                .ok_or_else(|| validation("binding Atom missing during readback"))?;
            let decision = store
                .read_content(content)?
                .get("decision")
                .and_then(Value::as_str)
                .ok_or_else(|| validation("binding Atom has no decision"))?
                .to_owned();
            counts.add(&decision);
        }
        Ok(counts)
    }

    fn add(&mut self, decision: &str) {
        match decision {
            "activated" => self.activated += 1,
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
    let result = SeedRelation {
        key: RelationKey(*next),
        generation: 0,
        source,
        target,
        predicate: predicate.to_owned(),
        content: None,
    };
    *next += 1;
    result
}

fn symbol(snapshot: &UniverseSnapshot, value: &str) -> Result<u32, UniverseError> {
    snapshot
        .symbol_id(value)
        .ok_or_else(|| validation(format!("ontology pilot symbol {value} is not interned")))
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> OntologyPilotManifest {
        serde_json::from_str(include_str!(
            "../../../fixtures/import/postgres-ontology-pilot.json"
        ))
        .unwrap()
    }

    #[test]
    fn approved_scoped_bindings_activate_and_read_back() {
        let manifest = manifest();
        let temp = tempfile::tempdir().unwrap();
        let evidence = run_ontology_pilot(&manifest, temp.path()).unwrap();

        assert_eq!(evidence.total_bindings, 7);
        assert_eq!(evidence.activated, 3);
        assert_eq!(evidence.compatibility, 1);
        assert_eq!(evidence.unresolved, 1);
        assert_eq!(evidence.quarantined, 2);
        assert_eq!(evidence.changeset_members, evidence.activated);
        assert_eq!(evidence.code_bindings_activated, 0);
        assert_eq!(evidence.source_rows_executable, 0);
        assert_eq!(
            evidence.activated
                + evidence.compatibility
                + evidence.unresolved
                + evidence.quarantined,
            evidence.total_bindings
        );
    }

    #[test]
    fn rerun_is_idempotent() {
        let manifest = manifest();
        let temp = tempfile::tempdir().unwrap();
        let first = run_ontology_pilot(&manifest, temp.path()).unwrap();
        let second = run_ontology_pilot(&manifest, temp.path()).unwrap();
        assert_eq!(first.final_revision, second.final_revision);
        assert_eq!(first.final_snapshot_hash, second.final_snapshot_hash);
    }

    #[test]
    fn code_subtype_cannot_be_activated() {
        let mut manifest = manifest();
        let code = manifest
            .bindings
            .iter_mut()
            .find(|binding| binding.source_is_code)
            .expect("fixture has a code subtype binding");
        code.decision = "activated".into();
        assert!(matches!(
            validate_manifest(&manifest),
            Err(UniverseError::Validation(message))
                if message.contains("cannot be activated as ontology")
        ));
    }

    #[test]
    fn activation_without_scope_is_rejected() {
        let mut manifest = manifest();
        manifest.bindings[0].source_graph_scope.clear();
        assert!(matches!(
            validate_manifest(&manifest),
            Err(UniverseError::Validation(message))
                if message.contains("lacks exact scope")
        ));
    }

    #[test]
    fn activated_predicate_without_endpoint_validation_is_rejected() {
        let mut manifest = manifest();
        let predicate = manifest
            .bindings
            .iter_mut()
            .find(|binding| binding.binding_kind == "predicate" && binding.decision == "activated")
            .expect("fixture has an activated predicate");
        predicate.validation.family = None;
        assert!(matches!(
            validate_manifest(&manifest),
            Err(UniverseError::Validation(message))
                if message.contains("missing direction/endpoint/cardinality/family")
        ));
    }
}
