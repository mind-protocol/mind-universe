//! Validator for the AUTHORED House Alarm construct fixture
//! (`fixtures/ontology/lumina-prime-house-alarm-v0.json`).
//!
//! This is a STRUCTURAL well-formedness check on the portable graph projection.
//! It proves the fixture is a self-consistent `contractKind: construct` that
//! encodes the canonical construct pattern (Sensor -> DepositBond -> Threshold
//! -> Effect) and that it would intern ZERO new symbols against the canonical
//! ontology (every authored predicate has a canonical mapping; every node_type /
//! validated subtype is already canonical).
//!
//! It does NOT run the alarm. The construct is authored-but-not-runnable: the
//! physics-event -> atom-deposit bridge is unbuilt, so no real citizen-body
//! intersection can yet fire the trigger. The fixture itself declares this, and
//! this test asserts that honesty is present rather than silently absent.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;

fn fixture_path() -> PathBuf {
    // tests run with CWD = crate dir (crates/universe-e2e); the fixtures live at
    // the workspace root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/ontology/lumina-prime-house-alarm-v0.json")
}

fn load() -> Value {
    let raw = std::fs::read(fixture_path()).expect("house-alarm fixture is readable");
    serde_json::from_slice(&raw).expect("house-alarm fixture is valid JSON")
}

/// The authored -> canonical predicate remap, kept in lock-step with the
/// injector (`crates/universe-e2e/src/bin/inject_orientation_beacon.rs`). An
/// authored predicate absent from this table would mint a non-canonical symbol,
/// so its absence is a hard failure.
fn canonical_predicate(authored: &str) -> Option<&'static str> {
    Some(match authored {
        "PART_OF" => "PART_OF",
        "IMPLEMENTED_IN" => "IMPLEMENTS",
        "DEFINED_BY_CODE" => "DEFINES",
        "IMPLEMENTED_BY" => "COMPILES_TO",
        "JUSTIFIED_BY" => "GROUNDS",
        "VALIDATED_BY" => "TESTS",
        "OBSERVED_BY" => "OBSERVES",
        "PRODUCES" => "PRODUCES",
        "FEEDS" => "FEEDS",
        "SUPPORTS" => "MOTIVATES",
        _ => return None,
    })
}

/// node_type symbols and validated subtypes that the injector interns. All of
/// these already exist in the canonical seed, so a conforming fixture uses only
/// these.
const CANONICAL_NODE_TYPES: &[&str] = &["space", "narrative", "thing"];
const CANONICAL_TYPE_SUBTYPES: &[&str] = &["metric", "validation"];

fn nodes(doc: &Value) -> Vec<&Value> {
    let mut v = vec![doc];
    for m in doc["members"].as_array().expect("members array") {
        v.push(m);
    }
    v
}

#[test]
fn fixture_is_a_construct_with_a_stable_shape() {
    let doc = load();
    assert_eq!(
        doc["content"]["contractKind"].as_str(),
        Some("construct"),
        "the House Alarm must declare contractKind: construct"
    );
    assert_eq!(
        doc["node_type"].as_str(),
        Some("space"),
        "a construct is a Space"
    );
    assert_eq!(doc["subtype"].as_str(), Some("house_alarm"));
    assert!(
        doc["members"].as_array().is_some_and(|m| !m.is_empty()),
        "the construct has members"
    );
}

#[test]
fn every_node_has_a_unique_id_and_canonical_symbol() {
    let doc = load();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for node in nodes(&doc) {
        let id = node["id"].as_str().expect("node has id").to_string();
        assert!(seen.insert(id.clone()), "duplicate node id {id}");
        let node_type = node["node_type"].as_str().expect("node_type");
        assert!(
            CANONICAL_NODE_TYPES.contains(&node_type),
            "node {id} uses non-canonical node_type {node_type}"
        );
        // The injector promotes a small set of subtypes to canonical type
        // symbols; whichever symbol it would intern must already be canonical.
        let subtype = node["subtype"].as_str().unwrap_or("");
        let interned = if CANONICAL_TYPE_SUBTYPES.contains(&subtype) {
            subtype
        } else {
            node_type
        };
        assert!(
            CANONICAL_NODE_TYPES.contains(&interned) || CANONICAL_TYPE_SUBTYPES.contains(&interned),
            "node {id} would intern non-canonical symbol {interned}"
        );
    }
}

