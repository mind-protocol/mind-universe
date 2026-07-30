//! The MutationBond's last mile: the generic write-side translator.
//!
//! A decision (an IR `Propose`) becomes a real graph mutation through exactly ONE
//! generic function instead of a per-caller Rust `translate` closure that hardcodes
//! a verb and field names. The closed [`MutationCommandKind`] makes "only the four
//! kernel write verbs, never a fifth" a *type-level* guarantee: the translator's
//! match is exhaustive, so a new verb is a compile error, not a runtime escape.
//! This is the write-side analog of `BehaviorLogicKind{Support,Inhibit,Neutral}`.

use serde_json::Value;
use universe_core::{EntityKey, RelationKey, Revision, UniverseError};
use universe_store::{ContentRef, EntityRecord, RelationRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseWriteSet};
use universe_vm::ExecutionReceipt;

/// The four kernel write verbs, as a closed type. Isomorphic to the
/// `UniverseCommand` variants a MutationBond may compile to; a fifth verb is
/// unrepresentable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationCommandKind {
    InternSymbols,
    PutEntity,
    PutRelation,
    TombstoneRelation,
}

/// A materialized mutation plan: exactly one kernel verb plus its graph-shaped
/// skeleton. The static parts (keys, symbol, endpoints) come from the bond; the
/// dynamic content, when the verb carries any, is named by `content_field` and
/// filled from the runtime proposal — never defaulted.
#[derive(Clone, Debug)]
pub enum MutationPlan {
    InternSymbols {
        symbols: Vec<String>,
    },
    PutEntity {
        key: EntityKey,
        generation: u32,
        symbol: u32,
        /// Name of the proposal-record field that supplies this entity's content,
        /// or `None` for a content-free entity.
        content_field: Option<String>,
    },
    PutRelation {
        key: RelationKey,
        generation: u32,
        source: EntityKey,
        target: EntityKey,
        predicate: u32,
        content_field: Option<String>,
    },
    TombstoneRelation {
        relation: RelationKey,
        generation: u32,
    },
}

impl MutationPlan {
    /// The kernel verb this plan compiles to.
    pub fn kind(&self) -> MutationCommandKind {
        match self {
            MutationPlan::InternSymbols { .. } => MutationCommandKind::InternSymbols,
            MutationPlan::PutEntity { .. } => MutationCommandKind::PutEntity,
            MutationPlan::PutRelation { .. } => MutationCommandKind::PutRelation,
            MutationPlan::TombstoneRelation { .. } => MutationCommandKind::TombstoneRelation,
        }
    }
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

/// Resolve the plan's content field from the runtime proposal and content-address
/// it. A `None` field means the verb carries no content. Otherwise the proposal
/// MUST be an object carrying that field with a non-null value — a missing,
/// non-object, or null field is a `Validation` failure, **never** a default. This
/// is the epistemic-honesty rule at the write boundary: absent content stays
/// absent; it is not coerced to `Unit`, `0`, or `{}`.
fn resolve_content(
    proposal: &Value,
    content_field: &Option<String>,
    store: &UniverseStore,
) -> Result<Option<ContentRef>, UniverseError> {
    let Some(field) = content_field else {
        return Ok(None);
    };
    let object = proposal
        .as_object()
        .ok_or_else(|| validation("mutation proposal must be a JSON object"))?;
    let value = object
        .get(field)
        .ok_or_else(|| validation(format!("proposal is missing content field `{field}`")))?;
    if value.is_null() {
        return Err(validation(format!(
            "proposal content field `{field}` is null — absent content is not a default"
        )));
    }
    Ok(Some(store.append_content(value)?))
}

/// Compile a materialized [`MutationPlan`] plus a runtime proposal into exactly
/// ONE atomic [`UniverseWriteSet`] command. The `match` is total over the closed
/// four-verb set, so every mutation compiles down to a single native kernel verb
/// and nothing can escape the boundary. This replaces the bespoke per-caller
/// `translate_fixture_proposal` closure with one generic path.
pub fn translate_mutation_proposal(
    plan: &MutationPlan,
    proposal: &Value,
    store: &UniverseStore,
    base_revision: Revision,
    idempotency_key: String,
    causal_ancestry: Vec<String>,
) -> Result<UniverseWriteSet, UniverseError> {
    if idempotency_key.trim().is_empty() {
        return Err(validation("mutation idempotency key is empty"));
    }
    let command = match plan {
        MutationPlan::InternSymbols { symbols } => {
            if symbols.is_empty() {
                return Err(validation("InternSymbols plan carries no symbols"));
            }
            UniverseCommand::InternSymbols {
                symbols: symbols.clone(),
            }
        }
        MutationPlan::PutEntity {
            key,
            generation,
            symbol,
            content_field,
        } => {
            let content = resolve_content(proposal, content_field, store)?;
            UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key: *key,
                    generation: *generation,
                    symbol: *symbol,
                    content,
                },
            }
        }
        MutationPlan::PutRelation {
            key,
            generation,
            source,
            target,
            predicate,
            content_field,
        } => {
            let content = resolve_content(proposal, content_field, store)?;
            UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: *key,
                    generation: *generation,
                    source: *source,
                    target: *target,
                    predicate: *predicate,
                    content,
                },
            }
        }
        MutationPlan::TombstoneRelation {
            relation,
            generation,
        } => UniverseCommand::TombstoneRelation {
            relation: *relation,
            generation: *generation,
        },
    };
    Ok(UniverseWriteSet {
        base_revision,
        idempotency_key,
        causal_ancestry,
        commands: vec![command],
    })
}

