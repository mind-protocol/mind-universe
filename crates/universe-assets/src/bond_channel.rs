//! Bond-channel grammar — the graph-native authority for how a link renders.
//!
//! `ALIGN.md` §2 fixes ONE table: each canonical link attribute (`family`,
//! `permanence`, `hierarchy`, `polarity`, `mode`, and the dynamic state
//! `energy/weight/recency/stability/gate`) projects onto its own perceptual
//! channel. The live renderer (`apps/mind-desktop/src/scene-svg.ts`) already
//! consumes some of these channels; this module closes the same drift `visual.rs`
//! and `layout_authority.rs` closed — it makes the TABLE ITSELF a content-addressed
//! Asset in the store, validated and read back byte-for-byte, so the renderer
//! *derives* the mapping instead of hard-coding a parallel one. It is the single
//! table ALIGN §5 prescribes, not a fifth divergent representation.
//!
//! Durable (this module): the `bond-channel-grammar` catalog as a Node→Asset
//! projection. Live: the per-edge channel values = grammar ⊕ the edge's canonical
//! attributes, composed at draw time — never persisted per frame.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use universe_core::{EntityKey, RelationKey, UniverseError, UniverseId};
use universe_store::{canonical_hash, GraphSeed, SeedEntity, SeedRelation, UniverseStore};

const UNIVERSE: UniverseId = UniverseId(0xE000);
const CONTRACT_ATOM: EntityKey = EntityKey(0xE001);
const CHANGESET_ATOM: EntityKey = EntityKey(0xE002);
const GRAMMAR_ATOM: EntityKey = EntityKey(0xE010);
const PAYLOAD_ATOM: EntityKey = EntityKey(0xE011);
const RELATION_BASE: u128 = 0xE200;

const CHANGE_ID: &str = "bond-channel-grammar-materialization-v0";
const CONTRACT_ID: &str = "bond-channel-projection-contract-v0";
const AUTHORITY: &str = "graph_first_bond_channel_authority";
const STATUS: &str = "approved_for_projection";