#[test]
fn every_relation_is_canonical_and_endpoints_exist() {
    let doc = load();
    let ids: BTreeSet<String> = nodes(&doc)
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect();
    let rels = doc["relations"].as_array().expect("relations array");
    assert!(!rels.is_empty(), "the construct wires its members together");
    // The only endpoint allowed to live OUTSIDE the injected member set is the
    // parent city Space: the injector partitions relations and SKIPS an edge
    // whose source or target is absent (this is how the PART_OF -> city edge is
    // dropped instead of dangling), exactly as inject_orientation_beacon does.
    const EXTERNAL_PARENT: &str = "space:l2:lumina-prime:city-v0";
    let mut intra_graph = 0usize;
    for r in rels {
        let source = r["source"].as_str().expect("relation source");
        let target = r["target"].as_str().expect("relation target");
        let authored = r["predicate"].as_str().expect("relation predicate");
        assert!(
            canonical_predicate(authored).is_some(),
            "authored predicate {authored} has no canonical mapping (would mint a new symbol)"
        );
        let source_ok = ids.contains(source) || source == EXTERNAL_PARENT;
        let target_ok = ids.contains(target) || target == EXTERNAL_PARENT;
        assert!(source_ok, "relation source {source} is neither a member nor the parent city");
        assert!(target_ok, "relation target {target} is neither a member nor the parent city");
        if ids.contains(source) && ids.contains(target) {
            intra_graph += 1;
        }
    }
    assert!(intra_graph > 0, "the construct has intra-graph relations that survive injection");
}

#[test]
fn zero_new_symbols_would_be_interned() {
    // The whole-fixture symbol budget: node symbols + canonical predicate
    // targets. All must be inside the canonical vocabulary this test knows.
    let doc = load();
    let mut requested: BTreeSet<String> = BTreeSet::new();
    for node in nodes(&doc) {
        let subtype = node["subtype"].as_str().unwrap_or("");
        let node_type = node["node_type"].as_str().unwrap();
        let sym = if CANONICAL_TYPE_SUBTYPES.contains(&subtype) {
            subtype
        } else {
            node_type
        };
        requested.insert(sym.to_string());
    }
    for r in doc["relations"].as_array().unwrap() {
        let authored = r["predicate"].as_str().unwrap();
        requested.insert(canonical_predicate(authored).unwrap().to_string());
    }
    let canonical: BTreeSet<String> = ["space", "narrative", "thing", "metric", "validation"]
        .iter()
        .map(|s| s.to_string())
        .chain(
            [
                "PART_OF", "IMPLEMENTS", "DEFINES", "COMPILES_TO", "GROUNDS", "TESTS", "OBSERVES",
                "PRODUCES", "FEEDS", "MOTIVATES",
            ]
            .iter()
            .map(|s| s.to_string()),
        )
        .collect();
    let new_symbols: Vec<&String> = requested.difference(&canonical).collect();
    assert!(
        new_symbols.is_empty(),
        "fixture would intern new symbols {new_symbols:?} (expected zero)"
    );
}

/// Locate the alarm atom circuit inside the `code:` member.
fn alarm_circuit(doc: &Value) -> Value {
    for m in doc["members"].as_array().unwrap() {
        if m["subtype"].as_str() == Some("code") {
            return m["content"]["alarm_atom_circuit"].clone();
        }
    }
    panic!("no code member with an alarm_atom_circuit");
}

#[test]
fn construct_pattern_sensor_deposit_threshold_effect_is_encoded() {
    let doc = load();

    // The construct declares the four-part pattern in prose.
    let pattern = &doc["content"]["construct_pattern"];
    for part in ["sensor", "deposit_bond", "threshold", "effect"] {
        assert!(
            pattern[part].as_str().is_some_and(|s| !s.is_empty()),
            "construct_pattern.{part} is described"
        );
    }

    let circuit = alarm_circuit(&doc);
    let atoms: BTreeMap<String, &Value> = circuit["atoms"]
        .as_array()
        .expect("atoms array")
        .iter()
        .map(|a| (a["key"].as_str().unwrap().to_string(), a))
        .collect();
    let bonds: BTreeMap<String, &Value> = circuit["bonds"]
        .as_array()
        .expect("bonds array")
        .iter()
        .map(|b| (b["key"].as_str().unwrap().to_string(), b))
        .collect();

    // SENSOR: an armed sensor atom placed in the field.
    assert!(
        atoms.contains_key("entry_sensor_armed"),
        "Sensor: entry_sensor_armed atom present"
    );

    // DEPOSIT BOND: the named Support bond that deposits +1 support onto the
    // trigger atom when a citizen body intersects the sensor.
    let deposit_key = circuit["deposit_bond"]
        .as_str()
        .expect("circuit names its deposit_bond");
    assert_eq!(deposit_key, "deposit_to_trigger");
    let deposit = bonds.get(deposit_key).expect("deposit bond exists");
    assert_eq!(
        deposit["polarity"].as_str(),
        Some("support"),
        "DepositBond is a Support BehaviorBond"
    );
    assert_eq!(
        deposit["source"].as_str(),
        Some("citizen_body_intersects"),
        "DepositBond is driven by the citizen-body intersection event"
    );
    let trigger_key = circuit["trigger_atom"].as_str().expect("names trigger atom");
    assert_eq!(deposit["target"].as_str(), Some(trigger_key));
    assert_eq!(
        deposit["energy"].as_i64(),
        Some(100),
        "DepositBond adds exactly one support (100)"
    );
    assert!(
        deposit["energy_status"]
            .as_str()
            .is_some_and(|s| s.starts_with("measured")),
        "DepositBond energy is measured, never authored/derived"
    );

    // THRESHOLD: the trigger atom fires at support >= 1 (one crossing = 100).
    let trigger = atoms.get(trigger_key).expect("trigger atom exists");
    assert_eq!(
        trigger["threshold"].as_i64(),
        Some(100),
        "Threshold fires at one measured support (support >= 1)"
    );
    assert_eq!(trigger["seed_energy"].as_i64(), Some(0), "trigger starts cold");

    // EFFECT: the fire drives an emitter that produces a 'notify' EffectIntent
    // and an EffectReceipt. Trigger -> emitter -> receipt chain present.
    assert!(
        bonds.values().any(|b| b["source"].as_str() == Some(trigger_key)
            && b["target"].as_str() == Some("notify_emitter")),
        "Effect: trigger fire drives the notify emitter"
    );
    assert!(
        bonds.values().any(|b| b["source"].as_str() == Some("notify_emitter")
            && b["target"].as_str() == Some("effect_receipt")),
        "Effect: the emitter produces an EffectReceipt"
    );

    // The effect is specifically a 'notify' EffectIntent.
    let effect_subtype = doc["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subtype"].as_str() == Some("code"))
        .and_then(|m| m["content"]["effect_intent_schema"]["subtype"].as_str());
    assert_eq!(effect_subtype, Some("notify"), "Effect is a notify EffectIntent");
}