/// Bridge a VM (`universe_ir`) value to a plain serde_json value so content a
/// runtime `Propose` produced can be content-addressed. Unlike serde's tagged
/// form (`{"type":"record",...}`), this yields plain objects/strings — a record's
/// fields become object keys. Values that cannot occur inside placement content
/// (`Content`/`Epistemic`) degrade to null rather than fabricating a shape.
pub fn ir_value_to_json(value: &universe_ir::Value) -> Value {
    use universe_ir::Value as Ir;
    match value {
        Ir::Unit => Value::Null,
        Ir::Bool(flag) => Value::Bool(*flag),
        Ir::Integer(number) => Value::Number((*number).into()),
        Ir::Text(text) => Value::String(text.clone()),
        Ir::Entity(key) => Value::String(format!("{key}")),
        Ir::List(items) => Value::Array(items.iter().map(ir_value_to_json).collect()),
        Ir::Record(fields) => Value::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), ir_value_to_json(value)))
                .collect(),
        ),
        Ir::Content(_) | Ir::Epistemic(_) | Ir::EpistemicState(_) => Value::Null,
    }
}

/// The wieldable runtime path: take the single proposal a VM `Propose` produced,
/// bridge it to json, and compile it through the pure [`translate_mutation_proposal`].
/// This is what makes "place a node" a *runtime gesture* instead of a hand-built
/// write set — the generic analog of the fixture-only `translate_fixture_proposal`.
pub fn translate_mutation_receipt(
    plan: &MutationPlan,
    receipt: &ExecutionReceipt,
    store: &UniverseStore,
    base_revision: Revision,
    idempotency_key: String,
    causal_ancestry: Vec<String>,
) -> Result<Option<UniverseWriteSet>, UniverseError> {
    if receipt.proposals.len() != 1 {
        return Err(validation(format!(
            "expected exactly one graph proposal, found {}",
            receipt.proposals.len()
        )));
    }
    let proposal = ir_value_to_json(&receipt.proposals[0].command);
    translate_mutation_proposal(
        plan,
        &proposal,
        store,
        base_revision,
        idempotency_key,
        causal_ancestry,
    )
    .map(Some)
}

