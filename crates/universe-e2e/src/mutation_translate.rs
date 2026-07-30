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
}