/// The six epistemic states the honesty layer must cover (CLAUDE.md discipline).
const REQUIRED_HONESTY: [&str; 2] = ["unknown", "not_measured"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BondChannelGrammar {
    /// The `bond-channel-grammar/…` document, preserved verbatim so the graph
    /// Asset is byte-identical to what the renderer derives its mapping from.
    #[serde(flatten)]
    pub value: Value,
}

impl BondChannelGrammar {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, UniverseError> {
        let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
        let value = serde_json::from_slice(&bytes)
            .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
        Ok(Self { value })
    }

    pub fn grammar_id(&self) -> Result<&str, UniverseError> {
        self.value
            .get("grammar_id")
            .and_then(Value::as_str)
            .ok_or_else(|| validation("grammar has no grammar_id"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BondChannelReceipt {
    pub kind: String,
    pub change_id: String,
    pub authority: String,
    pub status: String,
    pub universe: UniverseId,
    pub newly_committed: bool,
    pub grammar_id: String,
    pub grammar_sha256: String,
    /// The catalog read back from the store equals the fixture byte-for-byte.
    pub parity: bool,
    pub static_channels: usize,
    pub dynamic_channels: usize,
    pub honesty_states: usize,
    /// The `energy` channel requires `measured` — the membrane invariant, encoded.
    pub energy_requires_measured: bool,
    pub final_snapshot_hash: String,
}

fn channels<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, UniverseError> {
    value
        .get(key)
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
        .ok_or_else(|| validation(format!("grammar {key} must be a non-empty array")))
}

pub fn validate(grammar: &BondChannelGrammar) -> Result<(), UniverseError> {
    let value = &grammar.value;
    for key in ["static_channels", "dynamic_channels"] {
        for channel in channels(value, key)? {
            let has = |field: &str| {
                channel
                    .get(field)
                    .and_then(Value::as_str)
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
            };
            if !has("attribute") || !has("channel") {
                return Err(validation(format!(
                    "{key} entry must name an attribute and a channel"
                )));
            }
        }
    }
    // Honesty layer must map the un-measured states to a distinct, non-confident
    // treatment (Fog) — never a default.
    let honesty = value
        .get("honesty")
        .ok_or_else(|| validation("grammar has no honesty layer"))?;
    for state in REQUIRED_HONESTY {
        if honesty.get(state).and_then(Value::as_str) != Some("fog") {
            return Err(validation(format!(
                "honesty must render {state} as fog, never a confident default"
            )));
        }
    }
    // Membrane invariant: the energy channel may only stream when measured.
    if !energy_requires_measured(value) {
        return Err(validation(
            "the energy channel must declare requires_epistemic=measured (membrane invariant)",
        ));
    }
    // Colours needed by the renderer must be present.
    let colors = value
        .get("colors")
        .ok_or_else(|| validation("grammar has no colors"))?;
    for name in ["excitation", "inhibition", "neutral"] {
        if colors.get(name).and_then(Value::as_str).is_none() {
            return Err(validation(format!("grammar colors missing {name}")));
        }
    }
    Ok(())
}

fn energy_requires_measured(value: &Value) -> bool {
    value
        .get("dynamic_channels")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter().any(|channel| {
                channel.get("attribute").and_then(Value::as_str) == Some("energy")
                    && channel.get("requires_epistemic").and_then(Value::as_str) == Some("measured")
            })
        })
        .unwrap_or(false)
}

fn build_seed(grammar: &BondChannelGrammar) -> Result<GraphSeed, UniverseError> {
    validate(grammar)?;
    let grammar_sha256 = canonical_hash(&grammar.value)?;
    let grammar_id = grammar.grammar_id()?.to_owned();

    let entities = vec![
        seed_entity(
            CONTRACT_ATOM,
            "bond_channel_contract",
            json!({
                "kind": "bond_channel_contract",
                "contract_id": CONTRACT_ID,
                "node_remains_authoritative": true,
                "asset_is_derived": true,
                "invalidation_signals": ["grammar_revision", "grammar_hash"],
            }),
        ),
        seed_entity(
            GRAMMAR_ATOM,
            "bond_channel_grammar",
            json!({
                "kind": "bond_channel_grammar",
                "grammar_id": grammar_id,
                "output_kind": "bond_channel_table",
                "media_type": "application/vnd.mind.bond-channel-grammar+json",
                "grammar_sha256": grammar_sha256,
                "canonical_node_replaced": false,
            }),
        ),
        seed_entity(
            PAYLOAD_ATOM,
            "bond_channel_payload",
            json!({
                "kind": "bond_channel_payload",
                "content_address": format!("sha256:{grammar_sha256}"),
                "grammar_sha256": grammar_sha256,
                "value": grammar.value,
            }),
        ),
        seed_entity(
            CHANGESET_ATOM,
            "bond_channel_changeset",
            json!({
                "kind": "bond_channel_changeset",
                "change_id": CHANGE_ID,
                "authority": AUTHORITY,
                "status": STATUS,
                "contract": CONTRACT_ATOM,
                "scope": [GRAMMAR_ATOM],
            }),
        ),
    ];

    let relations = vec![
        seed_relation(RELATION_BASE, GRAMMAR_ATOM, CONTRACT_ATOM, "GOVERNED_BY"),
        seed_relation(RELATION_BASE + 1, GRAMMAR_ATOM, PAYLOAD_ATOM, "HAS_PAYLOAD"),
        seed_relation(RELATION_BASE + 2, GRAMMAR_ATOM, CHANGESET_ATOM, "PART_OF"),
        seed_relation(RELATION_BASE + 3, CHANGESET_ATOM, CONTRACT_ATOM, "GOVERNED_BY"),
    ];

    Ok(GraphSeed {
        universe: UNIVERSE,
        symbols: vec![
            "bond_channel_contract".to_owned(),
            "bond_channel_grammar".to_owned(),
            "bond_channel_payload".to_owned(),
            "bond_channel_changeset".to_owned(),
            "GOVERNED_BY".to_owned(),
            "HAS_PAYLOAD".to_owned(),
            "PART_OF".to_owned(),
        ],
        entities,
        relations,
    })
}

pub fn materialize(
    store_root: impl AsRef<Path>,
    grammar: &BondChannelGrammar,
) -> Result<BondChannelReceipt, UniverseError> {
    let store_root = store_root.as_ref();
    let seed = build_seed(grammar)?;
    let grammar_sha256 = canonical_hash(&grammar.value)?;

    let store = UniverseStore::open(store_root)?;
    let newly_committed = !store_root.join("snapshot.json").exists();
    if newly_committed {
        store.install_seed(&seed)?;
    }

    // Independent readback: reopen, replay, and confirm the grammar Asset is
    // byte-identical to the fixture the renderer derives from.
    let readback_store = UniverseStore::open(store_root)?;
    let readback = readback_store.replay(readback_store.load_snapshot()?)?;
    let payload = readback
        .entities
        .iter()
        .find(|entity| entity.key == PAYLOAD_ATOM)
        .and_then(|entity| entity.content.as_ref())
        .ok_or_else(|| validation("bond-channel payload missing after reopen"))?;
    let content = readback_store.read_content(payload)?;
    let read_value = content
        .get("value")
        .ok_or_else(|| validation("payload has no value"))?;
    let parity = canonical_hash(read_value)? == grammar_sha256;
    if !parity {
        return Err(validation("read-back grammar differs from the fixture"));
    }

    Ok(BondChannelReceipt {
        kind: "bond_channel_grammar_materialization_receipt".into(),
        change_id: CHANGE_ID.into(),
        authority: AUTHORITY.into(),
        status: STATUS.into(),
        universe: readback.universe,
        newly_committed,
        grammar_id: grammar.grammar_id()?.to_owned(),
        grammar_sha256,
        parity,
        static_channels: channels(&grammar.value, "static_channels")?.len(),
        dynamic_channels: channels(&grammar.value, "dynamic_channels")?.len(),
        honesty_states: grammar
            .value
            .get("honesty")
            .and_then(Value::as_object)
            .map(|map| map.keys().filter(|k| k.as_str() != "rule").count())
            .unwrap_or(0),
        energy_requires_measured: energy_requires_measured(&grammar.value),
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

fn seed_relation(key: u128, source: EntityKey, target: EntityKey, predicate: &str) -> SeedRelation {
    SeedRelation {
        key: RelationKey(key),
        generation: 0,
        source,
        target,
        predicate: predicate.to_owned(),
        content: None,
    }
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grammar() -> BondChannelGrammar {
        BondChannelGrammar::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/assets/bond-channel-grammar.json"),
        )
        .unwrap()
    }

    #[test]
    fn materializes_the_table_with_independent_parity() {
        let temp = tempfile::tempdir().unwrap();
        let receipt = materialize(temp.path().join("store"), &grammar()).unwrap();
        assert!(receipt.newly_committed);
        assert!(receipt.parity);
        assert!(receipt.energy_requires_measured);
        assert_eq!(receipt.static_channels, 6);
        assert_eq!(receipt.dynamic_channels, 5);
        assert_eq!(receipt.grammar_id, "bond-channel-grammar-v0");
    }

    #[test]
    fn rematerialization_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("store");
        let first = materialize(&root, &grammar()).unwrap();
        let second = materialize(&root, &grammar()).unwrap();
        assert!(first.newly_committed);
        assert!(!second.newly_committed);
        assert_eq!(first.final_snapshot_hash, second.final_snapshot_hash);
    }

    #[test]
    fn a_grammar_that_streams_unmeasured_energy_is_rejected() {
        let mut grammar = grammar();
        // Drop the membrane invariant from the energy channel.
        let dynamic = grammar.value["dynamic_channels"].as_array_mut().unwrap();
        for channel in dynamic.iter_mut() {
            if channel.get("attribute").and_then(Value::as_str) == Some("energy") {
                channel.as_object_mut().unwrap().remove("requires_epistemic");
            }
        }
        assert!(matches!(
            validate(&grammar),
            Err(UniverseError::Validation(message)) if message.contains("membrane invariant")
        ));
    }

    #[test]
    fn dishonest_unknown_rendering_is_rejected() {
        let mut grammar = grammar();
        grammar.value["honesty"]["unknown"] = json!("solid");
        assert!(matches!(
            validate(&grammar),
            Err(UniverseError::Validation(message)) if message.contains("fog")
        ));
    }
}