/// The mutation shape a MutationBond graph object furnishes: which kernel verb it
/// emits, the semantic type of the entity it writes, the proposal field carrying
/// the content, and the content contract (required fields). Projected FROM the
/// bond so the runtime hardcodes none of it — the bond in the graph IS the action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BondProjection {
    pub command_kind: MutationCommandKind,
    pub content_kind: String,
    pub content_field: String,
    pub required_fields: Vec<String>,
}

/// Project a MutationBond into its [`BondProjection`]. `bond_content` is the bond
/// instance node's content (carrying `runtime_binding.value.command_kind`);
/// `field_schema_content` is the node reached by the bond's `USES_FIELD_SCHEMA`
/// relation (carrying `field_schema.content_kind` + `required_fields`). Kept pure
/// over the two content documents so the graph-walk (relation resolution, content
/// reads) stays in the caller and this stays unit-testable.
pub fn project_mutation_bond(
    bond_content: &Value,
    field_schema_content: &Value,
) -> Result<BondProjection, UniverseError> {
    let command_kind = match bond_content
        .pointer("/runtime_binding/value/command_kind")
        .and_then(Value::as_str)
        .ok_or_else(|| validation("bond content lacks runtime_binding.value.command_kind"))?
    {
        "intern_symbols" => MutationCommandKind::InternSymbols,
        "put_entity" => MutationCommandKind::PutEntity,
        "put_relation" => MutationCommandKind::PutRelation,
        "tombstone_relation" => MutationCommandKind::TombstoneRelation,
        other => {
            return Err(validation(format!(
                "bond command_kind `{other}` is not one of the four kernel verbs"
            )))
        }
    };
    let schema = field_schema_content
        .get("field_schema")
        .ok_or_else(|| validation("field schema content lacks a `field_schema` object"))?;
    let content_kind = schema
        .get("content_kind")
        .and_then(Value::as_str)
        .ok_or_else(|| validation("field schema lacks content_kind"))?
        .to_string();
    let required_fields = schema
        .get("required_fields")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    // The proposal field carrying the content; runtime convention is `content`
    // unless the schema names another.
    let content_field = schema
        .get("proposal_field")
        .and_then(Value::as_str)
        .unwrap_or("content")
        .to_string();
    Ok(BondProjection {
        command_kind,
        content_kind,
        content_field,
        required_fields,
    })
}

impl BondProjection {
    /// Complete the projected shape into a runnable [`MutationPlan`] once the
    /// runtime has resolved the target key and its interned symbol.
    pub fn into_put_entity_plan(
        &self,
        key: EntityKey,
        symbol: u32,
    ) -> Result<MutationPlan, UniverseError> {
        if self.command_kind != MutationCommandKind::PutEntity {
            return Err(validation(format!(
                "into_put_entity_plan called on a {:?} bond",
                self.command_kind
            )));
        }
        Ok(MutationPlan::PutEntity {
            key,
            generation: 0,
            symbol,
            content_field: Some(self.content_field.clone()),
        })
    }

