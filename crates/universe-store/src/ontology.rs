//! Bounded reconstruction of the canonical ontology from a local graph cluster.

use crate::{canonical_hash, UniverseSnapshot, UniverseStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use universe_core::{EntityKey, RelationKey, Revision, UniverseError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OntologyLoadBudget {
    pub max_entities: usize,
    pub max_relations: usize,
}

impl Default for OntologyLoadBudget {
    fn default() -> Self {
        Self {
            max_entities: 512,
            max_relations: 2_048,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OntologyCounts {
    pub compatibility_predicates: usize,
    pub contracts: usize,
    pub doctrine_links: usize,
    pub epistemic_statuses: usize,
    pub gaps: usize,
    pub physical_profiles: usize,
    pub predicates: usize,
    pub relation_families: usize,
    pub registry_members: usize,
    pub semantic_types: usize,
    pub source_documents: usize,
    pub stored_node_types: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OntologyDefinitionKind {
    StoredNodeType,
    SemanticType,
    RelationFamily,
    Predicate,
    EpistemicStatus,
    CompatibilityPredicate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OntologyDefinition {
    pub key: EntityKey,
    pub kind: OntologyDefinitionKind,
    pub canonical_id: String,
    pub canonical: bool,
    pub compact_symbol: u32,
    pub executable: Option<Value>,
    pub endpoint_constraint: Option<Value>,
    pub constraint_status: Option<String>,
    pub stored_node_type: Option<String>,
    pub physical_profile: Option<EntityKey>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicalProfile {
    pub key: EntityKey,
    pub predicate: String,
    pub mapping_version: String,
    pub status: String,
    pub profile: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OntologyGap {
    pub key: EntityKey,
    pub canonical_id: String,
    pub subject: String,
    pub missing: BTreeSet<String>,
    pub status: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OntologyActivationState {
    BaseOnly,
    Active,
    ActiveWithBlockedBindings,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OntologyActivationDiagnostic {
    pub code: String,
    pub subject: String,
    pub missing: BTreeSet<String>,
    pub runtime_blocking: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActiveOntologyMember {
    pub key: EntityKey,
    pub change_set: EntityKey,
    pub membership_relation: RelationKey,
    pub kind: String,
    pub canonical_id: Option<String>,
    pub content_hash: String,
    pub membership_hash: String,
    pub content: Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActiveOntologyChangeSet {
    pub key: EntityKey,
    pub change_id: String,
    pub base_schema_version: String,
    pub target_schema_version: String,
    pub content_hash: String,
    pub members: Vec<EntityKey>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OntologyRegistry {
    pub manifest: EntityKey,
    pub base_manifest_hash: String,
    pub ontology_id: String,
    pub schema_version: String,
    pub mapping_version: String,
    pub status: String,
    pub source_hashes: BTreeMap<String, String>,
    pub counts: OntologyCounts,
    pub stored_node_types: BTreeMap<String, OntologyDefinition>,
    pub semantic_types: BTreeMap<String, OntologyDefinition>,
    pub relation_families: BTreeMap<String, OntologyDefinition>,
    pub predicates: BTreeMap<String, OntologyDefinition>,
    pub epistemic_statuses: BTreeMap<String, OntologyDefinition>,
    pub compatibility_predicates: BTreeMap<String, OntologyDefinition>,
    pub physical_profiles: BTreeMap<String, PhysicalProfile>,
    pub contracts: BTreeMap<String, EntityKey>,
    pub sources: BTreeMap<String, EntityKey>,
    pub gaps: BTreeMap<String, OntologyGap>,
    pub known_gaps: BTreeSet<String>,
    pub active_schema_version: String,
    pub activation_state: OntologyActivationState,
    pub active_change_sets: Vec<ActiveOntologyChangeSet>,
    pub overlay_members_by_key: BTreeMap<EntityKey, ActiveOntologyMember>,
    pub activation_diagnostics: Vec<OntologyActivationDiagnostic>,
    pub universe_revision: Revision,
    pub symbol_table_hash: String,
    pub authority_hash: String,
}

#[derive(Debug, Deserialize)]
struct StoredManifest {
    kind: String,
    ontology_id: String,
    schema_version: String,
    mapping_version: String,
    status: String,
    source_hashes: BTreeMap<String, String>,
    declared_counts: OntologyCounts,
    known_gaps: Vec<String>,
    compatibility_predicates: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct StoredDefinition {
    kind: String,
    definition_kind: OntologyDefinitionKind,
    canonical_id: String,
    canonical: bool,
    executable: Option<Value>,
    endpoint_constraint: Option<Value>,
    constraint_status: Option<String>,
    storage_mapping: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct StoredPhysicalProfile {
    kind: String,
    canonical_id: String,
    mapping_version: String,
    status: String,
    profile: Value,
}

#[derive(Debug, Deserialize)]
struct StoredContract {
    kind: String,
    canonical_id: String,
}

#[derive(Debug, Deserialize)]
struct StoredSource {
    kind: String,
    source_id: String,
    source_role: String,
    canonical_json_sha256: String,
    document: Value,
}

#[derive(Debug, Deserialize)]
struct StoredGap {
    kind: String,
    canonical_id: String,
    subject: String,
    missing: BTreeSet<String>,
    status: String,
}

#[derive(Debug, Deserialize)]
struct StoredChangeSet {
    kind: String,
    activation: String,
    base_manifest: EntityKey,
    base_schema_version: String,
    change_id: String,
    status: String,
    target_schema_version: String,
}

#[derive(Debug, Deserialize)]
struct StoredExtensionDefinition {
    kind: String,
    definition_kind: OntologyDefinitionKind,
    canonical_id: String,
    canonical: bool,
    #[serde(default)]
    endpoint_constraint: Option<Value>,
    #[serde(default)]
    stored_node_type: Option<String>,
    #[serde(default)]
    physical_profile: Option<EntityKey>,
}

#[derive(Debug, Deserialize)]
struct StoredRelation {
    kind: String,
    justification: String,
    role: String,
}

impl OntologyRegistry {
    /// Discovers the compact symbols required by explicitly active, approved
    /// ontology overlays. The returned names are graph data; callers publish
    /// them with `UniverseMutation::InternSymbols` before loading the registry.
    pub fn required_active_overlay_symbols(
        store: &UniverseStore,
        snapshot: &UniverseSnapshot,
        budget: OntologyLoadBudget,
    ) -> Result<Vec<String>, UniverseError> {
        enforce_budget(snapshot, budget)?;
        let mut active_change_sets = BTreeSet::new();
        for entity in &snapshot.entities {
            let Some(content) = entity.content.as_ref() else {
                continue;
            };
            let value = store.read_content(content)?;
            if value.get("kind").and_then(Value::as_str) != Some("ontology_changeset") {
                continue;
            }
            let change_set: StoredChangeSet =
                serde_json::from_value(value).map_err(json_validation)?;
            if change_set.activation == "active_graph_overlay"
                && change_set.status.starts_with("approved")
            {
                active_change_sets.insert(entity.key);
            }
        }

        let mut required = BTreeSet::new();
        for relation in snapshot
            .relations
            .iter()
            .filter(|relation| active_change_sets.contains(&relation.target))
        {
            let Some(relation_content) = relation.content.as_ref() else {
                continue;
            };
            let relation_value = store.read_content(relation_content)?;
            if relation_value.get("role").and_then(Value::as_str) != Some("changeset_membership") {
                continue;
            }
            let entity = snapshot
                .entities
                .iter()
                .find(|entity| entity.key == relation.source)
                .ok_or_else(|| {
                    validation(format!(
                        "active ChangeSet membership references missing entity {}",
                        relation.source
                    ))
                })?;
            let value = read_entity_content(store, entity.content.as_ref(), entity.key)?;
            if value.get("kind").and_then(Value::as_str) == Some("ontology_extension_definition") {
                let canonical_id = value
                    .get("canonical_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        validation(format!(
                            "ontology extension definition {} has no canonical_id",
                            entity.key
                        ))
                    })?;
                if snapshot.symbol_id(canonical_id).is_none() {
                    required.insert(canonical_id.to_owned());
                }
            }
        }
        Ok(required.into_iter().collect())
    }

    pub fn load(
        store: &UniverseStore,
        snapshot: &UniverseSnapshot,
        budget: OntologyLoadBudget,
    ) -> Result<Self, UniverseError> {
        enforce_budget(snapshot, budget)?;

        let manifest_symbol = symbol_id(snapshot, "protocol")?;
        let mut manifest_candidate = None;
        for entity in snapshot
            .entities
            .iter()
            .filter(|entity| entity.symbol == manifest_symbol)
        {
            let value = read_entity_content(store, entity.content.as_ref(), entity.key)?;
            if value.get("kind").and_then(Value::as_str) == Some("ontology_manifest") {
                if manifest_candidate.is_some() {
                    return Err(validation("multiple ontology manifests in local cluster"));
                }
                let manifest: StoredManifest =
                    serde_json::from_value(value).map_err(json_validation)?;
                manifest_candidate = Some((entity.key, manifest));
            }
        }
        let (manifest_key, manifest) = manifest_candidate
            .ok_or_else(|| validation("local cluster has no ontology manifest"))?;
        if manifest.kind != "ontology_manifest" {
            return Err(validation("manifest content kind changed during parsing"));
        }

        let part_of = symbol_id(snapshot, "PART_OF")?;
        let members: BTreeSet<_> = snapshot
            .relations
            .iter()
            .filter(|relation| relation.target == manifest_key && relation.predicate == part_of)
            .map(|relation| relation.source)
            .collect();
        if members.len() != manifest.declared_counts.registry_members {
            return Err(validation(format!(
                "manifest declares {} members but local neighborhood contains {}",
                manifest.declared_counts.registry_members,
                members.len()
            )));
        }

        let entity_by_key: BTreeMap<_, _> = snapshot
            .entities
            .iter()
            .map(|entity| (entity.key, entity))
            .collect();
        if members.iter().any(|key| !entity_by_key.contains_key(key)) {
            return Err(validation(
                "ontology membership points outside the local entity set",
            ));
        }

        let mut stored_node_types = BTreeMap::new();
        let mut semantic_types = BTreeMap::new();
        let mut relation_families = BTreeMap::new();
        let mut predicates = BTreeMap::new();
        let mut epistemic_statuses = BTreeMap::new();
        let mut compatibility_predicates = BTreeMap::new();
        let mut physical_profiles = BTreeMap::new();
        let mut contracts = BTreeMap::new();
        let mut sources = BTreeMap::new();
        let mut source_hashes = BTreeMap::new();
        let mut gaps = BTreeMap::new();

        for key in &members {
            let entity = entity_by_key[key];
            let value = read_entity_content(store, entity.content.as_ref(), *key)?;
            match value.get("kind").and_then(Value::as_str) {
                Some("ontology_definition") => {
                    let stored: StoredDefinition =
                        serde_json::from_value(value).map_err(json_validation)?;
                    if stored.kind != "ontology_definition" {
                        return Err(validation("definition content kind mismatch"));
                    }
                    let compact_symbol = symbol_id(snapshot, &stored.canonical_id)?;
                    let stored_node_type = stored
                        .storage_mapping
                        .as_ref()
                        .and_then(|mapping| mapping.get("l4"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let definition = OntologyDefinition {
                        key: *key,
                        kind: stored.definition_kind,
                        canonical_id: stored.canonical_id.clone(),
                        canonical: stored.canonical,
                        compact_symbol,
                        executable: stored.executable,
                        endpoint_constraint: stored.endpoint_constraint,
                        constraint_status: stored.constraint_status,
                        stored_node_type,
                        physical_profile: None,
                    };
                    let target = match stored.definition_kind {
                        OntologyDefinitionKind::StoredNodeType => &mut stored_node_types,
                        OntologyDefinitionKind::SemanticType => &mut semantic_types,
                        OntologyDefinitionKind::RelationFamily => &mut relation_families,
                        OntologyDefinitionKind::Predicate => &mut predicates,
                        OntologyDefinitionKind::EpistemicStatus => &mut epistemic_statuses,
                        OntologyDefinitionKind::CompatibilityPredicate => {
                            &mut compatibility_predicates
                        }
                    };
                    insert_unique(target, stored.canonical_id, definition, "definition")?;
                }
                Some("physical_profile") => {
                    let stored: StoredPhysicalProfile =
                        serde_json::from_value(value).map_err(json_validation)?;
                    if stored.kind != "physical_profile" {
                        return Err(validation("physical profile content kind mismatch"));
                    }
                    let profile = PhysicalProfile {
                        key: *key,
                        predicate: stored.canonical_id.clone(),
                        mapping_version: stored.mapping_version,
                        status: stored.status,
                        profile: stored.profile,
                    };
                    insert_unique(
                        &mut physical_profiles,
                        stored.canonical_id,
                        profile,
                        "physical profile",
                    )?;
                }
                Some("ontology_contract") => {
                    let stored: StoredContract =
                        serde_json::from_value(value).map_err(json_validation)?;
                    if stored.kind != "ontology_contract" {
                        return Err(validation("ontology contract content kind mismatch"));
                    }
                    insert_unique(
                        &mut contracts,
                        stored.canonical_id,
                        *key,
                        "ontology contract",
                    )?;
                }
                Some("ontology_source") => {
                    let stored: StoredSource =
                        serde_json::from_value(value).map_err(json_validation)?;
                    if stored.kind != "ontology_source" {
                        return Err(validation("ontology source content kind mismatch"));
                    }
                    let actual = canonical_hash(&stored.document)?;
                    if actual != stored.canonical_json_sha256 {
                        return Err(UniverseError::CorruptContent(format!(
                            "embedded ontology source {} hash mismatch",
                            stored.source_id
                        )));
                    }
                    insert_unique(
                        &mut sources,
                        stored.source_role.clone(),
                        *key,
                        "ontology source",
                    )?;
                    insert_unique(
                        &mut source_hashes,
                        stored.source_role,
                        actual,
                        "ontology source hash",
                    )?;
                }
                Some("ontology_gap") => {
                    let stored: StoredGap =
                        serde_json::from_value(value).map_err(json_validation)?;
                    if stored.kind != "ontology_gap" {
                        return Err(validation("ontology gap content kind mismatch"));
                    }
                    let gap = OntologyGap {
                        key: *key,
                        canonical_id: stored.canonical_id.clone(),
                        subject: stored.subject,
                        missing: stored.missing,
                        status: stored.status,
                    };
                    insert_unique(&mut gaps, stored.canonical_id, gap, "ontology gap")?;
                }
                Some(kind) => {
                    return Err(validation(format!(
                        "unknown ontology member content kind {kind}"
                    )));
                }
                None => return Err(validation("ontology member content has no kind")),
            }
        }

        if source_hashes != manifest.source_hashes {
            return Err(UniverseError::CorruptContent(
                "ontology manifest source hashes do not match embedded sources".into(),
            ));
        }

        let mut doctrine_links = 0;
        let mut scope = members.clone();
        scope.insert(manifest_key);
        for relation in snapshot
            .relations
            .iter()
            .filter(|relation| scope.contains(&relation.source) && scope.contains(&relation.target))
        {
            let content = relation.content.as_ref().ok_or_else(|| {
                validation(format!(
                    "ontology relation {} has no justification content",
                    relation.key
                ))
            })?;
            let stored: StoredRelation =
                serde_json::from_value(store.read_content(content)?).map_err(json_validation)?;
            if stored.kind != "ontology_relation" || stored.justification.trim().is_empty() {
                return Err(validation(format!(
                    "ontology relation {} is not explicitly justified",
                    relation.key
                )));
            }
            if stored.role == "doctrine_link" {
                doctrine_links += 1;
            }
        }

        for definition in semantic_types.values() {
            let stored_type = definition.stored_node_type.as_ref().ok_or_else(|| {
                validation(format!(
                    "semantic type {} has no stored node type mapping",
                    definition.canonical_id
                ))
            })?;
            if !stored_node_types.contains_key(stored_type) {
                return Err(validation(format!(
                    "semantic type {} maps to unknown stored node type {stored_type}",
                    definition.canonical_id
                )));
            }
        }

        for definition in predicates.values() {
            let family = definition
                .executable
                .as_ref()
                .and_then(|value| value.get("family"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    validation(format!(
                        "predicate {} has no executable family",
                        definition.canonical_id
                    ))
                })?;
            if !relation_families.contains_key(family) {
                return Err(validation(format!(
                    "predicate {} references unknown relation family {family}",
                    definition.canonical_id
                )));
            }
        }

        for profile in physical_profiles.values() {
            if profile.mapping_version != manifest.mapping_version {
                return Err(validation(format!(
                    "physical profile {} has mapping version {}, expected {}",
                    profile.predicate, profile.mapping_version, manifest.mapping_version
                )));
            }
            if profile.status != "prototype_not_calibrated" {
                return Err(validation(format!(
                    "physical profile {} silently claims calibrated authority",
                    profile.predicate
                )));
            }
            if !predicates.contains_key(&profile.predicate)
                && !compatibility_predicates.contains_key(&profile.predicate)
            {
                return Err(validation(format!(
                    "physical profile {} has no predicate definition",
                    profile.predicate
                )));
            }
        }

        for (predicate, profile) in &physical_profiles {
            let definition = predicates
                .get_mut(predicate)
                .or_else(|| compatibility_predicates.get_mut(predicate))
                .ok_or_else(|| {
                    validation(format!("physical profile {predicate} cannot be bound"))
                })?;
            definition.physical_profile = Some(profile.key);
        }

        let known_gaps: BTreeSet<_> = manifest.known_gaps.iter().cloned().collect();
        let gap_subjects: BTreeSet<_> = gaps.values().map(|gap| gap.subject.clone()).collect();
        let missing_constraints: BTreeSet<_> = predicates
            .values()
            .filter(|definition| {
                definition.constraint_status.as_deref() != Some("defined")
                    || definition.endpoint_constraint.is_none()
            })
            .map(|definition| definition.canonical_id.clone())
            .collect();
        let missing_profiles: BTreeSet<_> = predicates
            .values()
            .filter(|definition| definition.physical_profile.is_none())
            .map(|definition| definition.canonical_id.clone())
            .collect();
        if known_gaps != gap_subjects
            || known_gaps != missing_constraints
            || known_gaps != missing_profiles
        {
            return Err(validation(
                "declared ontology gaps do not match constraints, profiles, and gap Atoms",
            ));
        }
        for gap in gaps.values() {
            if gap.status != "unresolved"
                || gap.missing
                    != BTreeSet::from(["endpoint_constraint".into(), "physical_profile".into()])
            {
                return Err(validation(format!(
                    "gap {} does not preserve both missing definitions",
                    gap.canonical_id
                )));
            }
        }

        let declared_compatibility: BTreeSet<_> =
            manifest.compatibility_predicates.iter().cloned().collect();
        let actual_compatibility: BTreeSet<_> = compatibility_predicates.keys().cloned().collect();
        if declared_compatibility != actual_compatibility {
            return Err(validation(
                "manifest compatibility predicates differ from reconstructed definitions",
            ));
        }

        let actual_counts = OntologyCounts {
            compatibility_predicates: compatibility_predicates.len(),
            contracts: contracts.len(),
            doctrine_links,
            epistemic_statuses: epistemic_statuses.len(),
            gaps: gaps.len(),
            physical_profiles: physical_profiles.len(),
            predicates: predicates.len(),
            relation_families: relation_families.len(),
            registry_members: members.len(),
            semantic_types: semantic_types.len(),
            source_documents: sources.len(),
            stored_node_types: stored_node_types.len(),
        };
        if actual_counts != manifest.declared_counts {
            return Err(validation(format!(
                "ontology count mismatch: declared {:?}, actual {:?}",
                manifest.declared_counts, actual_counts
            )));
        }

        let (
            active_change_sets,
            overlay_members_by_key,
            active_schema_version,
            activation_state,
            activation_diagnostics,
        ) = load_active_overlays(
            store,
            snapshot,
            manifest_key,
            &manifest.schema_version,
            &entity_by_key,
            &mut stored_node_types,
            &mut semantic_types,
            &mut relation_families,
            &mut predicates,
            &mut epistemic_statuses,
            &mut compatibility_predicates,
            &mut contracts,
            &gaps,
        )?;
        let manifest_content_hash = entity_by_key[&manifest_key]
            .content
            .as_ref()
            .ok_or_else(|| validation("ontology manifest has no content reference"))?
            .sha256
            .clone();
        let symbol_table_hash = canonical_hash(&snapshot.symbols)?;
        let authority_hash = canonical_hash(&(
            manifest_key,
            &manifest_content_hash,
            &active_change_sets,
            &overlay_members_by_key,
            &symbol_table_hash,
        ))?;

        Ok(Self {
            manifest: manifest_key,
            base_manifest_hash: manifest_content_hash,
            ontology_id: manifest.ontology_id,
            schema_version: manifest.schema_version,
            mapping_version: manifest.mapping_version,
            status: manifest.status,
            source_hashes,
            counts: actual_counts,
            stored_node_types,
            semantic_types,
            relation_families,
            predicates,
            epistemic_statuses,
            compatibility_predicates,
            physical_profiles,
            contracts,
            sources,
            gaps,
            known_gaps,
            active_schema_version,
            activation_state,
            active_change_sets,
            overlay_members_by_key,
            activation_diagnostics,
            universe_revision: snapshot.revision,
            symbol_table_hash,
            authority_hash,
        })
    }

    pub fn semantic_type(&self, id: &str) -> Option<&OntologyDefinition> {
        self.semantic_types.get(id)
    }

    pub fn predicate(&self, id: &str) -> Option<&OntologyDefinition> {
        self.predicates.get(id)
    }

    pub fn definition_by_key(&self, key: EntityKey) -> Option<&OntologyDefinition> {
        [
            &self.stored_node_types,
            &self.semantic_types,
            &self.relation_families,
            &self.predicates,
            &self.epistemic_statuses,
            &self.compatibility_predicates,
        ]
        .into_iter()
        .find_map(|definitions| {
            definitions
                .values()
                .find(|definition| definition.key == key)
        })
    }

    pub fn active_member(&self, key: EntityKey) -> Option<&ActiveOntologyMember> {
        self.overlay_members_by_key.get(&key)
    }

    pub fn runtime_diagnostics_for(
        &self,
        definition: EntityKey,
    ) -> Vec<&OntologyActivationDiagnostic> {
        let Some(definition) = self.definition_by_key(definition) else {
            return Vec::new();
        };
        self.activation_diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.subject == definition.canonical_id)
            .collect()
    }
}

fn enforce_budget(
    snapshot: &UniverseSnapshot,
    budget: OntologyLoadBudget,
) -> Result<(), UniverseError> {
    if snapshot.entities.len() > budget.max_entities {
        return Err(UniverseError::BudgetExhausted(format!(
            "ontology cluster has {} entities, budget is {}",
            snapshot.entities.len(),
            budget.max_entities
        )));
    }
    if snapshot.relations.len() > budget.max_relations {
        return Err(UniverseError::BudgetExhausted(format!(
            "ontology cluster has {} relations, budget is {}",
            snapshot.relations.len(),
            budget.max_relations
        )));
    }
    Ok(())
}

type ActiveOverlayLoad = (
    Vec<ActiveOntologyChangeSet>,
    BTreeMap<EntityKey, ActiveOntologyMember>,
    String,
    OntologyActivationState,
    Vec<OntologyActivationDiagnostic>,
);

#[allow(clippy::too_many_arguments)]
fn load_active_overlays(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    manifest_key: EntityKey,
    base_schema_version: &str,
    entity_by_key: &BTreeMap<EntityKey, &crate::EntityRecord>,
    stored_node_types: &mut BTreeMap<String, OntologyDefinition>,
    semantic_types: &mut BTreeMap<String, OntologyDefinition>,
    relation_families: &mut BTreeMap<String, OntologyDefinition>,
    predicates: &mut BTreeMap<String, OntologyDefinition>,
    epistemic_statuses: &mut BTreeMap<String, OntologyDefinition>,
    compatibility_predicates: &mut BTreeMap<String, OntologyDefinition>,
    contracts: &mut BTreeMap<String, EntityKey>,
    gaps: &BTreeMap<String, OntologyGap>,
) -> Result<ActiveOverlayLoad, UniverseError> {
    let mut candidates = Vec::new();
    for entity in snapshot.entities.iter() {
        let Some(content) = entity.content.as_ref() else {
            continue;
        };
        let value = store.read_content(content)?;
        if value.get("kind").and_then(Value::as_str) != Some("ontology_changeset") {
            continue;
        }
        let change_set: StoredChangeSet = serde_json::from_value(value).map_err(json_validation)?;
        if change_set.kind != "ontology_changeset" {
            return Err(validation("ChangeSet content kind mismatch"));
        }
        if change_set.activation != "active_graph_overlay"
            || !change_set.status.starts_with("approved")
        {
            continue;
        }
        if change_set.base_manifest != manifest_key {
            return Err(validation(format!(
                "active ChangeSet {} targets a different ontology manifest",
                change_set.change_id
            )));
        }
        if change_set.base_schema_version != base_schema_version {
            return Err(validation(format!(
                "active ChangeSet {} expects base schema {}, loaded {}",
                change_set.change_id, change_set.base_schema_version, base_schema_version
            )));
        }
        candidates.push((entity.key, content.sha256.clone(), change_set));
    }
    candidates.sort_by_key(|(key, _, _)| *key);

    let mut active_change_sets = Vec::new();
    let mut overlay_members_by_key = BTreeMap::new();
    let mut active_schema_version = base_schema_version.to_owned();
    for (change_set_key, content_hash, change_set) in candidates {
        let mut target_links = 0;
        for relation in snapshot
            .relations
            .iter()
            .filter(|relation| relation.source == change_set_key && relation.target == manifest_key)
        {
            let content = relation.content.as_ref().ok_or_else(|| {
                validation(format!(
                    "ChangeSet target relation {} has no content",
                    relation.key
                ))
            })?;
            let stored: StoredRelation =
                serde_json::from_value(store.read_content(content)?).map_err(json_validation)?;
            if stored.role == "changeset_target" {
                if stored.kind != "ontology_relation" || stored.justification.trim().is_empty() {
                    return Err(validation(format!(
                        "ChangeSet target relation {} is not explicitly justified",
                        relation.key
                    )));
                }
                target_links += 1;
            }
        }
        if target_links != 1 {
            return Err(validation(format!(
                "active ChangeSet {} must have exactly one justified target link",
                change_set.change_id
            )));
        }

        let mut members = Vec::new();
        for relation in snapshot
            .relations
            .iter()
            .filter(|relation| relation.target == change_set_key)
        {
            let relation_content = relation.content.as_ref().ok_or_else(|| {
                validation(format!(
                    "ChangeSet membership relation {} has no content",
                    relation.key
                ))
            })?;
            let stored_relation: StoredRelation =
                serde_json::from_value(store.read_content(relation_content)?)
                    .map_err(json_validation)?;
            if stored_relation.role != "changeset_membership" {
                continue;
            }
            if stored_relation.kind != "ontology_relation"
                || stored_relation.justification.trim().is_empty()
            {
                return Err(validation(format!(
                    "ChangeSet membership relation {} is not explicitly justified",
                    relation.key
                )));
            }
            if overlay_members_by_key.contains_key(&relation.source) {
                return Err(validation(format!(
                    "ontology overlay member {} belongs to multiple active ChangeSets",
                    relation.source
                )));
            }
            let entity = entity_by_key.get(&relation.source).ok_or_else(|| {
                validation(format!(
                    "active ChangeSet {} references missing member {}",
                    change_set.change_id, relation.source
                ))
            })?;
            let member_content = entity.content.as_ref().ok_or_else(|| {
                validation(format!(
                    "active ChangeSet member {} has no content",
                    relation.source
                ))
            })?;
            let value = store.read_content(member_content)?;
            let kind = value
                .get("kind")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    validation(format!(
                        "active ChangeSet member {} has no kind",
                        relation.source
                    ))
                })?
                .to_owned();
            let canonical_id = value
                .get("canonical_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            members.push(relation.source);
            overlay_members_by_key.insert(
                relation.source,
                ActiveOntologyMember {
                    key: relation.source,
                    change_set: change_set_key,
                    membership_relation: relation.key,
                    kind,
                    canonical_id,
                    content_hash: member_content.sha256.clone(),
                    membership_hash: relation_content.sha256.clone(),
                    content: value,
                },
            );
        }
        if members.is_empty() {
            return Err(validation(format!(
                "active ChangeSet {} has no justified members",
                change_set.change_id
            )));
        }
        members.sort();
        active_schema_version = change_set.target_schema_version.clone();
        active_change_sets.push(ActiveOntologyChangeSet {
            key: change_set_key,
            change_id: change_set.change_id,
            base_schema_version: change_set.base_schema_version,
            target_schema_version: change_set.target_schema_version,
            content_hash,
            members,
        });
    }

    for member in overlay_members_by_key.values() {
        match member.kind.as_str() {
            "ontology_extension_definition" => {
                let stored: StoredExtensionDefinition =
                    serde_json::from_value(member.content.clone()).map_err(json_validation)?;
                if stored.kind != "ontology_extension_definition" {
                    return Err(validation("overlay definition content kind mismatch"));
                }
                let compact_symbol = symbol_id(snapshot, &stored.canonical_id)?;
                let definition = OntologyDefinition {
                    key: member.key,
                    kind: stored.definition_kind,
                    canonical_id: stored.canonical_id.clone(),
                    canonical: stored.canonical,
                    compact_symbol,
                    executable: Some(member.content.clone()),
                    endpoint_constraint: stored.endpoint_constraint.clone(),
                    constraint_status: stored
                        .endpoint_constraint
                        .as_ref()
                        .map(|_| "defined".to_owned()),
                    stored_node_type: stored.stored_node_type,
                    physical_profile: stored.physical_profile,
                };
                let target = match stored.definition_kind {
                    OntologyDefinitionKind::StoredNodeType => &mut *stored_node_types,
                    OntologyDefinitionKind::SemanticType => &mut *semantic_types,
                    OntologyDefinitionKind::RelationFamily => &mut *relation_families,
                    OntologyDefinitionKind::Predicate => &mut *predicates,
                    OntologyDefinitionKind::EpistemicStatus => &mut *epistemic_statuses,
                    OntologyDefinitionKind::CompatibilityPredicate => {
                        &mut *compatibility_predicates
                    }
                };
                insert_unique(
                    target,
                    stored.canonical_id,
                    definition,
                    "active overlay definition",
                )?;
            }
            "atom_projection_contract" => {
                let contract_id = member
                    .content
                    .get("contract_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        validation(format!(
                            "active projection contract {} has no contract_id",
                            member.key
                        ))
                    })?
                    .to_owned();
                insert_unique(
                    contracts,
                    contract_id,
                    member.key,
                    "active overlay contract",
                )?;
            }
            _ => {}
        }
    }

    for definition in semantic_types
        .values()
        .filter(|definition| overlay_members_by_key.contains_key(&definition.key))
    {
        let stored_type = definition.stored_node_type.as_ref().ok_or_else(|| {
            validation(format!(
                "active semantic type {} has no stored node type mapping",
                definition.canonical_id
            ))
        })?;
        if !stored_node_types.contains_key(stored_type) {
            return Err(validation(format!(
                "active semantic type {} maps to unknown stored node type {stored_type}",
                definition.canonical_id
            )));
        }
    }
    for definition in predicates
        .values()
        .filter(|definition| overlay_members_by_key.contains_key(&definition.key))
    {
        let profile_key = definition.physical_profile.ok_or_else(|| {
            validation(format!(
                "active predicate {} has no physical profile reference",
                definition.canonical_id
            ))
        })?;
        let profile = overlay_members_by_key.get(&profile_key).ok_or_else(|| {
            validation(format!(
                "active predicate {} references profile {} outside its active ChangeSet",
                definition.canonical_id, profile_key
            ))
        })?;
        if profile.kind != "atom_projection_profile"
            || profile.content.get("predicate").and_then(Value::as_str)
                != Some(definition.canonical_id.as_str())
        {
            return Err(validation(format!(
                "active predicate {} has a mismatched physical profile",
                definition.canonical_id
            )));
        }
    }

    let activation_diagnostics = gaps
        .values()
        .map(|gap| OntologyActivationDiagnostic {
            code: "unresolved_ontology_binding".into(),
            subject: gap.subject.clone(),
            missing: gap.missing.clone(),
            runtime_blocking: true,
        })
        .collect::<Vec<_>>();
    let activation_state = if active_change_sets.is_empty() {
        OntologyActivationState::BaseOnly
    } else if activation_diagnostics.is_empty() {
        OntologyActivationState::Active
    } else {
        OntologyActivationState::ActiveWithBlockedBindings
    };

    Ok((
        active_change_sets,
        overlay_members_by_key,
        active_schema_version,
        activation_state,
        activation_diagnostics,
    ))
}

fn read_entity_content(
    store: &UniverseStore,
    content: Option<&crate::ContentRef>,
    entity: EntityKey,
) -> Result<Value, UniverseError> {
    let content =
        content.ok_or_else(|| validation(format!("ontology entity {entity} has no content")))?;
    store.read_content(content)
}

fn symbol_id(snapshot: &UniverseSnapshot, symbol: &str) -> Result<u32, UniverseError> {
    snapshot
        .symbols
        .iter()
        .position(|candidate| candidate == symbol)
        .and_then(|index| u32::try_from(index).ok())
        .ok_or_else(|| validation(format!("ontology symbol {symbol} is not interned")))
}

fn insert_unique<T>(
    map: &mut BTreeMap<String, T>,
    key: String,
    value: T,
    label: &str,
) -> Result<(), UniverseError> {
    if map.insert(key.clone(), value).is_some() {
        Err(validation(format!("duplicate {label} {key}")))
    } else {
        Ok(())
    }
}

fn json_validation(error: serde_json::Error) -> UniverseError {
    validation(format!("invalid ontology content: {error}"))
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        apply_event, load_seed, EntityRecord, EventRecord, RelationRecord, UniverseMutation,
    };
    use std::path::Path;
    use universe_core::{RelationKey, Revision, Tick};

    #[test]
    fn approved_overlay_activates_after_atomic_symbol_publication_and_reopens() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let seed = load_seed(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/ontology/canonical-ontology.json"),
        )
        .unwrap();
        let snapshot = store.install_seed(&seed).unwrap();
        let manifest = EntityKey(0x1000);
        let change_set = EntityKey(0x3000);
        let semantic_type = EntityKey(0x3010);
        let predicate = EntityKey(0x3020);
        let profile = EntityKey(0x3060);
        let protocol_symbol = snapshot.symbol_id("protocol").unwrap();
        let thing_symbol = snapshot.symbol_id("thing").unwrap();
        let part_of = snapshot.symbol_id("PART_OF").unwrap();
        let proposes_change_to = snapshot.symbol_id("PROPOSES_CHANGE_TO").unwrap();

        let change_content = store
            .append_content(&serde_json::json!({
                "kind": "ontology_changeset",
                "activation": "active_graph_overlay",
                "base_manifest": manifest,
                "base_schema_version": "1.17.0",
                "change_id": "test-active-overlay",
                "status": "approved_by_test_authority",
                "target_schema_version": "1.18.0-test"
            }))
            .unwrap();
        let semantic_content = store
            .append_content(&serde_json::json!({
                "kind": "ontology_extension_definition",
                "definition_kind": "semantic_type",
                "canonical_id": "behavior_bond",
                "canonical": true,
                "stored_node_type": "thing"
            }))
            .unwrap();
        let predicate_content = store
            .append_content(&serde_json::json!({
                "kind": "ontology_extension_definition",
                "definition_kind": "predicate",
                "canonical_id": "SOURCE_ATOM",
                "canonical": true,
                "endpoint_constraint": {
                    "source_types": ["behavior_bond"],
                    "target": "any_atom"
                },
                "physical_profile": profile
            }))
            .unwrap();
        let profile_content = store
            .append_content(&serde_json::json!({
                "kind": "atom_projection_profile",
                "predicate": "SOURCE_ATOM",
                "profile_id": "test-source-atom-profile",
                "status": "active_overlay",
                "logic": {"transfers_energy": false},
                "spatial": {"materialization": "none"}
            }))
            .unwrap();
        let target_content = store
            .append_content(&serde_json::json!({
                "kind": "ontology_relation",
                "role": "changeset_target",
                "justification": "The approved overlay explicitly targets this manifest."
            }))
            .unwrap();
        let membership_contents = [semantic_type, predicate, profile].map(|_| {
            store
                .append_content(&serde_json::json!({
                    "kind": "ontology_relation",
                    "role": "changeset_membership",
                    "justification": "This record is an explicit member of the overlay."
                }))
                .unwrap()
        });

        let graph_mutations = vec![
            UniverseMutation::PutEntity {
                entity: EntityRecord {
                    key: change_set,
                    generation: 0,
                    symbol: protocol_symbol,
                    content: Some(change_content),
                },
            },
            UniverseMutation::PutEntity {
                entity: EntityRecord {
                    key: semantic_type,
                    generation: 0,
                    symbol: thing_symbol,
                    content: Some(semantic_content),
                },
            },
            UniverseMutation::PutEntity {
                entity: EntityRecord {
                    key: predicate,
                    generation: 0,
                    symbol: thing_symbol,
                    content: Some(predicate_content),
                },
            },
            UniverseMutation::PutEntity {
                entity: EntityRecord {
                    key: profile,
                    generation: 0,
                    symbol: thing_symbol,
                    content: Some(profile_content),
                },
            },
            UniverseMutation::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(0x5000),
                    generation: 0,
                    source: change_set,
                    target: manifest,
                    predicate: proposes_change_to,
                    content: Some(target_content),
                },
            },
            UniverseMutation::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(0x5001),
                    generation: 0,
                    source: semantic_type,
                    target: change_set,
                    predicate: part_of,
                    content: Some(membership_contents[0].clone()),
                },
            },
            UniverseMutation::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(0x5002),
                    generation: 0,
                    source: predicate,
                    target: change_set,
                    predicate: part_of,
                    content: Some(membership_contents[1].clone()),
                },
            },
            UniverseMutation::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(0x5003),
                    generation: 0,
                    source: profile,
                    target: change_set,
                    predicate: part_of,
                    content: Some(membership_contents[2].clone()),
                },
            },
        ];
        let graph_event = EventRecord::new(
            snapshot.universe,
            Revision(0),
            Tick(1),
            "test-overlay-graph",
            UniverseMutation::Batch {
                mutations: graph_mutations,
            },
        )
        .unwrap();
        store.append_event(&graph_event).unwrap();
        let with_graph = store.replay(store.load_snapshot().unwrap()).unwrap();

        let required = OntologyRegistry::required_active_overlay_symbols(
            &store,
            &with_graph,
            OntologyLoadBudget::default(),
        )
        .unwrap();
        assert_eq!(required, vec!["SOURCE_ATOM", "behavior_bond"]);
        let unresolved = OntologyRegistry::load(&store, &with_graph, OntologyLoadBudget::default());
        assert!(matches!(
            unresolved,
            Err(UniverseError::Validation(message))
                if message.contains("ontology symbol SOURCE_ATOM is not interned")
                    || message.contains("ontology symbol behavior_bond is not interned")
        ));

        let plan = with_graph.plan_symbol_interning(&required).unwrap();
        assert_eq!(plan.additions, required);
        let symbol_event = EventRecord::new(
            with_graph.universe,
            with_graph.revision,
            Tick(2),
            "test-overlay-symbols",
            UniverseMutation::InternSymbols {
                symbols: plan.additions,
            },
        )
        .unwrap();
        let mut candidate = with_graph.clone();
        apply_event(&mut candidate, &symbol_event).unwrap();
        store.append_event(&symbol_event).unwrap();

        let independent_store = UniverseStore::open(temp.path()).unwrap();
        let independent = independent_store
            .replay(independent_store.load_snapshot().unwrap())
            .unwrap();
        assert_eq!(candidate, independent);
        let registry = OntologyRegistry::load(
            &independent_store,
            &independent,
            OntologyLoadBudget::default(),
        )
        .unwrap();
        assert_eq!(registry.active_schema_version, "1.18.0-test");
        assert_eq!(
            registry.activation_state,
            OntologyActivationState::ActiveWithBlockedBindings
        );
        assert_eq!(registry.active_change_sets.len(), 1);
        assert_eq!(registry.overlay_members_by_key.len(), 3);
        assert_eq!(
            registry.definition_by_key(predicate).unwrap().canonical_id,
            "SOURCE_ATOM"
        );
        assert_eq!(
            registry.active_member(profile).unwrap().content_hash,
            registry.overlay_members_by_key[&profile].content_hash
        );
        assert_eq!(registry.universe_revision, Revision(2));
        assert_eq!(registry.base_manifest_hash.len(), 64);
        assert_eq!(registry.symbol_table_hash.len(), 64);
        assert_eq!(registry.authority_hash.len(), 64);
        let gap_definition = registry.predicate("PROPOSES_CHANGE_TO").unwrap();
        assert_eq!(
            registry.runtime_diagnostics_for(gap_definition.key)[0].code,
            "unresolved_ontology_binding"
        );

        let second_read = OntologyRegistry::load(
            &UniverseStore::open(temp.path()).unwrap(),
            &independent,
            OntologyLoadBudget::default(),
        )
        .unwrap();
        assert_eq!(registry.authority_hash, second_read.authority_hash);
    }
}