#[test]
fn every_bond_energy_is_measured_and_a_conserved_gate() {
    // No authored / derived / unknown energy may be streamed (epistemic honesty).
    let doc = load();
    let circuit = alarm_circuit(&doc);
    for b in circuit["bonds"].as_array().unwrap() {
        let key = b["key"].as_str().unwrap();
        assert!(
            b["energy_status"]
                .as_str()
                .is_some_and(|s| s.starts_with("measured")),
            "bond {key} streams non-measured energy"
        );
        assert!(
            b["energy"].as_i64().is_some_and(|e| e > 0),
            "bond {key} has a positive integer energy"
        );
    }
}

#[test]
fn gated_atoms_declare_their_required_supports() {
    // A gate atom (threshold >= sum of its incoming supports) must list exactly
    // the bonds that feed it, so a partial deposit cannot silently fire it.
    let doc = load();
    let circuit = alarm_circuit(&doc);
    let bonds_by_target: BTreeMap<String, Vec<String>> = {
        let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for b in circuit["bonds"].as_array().unwrap() {
            m.entry(b["target"].as_str().unwrap().to_string())
                .or_default()
                .push(b["key"].as_str().unwrap().to_string());
        }
        m
    };
    for a in circuit["atoms"].as_array().unwrap() {
        if let Some(required) = a["required_supports"].as_array() {
            let key = a["key"].as_str().unwrap();
            let declared: BTreeSet<&str> =
                required.iter().map(|v| v.as_str().unwrap()).collect();
            let incoming: BTreeSet<&str> = bonds_by_target
                .get(key)
                .map(|v| v.iter().map(String::as_str).collect())
                .unwrap_or_default();
            assert_eq!(
                declared, incoming,
                "atom {key} required_supports must match its incoming bonds exactly"
            );
            // The gate threshold must equal the sum of its declared supports so
            // it fires only on full support.
            let sum: i64 = required
                .iter()
                .map(|r| {
                    let rk = r.as_str().unwrap();
                    circuit["bonds"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|b| b["key"].as_str() == Some(rk))
                        .and_then(|b| b["energy"].as_i64())
                        .unwrap()
                })
                .sum();
            assert_eq!(
                a["threshold"].as_i64(),
                Some(sum),
                "gate {key} threshold must equal the sum of its required supports"
            );
        }
    }
}

#[test]
fn fixture_is_honest_that_it_is_not_yet_runnable() {
    // The single load-bearing seam (physics-event -> atom-deposit) is unbuilt.
    // The fixture MUST say so rather than imply a working alarm.
    let doc = load();
    assert_eq!(
        doc["content"]["authoring_status"].as_str(),
        Some("AUTHORED_NOT_RUNNABLE"),
        "the construct declares it is authored but not runnable"
    );
    let code = doc["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subtype"].as_str() == Some("code"))
        .unwrap();
    assert_eq!(code["content"]["runnable"].as_bool(), Some(false));
    assert!(
        code["content"]["runnable_blocked_on"]
            .as_str()
            .is_some_and(|s| s.contains("physics-event") && s.contains("atom-deposit")),
        "the code member names the unbuilt physics-event -> atom-deposit bridge"
    );
    let impl_node = doc["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subtype"].as_str() == Some("implementation"))
        .unwrap();
    assert_eq!(impl_node["content"]["runtime_status"].as_str(), Some("not_running"));
    assert_eq!(impl_node["content"]["graph_status"].as_str(), Some("not_written"));
}
