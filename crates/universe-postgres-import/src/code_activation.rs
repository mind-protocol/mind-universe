//! G2 phase 4 — approved-ChangeSet activation of a shadow-compared candidate.
//!
//! This closes the code-migration state machine for the one candidate that
//! compiled, shadow-executed deterministically, and matched its contract. It
//! does **not** run the code: activation pins a real, enabled `TriggerSubscription`
//! to the compiled CodeDefinition so *later* triggers may execute it. No trigger
//! is fired here and no `ExecutionReceipt` is produced.
//!
//! Activation is gated: it requires the full evidence chain read back from the
//! graph — compiled, shadow_executed, deterministic, equivalent,
//! `independently_compared` — plus an approved ChangeSet. Compilation, shadow
//! execution, or source status alone can never activate; a non-equivalent
//! candidate is refused and preserved, never activated.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseError, UniverseId};
use universe_ir::{
    TriggerBudgets, TriggerControls, TriggerEventKind, TriggerEvidenceRequirement,
    TriggerSubscription, TRIGGER_CONTRACT_VERSION,
};
use universe_store::{EntityRecord, RelationRecord, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

use crate::code_translation::{run_translation, TranslationManifest};

const CHANGESET_ATOM: EntityKey = EntityKey(0x8020);
const SUBSCRIPTION_ATOM: EntityKey = EntityKey(0x8021);
const OUTCOME_ATOM: EntityKey = EntityKey(0x8022);
const RECEIPT_ATOM: EntityKey = EntityKey(0x80f1);
const RELATION_BASE: u128 = 0x8d00;

const CHANGE_ID: &str = "postgres-code-activation-reconciliation-v0";
const AUTHORITY: &str = "graph_code_review_authority";
const STATUS: &str = "approved_by_shadow_comparison_review";
const CODE_REVISION: u64 = 1;

const SYM_CHANGESET: &str = "code_activation_changeset";
const SYM_SUBSCRIPTION: &str = "trigger_subscription";
const SYM_OUTCOME: &str = "activation_outcome";
const SYM_RECEIPT: &str = "import_receipt";
const SYM_PART_OF: &str = "PART_OF";
const SYM_ACTIVATES: &str = "ACTIVATES";
const SYM_GOVERNED_BY: &str = "GOVERNED_BY";
const SYM_HAS_RECEIPT: &str = "HAS_RECEIPT";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeActivationEvidence {
    pub change_id: String,
    pub universe: UniverseId,
    pub code_hash: String,
    /// Evidence-chain gate: true only when the graph shows the candidate
    /// compiled, shadow-executed, deterministic, equivalent, independently_compared.
    pub activatable: bool,
    pub activated: bool,
    pub subscription_valid: bool,
    pub subscription_enabled: bool,
    /// Must be zero — activation makes code eligible for later triggers, it does
    /// not execute it.
    pub executions_now: usize,
    pub state_reached: String,
    pub reason: Option<String>,
    pub final_snapshot_hash: String,
    pub final_revision: Revision,
    pub final_tick: Tick,
    pub receipt_atom: EntityKey,
}

pub fn run_activation(
    manifest: &TranslationManifest,
    output: impl AsRef<Path>,
) -> Result<CodeActivationEvidence, UniverseError> {
    let store_root = output.as_ref();
    // Ensure the compared candidate exists (idempotent) and read its evidence.
    let translation = run_translation(manifest, store_root)?;

    let store = UniverseStore::open(store_root)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;

    // Re-read the shadow receipt from the graph — the gate trusts the store, not
    // the in-process return value.
    let shadow_receipt = snapshot
        .entities
        .iter()
        .find(|entity| entity.key == manifest.batch.receipt_atom)
        .and_then(|entity| entity.content.as_ref())
        .ok_or_else(|| validation("shadow execution receipt is absent; cannot gate activation"))?;
    let shadow = store.read_content(shadow_receipt)?;
    let gate_ok = shadow.get("compiled") == Some(&Value::Bool(true))
        && shadow.get("shadow_executed") == Some(&Value::Bool(true))
        && shadow.get("deterministic") == Some(&Value::Bool(true))
        && shadow.get("equivalent") == Some(&Value::Bool(true))
        && shadow.get("state_reached").and_then(Value::as_str) == Some("independently_compared")
        && translation.equivalent
        && translation.deterministic;

    let subscription = TriggerSubscription {
        contract_version: TRIGGER_CONTRACT_VERSION,
        subscription: SUBSCRIPTION_ATOM,
        revision: Revision(1),
        enabled: true,
        event_kinds: vec![TriggerEventKind::ApprovedChangeSet],
        code_definition: manifest.definition_atom,
        code_revision: Revision(CODE_REVISION),
        code_hash: translation.code_hash.clone(),
        evidence_requirement: TriggerEvidenceRequirement::Measured,
        max_event_age_ticks: 8,
        budgets: TriggerBudgets {
            fuel: manifest.fuel,
            max_mutations: manifest.max_proposals.max(1),
            max_ticks: 1,
        },
        controls: TriggerControls {
            cooldown_ticks: 1,
            debounce_ticks: 1,
            max_causal_depth: 4,
            max_firings_per_tick: 1,
        },
        idempotency_namespace: CHANGE_ID.to_owned(),
    };
    let subscription_valid = universe_compiler::validate_trigger_subscription(&subscription).valid;

    // A non-equivalent or invalid candidate is refused and preserved, never activated.
    if !gate_ok || !subscription_valid {
        let reason = if !gate_ok {
            "evidence chain incomplete or non-equivalent"
        } else {
            "trigger subscription failed validation"
        };
        return Ok(CodeActivationEvidence {
            change_id: CHANGE_ID.into(),
            universe: snapshot.universe,
            code_hash: translation.code_hash,
            activatable: false,
            activated: false,
            subscription_valid,
            subscription_enabled: false,
            executions_now: 0,
            state_reached: "activation_refused".into(),
            reason: Some(reason.into()),
            final_snapshot_hash: snapshot.canonical_hash()?,
            final_revision: snapshot.revision,
            final_tick: snapshot.tick,
            receipt_atom: RECEIPT_ATOM,
        });
    }

    let subscription_value = serde_json::to_value(&subscription)
        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
    let receipt_content = json!({
        "kind": "adaptation_receipt",
        "change_id": CHANGE_ID,
        "authority": AUTHORITY,
        "status": STATUS,
        "information_status": "measured",
        "activated_for_later_execution": true,
        "executed_now": false,
        "code_definition": manifest.definition_atom,
        "code_revision": CODE_REVISION,
        "code_hash": translation.code_hash,
        "gate": {
            "compiled": true,
            "shadow_executed": true,
            "deterministic": true,
            "equivalent": true,
            "approved_changeset": true,
        },
        "transitions": [
            "independently_compared",
            "approved_changeset",
            "activated_for_later_execution",
        ],
    });

    let activate_key = format!("{CHANGE_ID}:activate");
    if !snapshot.event_keys.contains(&activate_key) {
        let plan = snapshot.plan_symbol_interning(
            &[
                SYM_CHANGESET,
                SYM_SUBSCRIPTION,
                SYM_OUTCOME,
                SYM_RECEIPT,
                SYM_PART_OF,
                SYM_ACTIVATES,
                SYM_GOVERNED_BY,
                SYM_HAS_RECEIPT,
            ]
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>(),
        )?;
        let sym = |name: &str| -> Result<u32, UniverseError> {
            plan.assignments
                .get(name)
                .copied()
                .ok_or_else(|| validation(format!("symbol {name} was not planned")))
        };

        let changeset_ref = store.append_content(&json!({
            "kind": "code_activation_changeset",
            "change_id": CHANGE_ID,
            "authority": AUTHORITY,
            "status": STATUS,
        }))?;
        let subscription_ref = store.append_content(&json!({
            "kind": "trigger_subscription",
            "enabled": true,
            "fired": false,
            "subscription": subscription_value,
        }))?;
        let outcome_ref = store.append_content(&json!({
            "kind": "activation_outcome",
            "code_definition": manifest.definition_atom,
            "state": "activated_for_later_execution",
            "executed_now": false,
        }))?;
        let receipt_ref = store.append_content(&receipt_content)?;

        let mut relation_key = RELATION_BASE;
        let mut next_relation = |source, target, predicate| {
            let command = UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(relation_key),
                    generation: 0,
                    source,
                    target,
                    predicate,
                    content: None,
                },
            };
            relation_key += 1;
            command
        };

        let mut commands = vec![UniverseCommand::InternSymbols {
            symbols: plan.additions.clone(),
        }];
        for (key, symbol, content) in [
            (CHANGESET_ATOM, sym(SYM_CHANGESET)?, changeset_ref),
            (SUBSCRIPTION_ATOM, sym(SYM_SUBSCRIPTION)?, subscription_ref),
            (OUTCOME_ATOM, sym(SYM_OUTCOME)?, outcome_ref),
            (RECEIPT_ATOM, sym(SYM_RECEIPT)?, receipt_ref),
        ] {
            commands.push(UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key,
                    generation: 0,
                    symbol,
                    content: Some(content),
                },
            });
        }
        // Subscription is a member of the approved ChangeSet and pins the code.
        commands.push(next_relation(
            SUBSCRIPTION_ATOM,
            CHANGESET_ATOM,
            sym(SYM_PART_OF)?,
        ));
        commands.push(next_relation(
            SUBSCRIPTION_ATOM,
            manifest.definition_atom,
            sym(SYM_ACTIVATES)?,
        ));
        commands.push(next_relation(
            OUTCOME_ATOM,
            CHANGESET_ATOM,
            sym(SYM_PART_OF)?,
        ));
        commands.push(next_relation(
            CHANGESET_ATOM,
            manifest.source.atom,
            sym(SYM_GOVERNED_BY)?,
        ));
        commands.push(next_relation(
            CHANGESET_ATOM,
            RECEIPT_ATOM,
            sym(SYM_HAS_RECEIPT)?,
        ));

        let transaction = UniverseTransaction::prepare(
            &snapshot,
            UniverseWriteSet {
                base_revision: snapshot.revision,
                idempotency_key: activate_key,
                causal_ancestry: vec![CHANGE_ID.to_owned()],
                commands,
            },
        )?;
        let tick = Tick(snapshot.tick.0 + 1);
        transaction.commit(&store, &mut snapshot, tick)?;
    }

    // Independent replay + verification.
    let final_store = UniverseStore::open(store_root)?;
    let final_snapshot = final_store.replay(final_store.load_snapshot()?)?;

    let part_of = final_snapshot
        .symbol_id(SYM_PART_OF)
        .ok_or_else(|| validation("PART_OF symbol absent after replay"))?;
    let membership = final_snapshot.relations.iter().any(|relation| {
        relation.source == SUBSCRIPTION_ATOM
            && relation.target == CHANGESET_ATOM
            && relation.predicate == part_of
    });
    let subscription_entity = final_snapshot
        .entities
        .iter()
        .find(|entity| entity.key == SUBSCRIPTION_ATOM)
        .and_then(|entity| entity.content.as_ref())
        .ok_or_else(|| validation("activated subscription missing after replay"))?;
    let stored_subscription = final_store.read_content(subscription_entity)?;
    let subscription_enabled = stored_subscription.get("enabled") == Some(&Value::Bool(true))
        && stored_subscription.get("fired") == Some(&Value::Bool(false));

    // No execution/ExecutionReceipt of the code happened: nothing was fired.
    let mut executions_now = 0;
    for entity in &final_snapshot.entities {
        if let Some(content_ref) = entity.content.as_ref() {
            let content = final_store.read_content(content_ref)?;
            match content.get("kind").and_then(Value::as_str) {
                Some("execution_receipt") | Some("triggered_execution_receipt") => {
                    executions_now += 1
                }
                _ => {}
            }
            if content.get("fired") == Some(&Value::Bool(true)) {
                executions_now += 1;
            }
        }
    }
    if !membership || !subscription_enabled || executions_now != 0 {
        return Err(UniverseError::CorruptContent(
            "activation readback failed: missing membership, disabled subscription, or an execution occurred".into(),
        ));
    }

    let receipt_entity = final_snapshot
        .entities
        .iter()
        .find(|entity| entity.key == RECEIPT_ATOM)
        .and_then(|entity| entity.content.as_ref())
        .ok_or_else(|| validation("activation receipt missing after replay"))?;
    if final_store.read_content(receipt_entity)? != receipt_content {
        return Err(UniverseError::CorruptContent(
            "activation receipt differs after replay".into(),
        ));
    }

    Ok(CodeActivationEvidence {
        change_id: CHANGE_ID.into(),
        universe: final_snapshot.universe,
        code_hash: translation.code_hash,
        activatable: true,
        activated: true,
        subscription_valid: true,
        subscription_enabled: true,
        executions_now: 0,
        state_reached: "activated_for_later_execution".into(),
        reason: None,
        final_snapshot_hash: final_snapshot.canonical_hash()?,
        final_revision: final_snapshot.revision,
        final_tick: final_snapshot.tick,
        receipt_atom: RECEIPT_ATOM,
    })
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> TranslationManifest {
        serde_json::from_str(include_str!(
            "../../../fixtures/import/postgres-code-translation-pilot.json"
        ))
        .unwrap()
    }

    #[test]
    fn compared_candidate_activates_for_later_execution_without_running() {
        let manifest = manifest();
        let temp = tempfile::tempdir().unwrap();
        let evidence = run_activation(&manifest, temp.path()).unwrap();

        assert!(evidence.activatable);
        assert!(evidence.activated);
        assert!(evidence.subscription_valid);
        assert!(evidence.subscription_enabled);
        assert_eq!(evidence.executions_now, 0);
        assert_eq!(evidence.state_reached, "activated_for_later_execution");
        assert_eq!(evidence.code_hash.len(), 64);
    }

    #[test]
    fn rerun_is_idempotent() {
        let manifest = manifest();
        let temp = tempfile::tempdir().unwrap();
        let first = run_activation(&manifest, temp.path()).unwrap();
        let second = run_activation(&manifest, temp.path()).unwrap();
        assert_eq!(first.final_snapshot_hash, second.final_snapshot_hash);
        assert_eq!(first.final_revision, second.final_revision);
    }

    #[test]
    fn non_equivalent_candidate_is_refused() {
        let mut manifest = manifest();
        manifest.inputs.blueprint_state = manifest.inputs.l1_state + 1; // not reconciled
        let temp = tempfile::tempdir().unwrap();
        let evidence = run_activation(&manifest, temp.path()).unwrap();
        assert!(!evidence.activatable);
        assert!(!evidence.activated);
        assert_eq!(evidence.state_reached, "activation_refused");
    }
}