    /// Enforce the bond's content contract: the proposed content must carry every
    /// field the schema requires. A missing field is a failure, never a default.
    pub fn validate_content(&self, content: &Value) -> Result<(), UniverseError> {
        let object = content
            .as_object()
            .ok_or_else(|| validation("proposed content is not an object"))?;
        for field in &self.required_fields {
            if !object.contains_key(field) {
                return Err(validation(format!(
                    "proposed content is missing required field `{field}`"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_proposal() -> Value {
        serde_json::json!({})
    }

    #[test]
    fn kind_maps_each_variant() {
        assert_eq!(
            MutationPlan::InternSymbols { symbols: vec![] }.kind(),
            MutationCommandKind::InternSymbols
        );
        assert_eq!(
            MutationPlan::PutEntity {
                key: EntityKey(1),
                generation: 0,
                symbol: 0,
                content_field: None
            }
            .kind(),
            MutationCommandKind::PutEntity
        );
        assert_eq!(
            MutationPlan::PutRelation {
                key: RelationKey(1),
                generation: 0,
                source: EntityKey(1),
                target: EntityKey(2),
                predicate: 0,
                content_field: None
            }
            .kind(),
            MutationCommandKind::PutRelation
        );
        assert_eq!(
            MutationPlan::TombstoneRelation {
                relation: RelationKey(1),
                generation: 0
            }
            .kind(),
            MutationCommandKind::TombstoneRelation
        );
    }

    fn scratch_store() -> (tempfile::TempDir, UniverseStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn intern_symbols_compiles_to_one_verb() {
        let (_dir, store) = scratch_store();
        let plan = MutationPlan::InternSymbols {
            symbols: vec!["built_position".into()],
        };
        let ws = translate_mutation_proposal(
            &plan,
            &empty_proposal(),
            &store,
            Revision(0),
            "mutation:test:v0".into(),
            vec!["changeset:test".into()],
        )
        .unwrap();
        assert_eq!(ws.commands.len(), 1);
        assert!(matches!(ws.commands[0], UniverseCommand::InternSymbols { .. }));
    }

    #[test]
    fn put_entity_without_content_needs_no_proposal_field() {
        let (_dir, store) = scratch_store();
        let plan = MutationPlan::PutEntity {
            key: EntityKey(0x9000),
            generation: 0,
            symbol: 7,
            content_field: None,
        };
        let ws = translate_mutation_proposal(
            &plan,
            &empty_proposal(),
            &store,
            Revision(3),
            "mutation:test:v0".into(),
            vec![],
        )
        .unwrap();
        match &ws.commands[0] {
            UniverseCommand::PutEntity { entity } => {
                assert_eq!(entity.key, EntityKey(0x9000));
                assert_eq!(entity.symbol, 7);
                assert!(entity.content.is_none());
            }
            other => panic!("expected PutEntity, got {other:?}"),
        }
        assert_eq!(ws.base_revision, Revision(3));
    }

    #[test]
    fn put_entity_with_content_is_content_addressed() {
        let (_dir, store) = scratch_store();
        let plan = MutationPlan::PutEntity {
            key: EntityKey(0x9000),
            generation: 0,
            symbol: 7,
            content_field: Some("content".into()),
        };
        let proposal = serde_json::json!({
            "mutation_bond": "0x4070",
            "content": {"kind": "built_position", "x": 12.0, "y": 0.0, "z": -4.0, "provenance": "built"}
        });
        let ws = translate_mutation_proposal(
            &plan,
            &proposal,
            &store,
            Revision(0),
            "mutation:test:v0".into(),
            vec![],
        )
        .unwrap();
        match &ws.commands[0] {
            UniverseCommand::PutEntity { entity } => {
                let content_ref = entity.content.as_ref().expect("content addressed");
                let read = store.read_content(content_ref).unwrap();
                assert_eq!(read["x"], 12.0);
                assert_eq!(read["provenance"], "built");
            }
            other => panic!("expected PutEntity, got {other:?}"),
        }
    }

    #[test]
    fn put_relation_carries_endpoints() {
        let (_dir, store) = scratch_store();
        let plan = MutationPlan::PutRelation {
            key: RelationKey(0x9100),
            generation: 0,
            source: EntityKey(0x1000),
            target: EntityKey(0x9000),
            predicate: 42,
            content_field: None,
        };
        let ws = translate_mutation_proposal(
            &plan,
            &empty_proposal(),
            &store,
            Revision(0),
            "mutation:test:v0".into(),
            vec![],
        )
        .unwrap();
        match &ws.commands[0] {
            UniverseCommand::PutRelation { relation } => {
                assert_eq!(relation.source, EntityKey(0x1000));
                assert_eq!(relation.target, EntityKey(0x9000));
                assert_eq!(relation.predicate, 42);
            }
            other => panic!("expected PutRelation, got {other:?}"),
        }
    }

    #[test]
    fn tombstone_relation_compiles() {
        let (_dir, store) = scratch_store();
        let plan = MutationPlan::TombstoneRelation {
            relation: RelationKey(0x9100),
            generation: 1,
        };
        let ws = translate_mutation_proposal(
            &plan,
            &empty_proposal(),
            &store,
            Revision(0),
            "mutation:test:v0".into(),
            vec![],
        )
        .unwrap();
        assert!(matches!(
            ws.commands[0],
            UniverseCommand::TombstoneRelation {
                relation: RelationKey(0x9100),
                generation: 1
            }
        ));
    }

    #[test]
    fn missing_content_field_is_a_failure_not_a_default() {
        let (_dir, store) = scratch_store();
        let plan = MutationPlan::PutEntity {
            key: EntityKey(0x9000),
            generation: 0,
            symbol: 7,
            content_field: Some("content".into()),
        };
        // Proposal is missing the named `content` field.
        let err = translate_mutation_proposal(
            &plan,
            &serde_json::json!({"mutation_bond": "0x4070"}),
            &store,
            Revision(0),
            "mutation:test:v0".into(),
            vec![],
        )
        .unwrap_err();
        assert!(matches!(err, UniverseError::Validation(_)));
    }

    #[test]
    fn null_content_field_is_a_failure() {
        let (_dir, store) = scratch_store();
        let plan = MutationPlan::PutEntity {
            key: EntityKey(0x9000),
            generation: 0,
            symbol: 7,
            content_field: Some("content".into()),
        };
        let err = translate_mutation_proposal(
            &plan,
            &serde_json::json!({"content": null}),
            &store,
            Revision(0),
            "mutation:test:v0".into(),
            vec![],
        )
        .unwrap_err();
        assert!(matches!(err, UniverseError::Validation(_)));
    }

    #[test]
    fn empty_idempotency_key_is_rejected() {
        let (_dir, store) = scratch_store();
        let plan = MutationPlan::TombstoneRelation {
            relation: RelationKey(1),
            generation: 0,
        };
        let err = translate_mutation_proposal(
            &plan,
            &empty_proposal(),
            &store,
            Revision(0),
            "   ".into(),
            vec![],
        )
        .unwrap_err();
        assert!(matches!(err, UniverseError::Validation(_)));
    }

    #[test]
    fn ir_value_to_json_yields_plain_objects() {
        let ir = universe_ir::Value::Record(std::collections::BTreeMap::from([
            (
                "kind".to_string(),
                universe_ir::Value::Text("built_position".into()),
            ),
            ("weight".to_string(), universe_ir::Value::Integer(7)),
        ]));
        let json = ir_value_to_json(&ir);
        assert_eq!(json["kind"], "built_position");
        assert_eq!(json["weight"], 7);
    }

    #[test]
    fn translate_mutation_receipt_compiles_a_runtime_proposal() {
        let (_dir, store) = scratch_store();
        let content = universe_ir::Value::Record(std::collections::BTreeMap::from([
            (
                "kind".to_string(),
                universe_ir::Value::Text("built_position".into()),
            ),
            (
                "provenance".to_string(),
                universe_ir::Value::Text("built".into()),
            ),
        ]));
        let command = universe_ir::Value::Record(std::collections::BTreeMap::from([(
            "content".to_string(),
            content,
        )]));
        let receipt = universe_vm::ExecutionReceipt {
            code_revision: Revision(0),
            starting_universe_revision: Revision(0),
            starting_tick: universe_core::Tick(0),
            code_hash: "test".into(),
            fuel_used: 0,
            result: universe_ir::Value::Unit,
            proposals: vec![universe_vm::WriteProposal { command }],
            trace: vec![],
        };
        let plan = MutationPlan::PutEntity {
            key: EntityKey(0x9020),
            generation: 0,
            symbol: 7,
            content_field: Some("content".into()),
        };
        let ws = translate_mutation_receipt(
            &plan,
            &receipt,
            &store,
            Revision(0),
            "mutation:receipt:v0".into(),
            vec![],
        )
        .unwrap()
        .unwrap();
        match &ws.commands[0] {
            UniverseCommand::PutEntity { entity } => {
                let read = store.read_content(entity.content.as_ref().unwrap()).unwrap();
                assert_eq!(read["kind"], "built_position");
                assert_eq!(read["provenance"], "built");
            }
            other => panic!("expected PutEntity, got {other:?}"),
        }
    }

    #[test]
    fn project_mutation_bond_reads_the_shape_from_the_graph() {
        let bond = serde_json::json!({"runtime_binding":{"value":{"command_kind":"put_entity"}}});
        let schema = serde_json::json!({"field_schema":{"content_kind":"built_position","required_fields":["x","y","z"]}});
        let projection = project_mutation_bond(&bond, &schema).unwrap();
        assert_eq!(projection.command_kind, MutationCommandKind::PutEntity);
        assert_eq!(projection.content_kind, "built_position");
        assert_eq!(projection.content_field, "content");
        assert_eq!(projection.required_fields, vec!["x", "y", "z"]);
    }

    #[test]
    fn projected_bond_builds_a_put_entity_plan() {
        let bond = serde_json::json!({"runtime_binding":{"value":{"command_kind":"put_entity"}}});
        let schema = serde_json::json!({"field_schema":{"content_kind":"built_position","required_fields":["x"]}});
        let projection = project_mutation_bond(&bond, &schema).unwrap();
        let plan = projection.into_put_entity_plan(EntityKey(0x9020), 7).unwrap();
        match plan {
            MutationPlan::PutEntity {
                key,
                symbol,
                content_field,
                ..
            } => {
                assert_eq!(key, EntityKey(0x9020));
                assert_eq!(symbol, 7);
                assert_eq!(content_field.as_deref(), Some("content"));
            }
            other => panic!("expected PutEntity plan, got {other:?}"),
        }
    }

    #[test]
    fn bond_content_contract_rejects_a_missing_required_field() {
        let bond = serde_json::json!({"runtime_binding":{"value":{"command_kind":"put_entity"}}});
        let schema = serde_json::json!({"field_schema":{"content_kind":"built_position","required_fields":["x","y","z"]}});
        let projection = project_mutation_bond(&bond, &schema).unwrap();
        projection
            .validate_content(&serde_json::json!({"x":1.0,"y":2.0,"z":3.0}))
            .unwrap();
        let err = projection
            .validate_content(&serde_json::json!({"x":1.0,"y":2.0}))
            .unwrap_err();
        assert!(matches!(err, UniverseError::Validation(_)));
    }

    #[test]
    fn projects_the_real_mutation_bond_authority_fixture() {
        // The projection reads the shape from the ACTUAL authored fixture, not a
        // synthetic stand-in: the real bond in the graph furnishes the plan.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/ontology/mutation-bond-authority.json");
        let fixture: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let entities = fixture["entities"].as_array().unwrap();
        let bond = entities
            .iter()
            .find(|entity| entity["content"]["kind"] == "mutation_bond_instance")
            .expect("fixture has a mutation_bond_instance");
        let schema = entities
            .iter()
            .find(|entity| entity["content"]["kind"] == "field_schema_instance")
            .expect("fixture has a field_schema_instance");
        let projection = project_mutation_bond(&bond["content"], &schema["content"]).unwrap();
        assert_eq!(projection.command_kind, MutationCommandKind::PutEntity);
        assert_eq!(projection.content_kind, "built_position");
        assert_eq!(projection.required_fields, vec!["x", "y", "z"]);
    }
}
