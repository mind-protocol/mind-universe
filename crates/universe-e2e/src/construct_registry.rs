//! Resolve constructs FROM THE COMMITTED GRAPH, not the fixture file.
//!
//! `construct_resolver` (Phase 1) turns an authored `alarm_atom_circuit` block
//! into the runtime `PhysicsWaveInputs` the physics floor consumes. But its
//! driver (`bin/house_alarm_resolved`) reads that block from the fixture JSON on
//! disk. This module closes the honesty gap: it reads the circuit from a
//! construct's COMMITTED graph content — the `code` node's durable content,
//! hydrated via [`Supervisor::read_content`] — so the physics that runs is the
//! physics the store actually holds, not a re-read of the authoring file.
//!
//! It is a pure BOUNDED read path: it hydrates only the entities that carry a
//! content ref, up to a caller-supplied ceiling (the scratch store is small; a
//! whole live universe is never scanned). It commits nothing, mutates nothing,
//! and mints zero symbols. The circuit -> runtime-inputs transform is entirely
//! Phase 1's `resolve_construct`; this module only locates and hydrates.

use serde_json::Value;

use universe_core::EntityKey;
use universe_store::UniverseSnapshot;
use universe_supervisor::Supervisor;

use crate::construct_resolver::{resolve_construct, AlarmAtomCircuit, ResolvedConstruct};
use crate::E2eError;

/// A construct resolved from committed graph content, tagged with the
/// [`EntityKey`] of the `code` node whose durable content furnished its circuit.
/// The key doubles as the construct's stable id in the wake-queue.
#[derive(Clone, Debug)]
pub struct RegisteredConstruct {
    /// The committed `code` node whose content carried the `alarm_atom_circuit`.
    pub code_node: EntityKey,
    /// The runtime physics resolved from that committed content.
    pub resolved: ResolvedConstruct,
}

/// Locate the `alarm_atom_circuit` inside a hydrated content value. The injector
/// wraps a member's authored content as `{canonical_id, node_type, subtype,
/// content}` so the committed node exposes the circuit at
/// `/content/alarm_atom_circuit`; a raw portable projection exposes it directly
/// at `alarm_atom_circuit`. Both are accepted, injected wrapping first.
fn circuit_value(content: &Value) -> Option<&Value> {
    content
        .pointer("/content/alarm_atom_circuit")
        .or_else(|| content.get("alarm_atom_circuit"))
}

/// Resolve a construct's runtime physics from a committed node's hydrated content
/// value.
///
/// The `circuit_json` is the content a downstream reader gets back from
/// [`Supervisor::read_content`]; the `alarm_atom_circuit` block is extracted,
/// deserialized into an [`AlarmAtomCircuit`], and run through Phase 1's
/// [`resolve_construct`]. The circuit is graph-native: it comes from the store's
/// committed content, never a re-read of the fixture file.
pub fn resolve_construct_from_content(circuit_json: &Value) -> Result<ResolvedConstruct, E2eError> {
    let circuit_value = circuit_value(circuit_json).ok_or_else(|| {
        E2eError::Contract("committed node content carries no alarm_atom_circuit".into())
    })?;
    let circuit: AlarmAtomCircuit =
        serde_json::from_value(circuit_value.clone()).map_err(|error| {
            E2eError::Contract(format!("committed alarm_atom_circuit did not deserialize: {error}"))
        })?;
    resolve_construct(&circuit)
}

/// Find the construct in a committed snapshot and resolve its runtime physics.
///
/// Walks the snapshot's entities, hydrating the content of each entity that
/// carries a content ref (via [`Supervisor::read_content`]) and selecting the
/// FIRST whose content contains an `alarm_atom_circuit`. Hydration is BOUNDED by
/// `max_hydrations`: exceeding it before a circuit is found is a hard error, not
/// a silent whole-store scan. Returns the resolved construct plus its `code`
/// node key (its stable wake-queue id).
pub fn find_construct_in_snapshot(
    supervisor: &Supervisor,
    snapshot: &UniverseSnapshot,
    max_hydrations: usize,
) -> Result<RegisteredConstruct, E2eError> {
    let mut hydrations = 0usize;
    for entity in &snapshot.entities {
        let Some(content_ref) = entity.content.as_ref() else {
            continue;
        };
        if hydrations >= max_hydrations {
            return Err(E2eError::Contract(format!(
                "hydration budget {max_hydrations} exhausted before an alarm_atom_circuit was found"
            )));
        }
        hydrations += 1;
        let content = supervisor.read_content(content_ref)?;
        if circuit_value(&content).is_none() {
            continue;
        }
        let resolved = resolve_construct_from_content(&content)?;
        return Ok(RegisteredConstruct {
            code_node: entity.key,
            resolved,
        });
    }
    Err(E2eError::Contract(
        "no committed node carries an alarm_atom_circuit".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but complete authored circuit, as JSON, matching the
    /// `AlarmAtomCircuit` schema: two atoms, a deposit edge and a conduction edge,
    /// one terminal effect binding.
    fn circuit_json() -> Value {
        serde_json::json!({
            "atoms": [
                { "key": "sensor", "threshold": 0 },
                { "key": "trig", "threshold": 100 },
                { "key": "emit", "threshold": 100 }
            ],
            "bonds": [
                { "key": "deposit", "source": "sensor", "target": "trig",
                  "polarity": "support", "energy": 100 },
                { "key": "trig_to_emit", "source": "trig", "target": "emit",
                  "polarity": "support", "energy": 100 }
            ],
            "deposit_bond": "deposit",
            "trigger_atom": "trig",
            "effect_bindings": [
                { "emitter_atom": "emit", "capability": "safe.notify",
                  "idempotency_key": "test:notify", "message": "hi", "deadline_tick": 500 }
            ],
            "external_measured_injections": { "sensor": 100 }
        })
    }

    /// The circuit is resolvable both under the injected `/content/...` wrapping
    /// and directly at the top level — the two shapes a committed node vs a raw
    /// projection expose.
    #[test]
    fn resolves_under_injected_wrapping_and_raw() {
        let wrapped = serde_json::json!({
            "canonical_id": "code:test",
            "node_type": "code",
            "subtype": "",
            "content": { "alarm_atom_circuit": circuit_json() }
        });
        let resolved = resolve_construct_from_content(&wrapped).expect("wrapped resolves");
        // Split: sensor half is {sensor}; construct half is {trig, emit}; one deposit.
        assert_eq!(resolved.sensor_cluster.atoms.len(), 1);
        assert_eq!(resolved.construct_cluster.atoms.len(), 2);
        assert_eq!(resolved.deposit_bindings.len(), 1);
        assert_eq!(resolved.effect_bindings.len(), 1);

        let raw = serde_json::json!({ "alarm_atom_circuit": circuit_json() });
        let from_raw = resolve_construct_from_content(&raw).expect("raw resolves");
        assert_eq!(from_raw.construct_cluster.atoms.len(), 2);
    }

    /// Content with no `alarm_atom_circuit` is a clean contract error, never a panic.
    #[test]
    fn missing_circuit_is_a_contract_error() {
        let empty = serde_json::json!({ "content": { "something_else": true } });
        assert!(matches!(
            resolve_construct_from_content(&empty),
            Err(E2eError::Contract(_))
        ));
    }
}
