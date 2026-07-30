//! Deterministic generic fixtures shared by integration tests.

use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use universe_core::{EntityKey, RelationKey, Tick, UniverseError, UniverseId};
use universe_store::{
    load_genesis, load_seed,
    ontology::{OntologyActivationState, OntologyLoadBudget, OntologyRegistry},
    EntityRecord, RelationRecord, SeedEntity, SeedRelation, UniverseSnapshot, UniverseStore,
};
use universe_transactions::{
    CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet,
};

pub const MINIMAL_UNIVERSE_ID: UniverseId = UniverseId(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BehaviorBindingPredicateKeys {
    pub source_atom: EntityKey,
    pub target_atom: EntityKey,
    pub uses_predicate: EntityKey,
    pub uses_profile: EntityKey,
    pub has_logic_role: EntityKey,
    pub gated_by: EntityKey,
    pub serves_objective: EntityKey,
    pub justified_by: EntityKey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BehaviorBindingRelationKeys {
    pub source_atom: RelationKey,
    pub target_atom: RelationKey,
    pub uses_predicate: RelationKey,
    pub uses_profile: RelationKey,
    pub has_logic_role: RelationKey,
    pub gated_by: [RelationKey; 2],
    pub serves_objective: RelationKey,
    pub justified_by: RelationKey,
    pub applies_in: RelationKey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BehaviorBondAuthorityKeys {
    pub change_set: EntityKey,
    pub projection_contract: EntityKey,
    pub behavior_bond_type: EntityKey,
    pub logic_role_type: EntityKey,
    pub gate_type: EntityKey,
    pub support_role: EntityKey,
    pub binding_predicates: BehaviorBindingPredicateKeys,
    pub semantic_predicate: EntityKey,
    pub behavior_profile: EntityKey,
    pub behavior_bond: EntityKey,
    pub source: EntityKey,
    pub target: EntityKey,
    pub gates: [EntityKey; 2],
    pub objective: EntityKey,
    pub justification: EntityKey,
    pub context: EntityKey,
    pub binding_relations: BehaviorBindingRelationKeys,
}

pub const BEHAVIOR_BOND_AUTHORITY_KEYS: BehaviorBondAuthorityKeys = BehaviorBondAuthorityKeys {
    change_set: EntityKey(0x3000),
    projection_contract: EntityKey(0x3001),
    behavior_bond_type: EntityKey(0x3010),
    logic_role_type: EntityKey(0x3011),
    gate_type: EntityKey(0x3012),
    support_role: EntityKey(0x3020),
    binding_predicates: BehaviorBindingPredicateKeys {
        source_atom: EntityKey(0x3030),
        target_atom: EntityKey(0x3031),
        uses_predicate: EntityKey(0x3032),
        uses_profile: EntityKey(0x3033),
        has_logic_role: EntityKey(0x3034),
        gated_by: EntityKey(0x3035),
        serves_objective: EntityKey(0x3036),
        justified_by: EntityKey(0x3037),
    },
    semantic_predicate: EntityKey(0x1401),
    behavior_profile: EntityKey(0x3080),
    behavior_bond: EntityKey(0x3070),
    source: EntityKey(0x3071),
    target: EntityKey(0x3072),
    gates: [EntityKey(0x3073), EntityKey(0x3074)],
    objective: EntityKey(0x3075),
    justification: EntityKey(0x3076),
    context: EntityKey(0x3077),
    binding_relations: BehaviorBindingRelationKeys {
        source_atom: RelationKey(0x6100),
        target_atom: RelationKey(0x6101),
        uses_predicate: RelationKey(0x6102),
        uses_profile: RelationKey(0x6103),
        has_logic_role: RelationKey(0x6104),
        gated_by: [RelationKey(0x6105), RelationKey(0x6106)],
        serves_objective: RelationKey(0x6107),
        justified_by: RelationKey(0x6108),
        applies_in: RelationKey(0x6109),
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BehaviorBondAuthorityReadback {
    pub snapshot: UniverseSnapshot,
    pub registry: OntologyRegistry,
    pub keys: BehaviorBondAuthorityKeys,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BehaviorBondAuthorityInstall {
    pub readback: BehaviorBondAuthorityReadback,
    pub receipt: CommitReceipt,
}

#[derive(Debug, Deserialize)]
struct BehaviorBondAuthorityFixture {
    contract: String,
    version: u16,
    universe: UniverseId,
    symbols: Vec<String>,
    entities: Vec<SeedEntity>,
    relations: Vec<SeedRelation>,
}

pub fn minimal_snapshot() -> UniverseSnapshot {
    load_genesis(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/genesis/minimal-genesis.json"),
    )
    .expect("repository Genesis fixture must remain valid")
}

pub fn create_behavior_bond_authority_store(
    root: impl AsRef<Path>,
) -> Result<BehaviorBondAuthorityInstall, UniverseError> {
    let fixture = load_behavior_bond_authority_fixture()?;
    let seed = load_seed(repository_path("fixtures/ontology/canonical-ontology.json"))?;
    if fixture.universe != seed.universe {
        return Err(validation(
            "BehaviorBond authority fixture targets a different Universe",
        ));
    }
    let store = UniverseStore::open(root.as_ref())?;
    let mut snapshot = store.install_seed(&seed)?;
    validate_fixture_patch(&fixture, &snapshot)?;

    let mut requested = fixture.symbols.clone();
    requested.extend(fixture.entities.iter().map(|entity| entity.symbol.clone()));
    requested.extend(
        fixture
            .relations
            .iter()
            .map(|relation| relation.predicate.clone()),
    );
    let symbol_plan = snapshot.plan_symbol_interning(&requested)?;
    let symbol_ids = requested
        .iter()
        .map(|symbol| {
            symbol_plan
                .assignments
                .get(symbol)
                .copied()
                .map(|id| (symbol.as_str(), id))
                .ok_or_else(|| validation(format!("fixture symbol {symbol} was not planned")))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;

    let mut commands = Vec::with_capacity(
        usize::from(!symbol_plan.additions.is_empty())
            + fixture.entities.len()
            + fixture.relations.len(),
    );
    if !symbol_plan.additions.is_empty() {
        commands.push(UniverseCommand::InternSymbols {
            symbols: symbol_plan.additions,
        });
    }
    for entity in fixture.entities {
        commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: entity.key,
                generation: entity.generation,
                symbol: symbol_ids[entity.symbol.as_str()],
                content: Some(store.append_content(&entity.content)?),
            },
        });
    }
    for relation in fixture.relations {
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: relation.key,
                generation: relation.generation,
                source: relation.source,
                target: relation.target,
                predicate: symbol_ids[relation.predicate.as_str()],
                content: relation
                    .content
                    .as_ref()
                    .map(|content| store.append_content(content))
                    .transpose()?,
            },
        });
    }
    let transaction = UniverseTransaction::prepare(
        &snapshot,
        UniverseWriteSet {
            base_revision: snapshot.revision,
            idempotency_key: "fixture:behavior-bond-authority:v1".into(),
            causal_ancestry: vec!["changeset:behavior-bond-authority-v1".into()],
            commands,
        },
    )?;
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    let readback = open_behavior_bond_authority_store(root)?;
    if readback.snapshot != snapshot {
        return Err(validation(
            "independent BehaviorBond authority readback differs from committed state",
        ));
    }
    Ok(BehaviorBondAuthorityInstall { readback, receipt })
}

pub fn open_behavior_bond_authority_store(
    root: impl AsRef<Path>,
) -> Result<BehaviorBondAuthorityReadback, UniverseError> {
    let store = UniverseStore::open(root)?;
    let snapshot = store.replay(store.load_snapshot()?)?;
    let registry = OntologyRegistry::load(&store, &snapshot, OntologyLoadBudget::default())?;
    if registry.activation_state == OntologyActivationState::BaseOnly
        || !registry
            .active_change_sets
            .iter()
            .any(|change_set| change_set.key == BEHAVIOR_BOND_AUTHORITY_KEYS.change_set)
    {
        return Err(validation(
            "BehaviorBond authority ChangeSet is not active in the registry",
        ));
    }
    if registry
        .runtime_diagnostics_for(BEHAVIOR_BOND_AUTHORITY_KEYS.semantic_predicate)
        .iter()
        .any(|diagnostic| diagnostic.runtime_blocking)
    {
        return Err(validation(
            "BehaviorBond semantic predicate has a runtime-blocking ontology diagnostic",
        ));
    }
    verify_all_content_hashes(&store, &snapshot)?;
    verify_relation_authority(&store, &snapshot, &registry)?;
    Ok(BehaviorBondAuthorityReadback {
        snapshot,
        registry,
        keys: BEHAVIOR_BOND_AUTHORITY_KEYS,
    })
}

fn load_behavior_bond_authority_fixture() -> Result<BehaviorBondAuthorityFixture, UniverseError> {
    let bytes = fs::read(repository_path(
        "fixtures/ontology/behavior-bond-authority.json",
    ))
    .map_err(|error| UniverseError::Io(error.to_string()))?;
    let fixture: BehaviorBondAuthorityFixture = serde_json::from_slice(&bytes)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    if fixture.contract != "mind-universe-authority-fixture" || fixture.version != 0 {
        return Err(UniverseError::UnsupportedVersion(fixture.version));
    }
    Ok(fixture)
}

fn validate_fixture_patch(
    fixture: &BehaviorBondAuthorityFixture,
    snapshot: &UniverseSnapshot,
) -> Result<(), UniverseError> {
    let base_entities = snapshot
        .entities
        .iter()
        .map(|entity| entity.key)
        .collect::<BTreeSet<_>>();
    let fixture_entities = fixture
        .entities
        .iter()
        .map(|entity| entity.key)
        .collect::<BTreeSet<_>>();
    if fixture_entities.len() != fixture.entities.len() {
        return Err(validation(
            "BehaviorBond authority fixture has duplicate entity keys",
        ));
    }
    if fixture_entities
        .iter()
        .any(|key| base_entities.contains(key))
    {
        return Err(validation(
            "BehaviorBond authority fixture overwrites a base entity",
        ));
    }
    let fixture_relations = fixture
        .relations
        .iter()
        .map(|relation| relation.key)
        .collect::<BTreeSet<_>>();
    if fixture_relations.len() != fixture.relations.len()
        || fixture_relations.iter().any(|key| {
            snapshot
                .relations
                .iter()
                .any(|relation| relation.key == *key)
        })
    {
        return Err(validation(
            "BehaviorBond authority fixture has duplicate relation keys",
        ));
    }
    let all_entities = base_entities
        .union(&fixture_entities)
        .copied()
        .collect::<BTreeSet<_>>();
    if fixture.relations.iter().any(|relation| {
        !all_entities.contains(&relation.source) || !all_entities.contains(&relation.target)
    }) {
        return Err(validation(
            "BehaviorBond authority relation has an unknown endpoint",
        ));
    }
    let expected_entities = [
        BEHAVIOR_BOND_AUTHORITY_KEYS.change_set,
        BEHAVIOR_BOND_AUTHORITY_KEYS.projection_contract,
        BEHAVIOR_BOND_AUTHORITY_KEYS.behavior_bond_type,
        BEHAVIOR_BOND_AUTHORITY_KEYS.logic_role_type,
        BEHAVIOR_BOND_AUTHORITY_KEYS.gate_type,
        BEHAVIOR_BOND_AUTHORITY_KEYS.support_role,
        BEHAVIOR_BOND_AUTHORITY_KEYS.behavior_profile,
        BEHAVIOR_BOND_AUTHORITY_KEYS.behavior_bond,
        BEHAVIOR_BOND_AUTHORITY_KEYS.source,
        BEHAVIOR_BOND_AUTHORITY_KEYS.target,
        BEHAVIOR_BOND_AUTHORITY_KEYS.gates[0],
        BEHAVIOR_BOND_AUTHORITY_KEYS.gates[1],
        BEHAVIOR_BOND_AUTHORITY_KEYS.objective,
        BEHAVIOR_BOND_AUTHORITY_KEYS.justification,
        BEHAVIOR_BOND_AUTHORITY_KEYS.context,
    ];
    if expected_entities
        .iter()
        .any(|key| !fixture_entities.contains(key))
    {
        return Err(validation(
            "BehaviorBond authority fixture is missing an expected entity",
        ));
    }
    Ok(())
}

fn verify_all_content_hashes(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
) -> Result<(), UniverseError> {
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
    }
    Ok(())
}

fn verify_relation_authority(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    registry: &OntologyRegistry,
) -> Result<(), UniverseError> {
    let keys = BEHAVIOR_BOND_AUTHORITY_KEYS;
    let predicates = keys.binding_predicates;
    let relations = keys.binding_relations;
    let expected = [
        (relations.source_atom, predicates.source_atom, keys.source),
        (relations.target_atom, predicates.target_atom, keys.target),
        (
            relations.uses_predicate,
            predicates.uses_predicate,
            keys.semantic_predicate,
        ),
        (
            relations.uses_profile,
            predicates.uses_profile,
            keys.behavior_profile,
        ),
        (
            relations.has_logic_role,
            predicates.has_logic_role,
            keys.support_role,
        ),
        (relations.gated_by[0], predicates.gated_by, keys.gates[0]),
        (relations.gated_by[1], predicates.gated_by, keys.gates[1]),
        (
            relations.serves_objective,
            predicates.serves_objective,
            keys.objective,
        ),
        (
            relations.justified_by,
            predicates.justified_by,
            keys.justification,
        ),
    ];
    for (relation_key, predicate_key, target) in expected {
        let relation = snapshot
            .relations
            .iter()
            .find(|relation| relation.key == relation_key)
            .ok_or_else(|| {
                validation(format!(
                    "BehaviorBond binding relation {relation_key} is missing"
                ))
            })?;
        let predicate = registry.definition_by_key(predicate_key).ok_or_else(|| {
            validation(format!(
                "BehaviorBond binding predicate definition {predicate_key} is missing"
            ))
        })?;
        if relation.source != keys.behavior_bond
            || relation.target != target
            || relation.predicate != predicate.compact_symbol
        {
            return Err(validation(format!(
                "BehaviorBond binding relation {relation_key} does not match its authority keys"
            )));
        }
    }
    let bond = snapshot
        .entities
        .iter()
        .find(|entity| entity.key == keys.behavior_bond)
        .ok_or_else(|| validation("BehaviorBond entity is missing"))?;
    let bond_content = store.read_content(
        bond.content
            .as_ref()
            .ok_or_else(|| validation("BehaviorBond entity has no content"))?,
    )?;
    for forbidden in [
        "source",
        "target",
        "predicate",
        "profile",
        "logic_role",
        "gates",
        "objective",
        "justifications",
        "context",
        "authority",
        "budgets",
    ] {
        if bond_content.get(forbidden).is_some() {
            return Err(validation(format!(
                "BehaviorBond content duplicates relation binding {forbidden}"
            )));
        }
    }
    let runtime_binding = bond_content
        .get("runtime_binding")
        .ok_or_else(|| validation("BehaviorBond content has no runtime_binding"))?;
    if runtime_binding
        .get("kind")
        .and_then(serde_json::Value::as_str)
        != Some("bond")
    {
        return Err(validation(
            "BehaviorBond runtime_binding does not contain bond properties",
        ));
    }
    let authority = runtime_binding
        .get("value")
        .and_then(|value| value.get("authority"))
        .ok_or_else(|| validation("BehaviorBond runtime_binding has no authority"))?;
    let active_change_set = registry
        .active_change_sets
        .iter()
        .find(|change_set| change_set.key == keys.change_set)
        .ok_or_else(|| validation("BehaviorBond ChangeSet is not active"))?;
    let projection_contract = registry
        .active_member(keys.projection_contract)
        .ok_or_else(|| validation("BehaviorBond projection contract is not active"))?;
    for (field, expected) in [
        ("change_set", keys.change_set.to_string()),
        ("context", keys.context.to_string()),
        ("change_set_hash", active_change_set.content_hash.clone()),
        ("ontology_hash", registry.base_manifest_hash.clone()),
        ("mapping_hash", projection_contract.content_hash.clone()),
    ] {
        if authority.get(field).and_then(serde_json::Value::as_str) != Some(expected.as_str()) {
            return Err(validation(format!(
                "BehaviorBond authority field {field} does not match active readback"
            )));
        }
    }
    if authority
        .get("universe_revision")
        .and_then(serde_json::Value::as_u64)
        != Some(snapshot.revision.0)
    {
        return Err(validation(
            "BehaviorBond authority universe_revision does not match readback",
        ));
    }
    let context_relation = snapshot
        .relations
        .iter()
        .find(|relation| relation.key == relations.applies_in)
        .ok_or_else(|| validation("BehaviorBond context relation is missing"))?;
    if context_relation.source != keys.behavior_bond || context_relation.target != keys.context {
        return Err(validation(
            "BehaviorBond context relation does not match authority keys",
        ));
    }
    let profile = registry
        .active_member(keys.behavior_profile)
        .ok_or_else(|| validation("BehaviorBond physical profile is not active"))?;
    if profile
        .content
        .pointer("/runtime_binding/kind")
        .and_then(serde_json::Value::as_str)
        != Some("physical_profile")
    {
        return Err(validation(
            "BehaviorBond profile has no physical_profile runtime_binding",
        ));
    }
    let logic_role = registry
        .active_member(keys.support_role)
        .ok_or_else(|| validation("BehaviorBond logic role is not active"))?;
    if logic_role
        .content
        .pointer("/runtime_binding/kind")
        .and_then(serde_json::Value::as_str)
        != Some("logic_role")
    {
        return Err(validation(
            "BehaviorBond logic role has no generic runtime_binding",
        ));
    }
    for gate in keys.gates {
        let gate = registry
            .active_member(gate)
            .ok_or_else(|| validation("BehaviorBond gate is not active"))?;
        if gate
            .content
            .pointer("/runtime_binding/kind")
            .and_then(serde_json::Value::as_str)
            != Some("gate")
        {
            return Err(validation("BehaviorBond gate has no runtime_binding"));
        }
    }
    Ok(())
}

fn repository_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_core::Revision;

    #[test]
    fn fixture_is_deterministic_and_valid() {
        let a = minimal_snapshot();
        let b = minimal_snapshot();
        assert_eq!(a.canonical_hash().unwrap(), b.canonical_hash().unwrap());
        a.validate().unwrap();
        assert_eq!(a.symbols[a.entities[0].symbol as usize], "Actor");
        assert!(a.relations.iter().any(|relation| {
            a.symbols[relation.predicate as usize] == "result_type"
                && a.symbols[a
                    .entities
                    .iter()
                    .find(|entity| entity.key == relation.target)
                    .unwrap()
                    .symbol as usize]
                    == "Moment"
        }));
    }

    #[test]
    fn behavior_bond_authority_store_is_deterministic_and_independently_readable() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_install = create_behavior_bond_authority_store(first.path()).unwrap();
        let second_install = create_behavior_bond_authority_store(second.path()).unwrap();
        assert!(matches!(
            first_install.receipt,
            CommitReceipt::Committed {
                previous_revision: Revision(0),
                revision: Revision(1),
                ..
            }
        ));
        assert_eq!(
            first_install.readback.snapshot.canonical_hash().unwrap(),
            second_install.readback.snapshot.canonical_hash().unwrap()
        );
        assert_eq!(
            first_install.readback.registry.authority_hash,
            second_install.readback.registry.authority_hash
        );
        let reopened = open_behavior_bond_authority_store(first.path()).unwrap();
        assert_eq!(reopened, first_install.readback);
        assert!(OntologyRegistry::required_active_overlay_symbols(
            &UniverseStore::open(first.path()).unwrap(),
            &reopened.snapshot,
            OntologyLoadBudget::default()
        )
        .unwrap()
        .is_empty());
        let keys = reopened.keys;
        let profile = reopened
            .registry
            .active_member(keys.behavior_profile)
            .unwrap();
        assert_eq!(
            profile
                .content
                .pointer("/runtime_binding/value/transfer_energy"),
            Some(&serde_json::json!(100))
        );
        assert_eq!(
            reopened
                .snapshot
                .entities
                .iter()
                .find(|entity| entity.key == keys.behavior_profile)
                .unwrap()
                .content
                .as_ref()
                .unwrap()
                .sha256,
            profile.content_hash
        );
        let logic = reopened.registry.active_member(keys.support_role).unwrap();
        assert_eq!(
            logic.content.pointer("/runtime_binding/value/kind"),
            Some(&serde_json::json!("support"))
        );
        assert_eq!(
            reopened
                .snapshot
                .entities
                .iter()
                .find(|entity| entity.key == keys.support_role)
                .unwrap()
                .content
                .as_ref()
                .unwrap()
                .sha256,
            logic.content_hash
        );
    }

    #[test]
    fn invalid_behavior_batch_does_not_publish_a_valid_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let install = create_behavior_bond_authority_store(temp.path()).unwrap();
        let snapshot = install.readback.snapshot;
        let before_hash = snapshot.canonical_hash().unwrap();
        let symbol = snapshot.symbol_id("thing").unwrap();
        let invalid = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: snapshot.revision,
                idempotency_key: "fixture:invalid-prefix".into(),
                causal_ancestry: vec![],
                commands: vec![
                    UniverseCommand::PutEntity {
                        entity: EntityRecord {
                            key: EntityKey(0x9f00),
                            generation: 0,
                            symbol,
                            content: None,
                        },
                    },
                    UniverseCommand::PutRelation {
                        relation: RelationRecord {
                            key: RelationKey(0x9f01),
                            generation: 0,
                            source: EntityKey(0x9f00),
                            target: EntityKey(0xdead),
                            predicate: snapshot.symbol_id("PART_OF").unwrap(),
                            content: None,
                        },
                    },
                ],
            },
        );
        assert!(matches!(invalid, Err(UniverseError::Validation(_))));
        assert_eq!(snapshot.canonical_hash().unwrap(), before_hash);
        assert!(snapshot
            .entities
            .iter()
            .all(|entity| entity.key != EntityKey(0x9f00)));
    }
}
