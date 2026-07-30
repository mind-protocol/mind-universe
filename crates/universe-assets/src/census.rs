//! Read-only Node→Asset conversion census over an existing Universe store.
//!
//! The inventory in [`crate::inventory`] proves the conversion mechanism against
//! a synthetic pilot universe. The census applies the same conversion-class
//! vocabulary to a *real* corpus — e.g. the canonical ontology's 231 Nodes —
//! without mutating that authority. It reads each Node, resolves its
//! `asset_requirement` through an explicit, graph-declared [`CensusPolicy`], and
//! derives its conversion class. The census never invents a requirement: any
//! kind the policy does not cover stays `unknown`, and any Node whose class
//! cannot be measured stays `unknown` rather than defaulting to a convenient
//! value.
//!
//! The census is a *measurement*, not a ChangeSet. Classifying the canonical
//! Nodes into Assets would require an approved overlay against the ontology
//! manifest; this module only observes and reports, mirroring the read-only
//! reconstruction receipts produced elsewhere.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};
use universe_core::{EntityKey, UniverseError, UniverseId};
use universe_store::{canonical_hash, UniverseSnapshot, UniverseStore};

/// Conversion classes required by G1. `converted`/`partial`/`blocked` describe a
/// Node whose Asset is (respectively) current, present-but-imperfect, or absent;
/// `intentionally_assetless` is a Node that declares it needs no Asset;
/// `unknown` preserves un-measured or undeclared conversion status.
const CLASSES: [&str; 5] = [
    "converted",
    "partial",
    "blocked",
    "intentionally_assetless",
    "unknown",
];
/// The only requirement values a policy rule may assert.
const REQUIREMENTS: [&str; 3] = ["required", "intentionally_assetless", "unknown"];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequirementRule {
    /// Content `kind`, or `"ontology_definition:<definition_kind>"` for the six
    /// definition sub-kinds, matched exactly against the resolved Node kind.
    pub kind: String,
    pub requirement: String,
    /// Why this kind carries this requirement — an explicit, reviewable claim,
    /// never a silent default.
    pub justification: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CensusPolicy {
    pub policy_id: String,
    /// Requirement assigned to any Node kind not named by a rule. Must be
    /// `unknown` so unmapped Nodes preserve their un-measured status.
    pub default_requirement: String,
    pub rules: Vec<RequirementRule>,
}

impl CensusPolicy {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, UniverseError> {
        let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| UniverseError::CorruptContent(error.to_string()))
    }

    fn validate(&self) -> Result<(), UniverseError> {
        if self.default_requirement != "unknown" {
            return Err(validation(
                "census default requirement must be unknown to preserve un-measured status",
            ));
        }
        let mut kinds = std::collections::BTreeSet::new();
        for rule in &self.rules {
            if !REQUIREMENTS.contains(&rule.requirement.as_str()) {
                return Err(validation(format!(
                    "census rule for {} asserts unsupported requirement {}",
                    rule.kind, rule.requirement
                )));
            }
            if rule.justification.trim().is_empty() {
                return Err(validation(format!(
                    "census rule for {} is not justified",
                    rule.kind
                )));
            }
            if !kinds.insert(rule.kind.clone()) {
                return Err(validation(format!(
                    "census rule for {} is duplicated",
                    rule.kind
                )));
            }
        }
        Ok(())
    }

    pub fn requirement_for(&self, kind: &str) -> &str {
        self.rules
            .iter()
            .find(|rule| rule.kind == kind)
            .map(|rule| rule.requirement.as_str())
            .unwrap_or(self.default_requirement.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NodeClassification {
    pub entity: EntityKey,
    pub kind: String,
    pub requirement: String,
    pub has_current_asset: bool,
    pub conversion_class: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CensusReceipt {
    pub kind: String,
    pub policy_id: String,
    pub policy_hash: String,
    pub universe: UniverseId,
    pub snapshot_hash: String,
    pub total_nodes: usize,
    pub kind_counts: BTreeMap<String, usize>,
    pub requirement_counts: BTreeMap<String, usize>,
    pub class_counts: BTreeMap<String, usize>,
    pub classifications: Vec<NodeClassification>,
    pub epistemic_state: String,
}

/// Resolves a Node's classification kind: the content `kind`, refined to
/// `ontology_definition:<definition_kind>` for ontology definitions so the six
/// definition sub-kinds can carry distinct requirements.
fn node_kind(content: &Value) -> String {
    let kind = content
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("<no-kind>");
    if kind == "ontology_definition" {
        if let Some(sub) = content.get("definition_kind").and_then(Value::as_str) {
            return format!("ontology_definition:{sub}");
        }
    }
    kind.to_owned()
}

/// Runs the census read-only against an already-open store and its snapshot.
pub fn run_census(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    policy: &CensusPolicy,
) -> Result<CensusReceipt, UniverseError> {
    policy.validate()?;

    // Index existing Asset projections by the Node they derive from, so a Node
    // that already has a projection is observed rather than assumed absent.
    let mut assets_by_source: BTreeMap<EntityKey, usize> = BTreeMap::new();
    for entity in &snapshot.entities {
        let Some(content_ref) = entity.content.as_ref() else {
            continue;
        };
        let content = store.read_content(content_ref)?;
        if content.get("kind").and_then(Value::as_str) != Some("asset_projection") {
            continue;
        }
        let lifecycle_current = match content.get("lifecycle") {
            Some(Value::String(state)) => state == "current" || state == "observed",
            Some(Value::Object(map)) => map.get("state").and_then(Value::as_str) == Some("current"),
            _ => false,
        };
        if !lifecycle_current {
            continue;
        }
        // EntityKey serializes as a 32-digit hex string (see universe-core), so
        // source_node must be parsed base-16, not decimal.
        if let Some(source) = content
            .get("source_node")
            .and_then(Value::as_str)
            .and_then(|raw| u128::from_str_radix(raw, 16).ok().map(EntityKey))
        {
            *assets_by_source.entry(source).or_default() += 1;
        }
    }

    let mut kind_counts = BTreeMap::new();
    let mut requirement_counts: BTreeMap<String, usize> =
        REQUIREMENTS.iter().map(|r| (r.to_string(), 0)).collect();
    let mut class_counts: BTreeMap<String, usize> =
        CLASSES.iter().map(|c| (c.to_string(), 0)).collect();
    let mut classifications = Vec::with_capacity(snapshot.entities.len());

    for entity in &snapshot.entities {
        let kind = match entity.content.as_ref() {
            Some(content_ref) => node_kind(&store.read_content(content_ref)?),
            None => "<no-content>".to_owned(),
        };
        // An Asset is itself a derived projection, never a Node to be converted;
        // excluding it keeps the census over canonical Nodes only.
        if kind == "asset_projection" || kind == "asset_payload" {
            continue;
        }
        let requirement = policy.requirement_for(&kind).to_owned();
        let asset_count = assets_by_source.get(&entity.key).copied().unwrap_or(0);
        let has_current_asset = asset_count >= 1;
        let conversion_class = match requirement.as_str() {
            "intentionally_assetless" => "intentionally_assetless",
            "unknown" => "unknown",
            "required" => {
                if asset_count > 1 {
                    "partial" // duplicate current projections — converted but not canonical
                } else if has_current_asset {
                    "converted"
                } else {
                    "blocked" // required Asset not yet materialized
                }
            }
            other => {
                return Err(validation(format!(
                    "requirement {other} has no conversion class"
                )))
            }
        }
        .to_owned();

        *kind_counts.entry(kind.clone()).or_insert(0) += 1;
        *requirement_counts
            .get_mut(&requirement)
            .expect("validated requirement") += 1;
        *class_counts
            .get_mut(&conversion_class)
            .expect("validated class") += 1;
        classifications.push(NodeClassification {
            entity: entity.key,
            kind,
            requirement,
            has_current_asset,
            conversion_class,
        });
    }
    classifications.sort_by_key(|classification| classification.entity);

    Ok(CensusReceipt {
        kind: "node_asset_conversion_census".into(),
        policy_id: policy.policy_id.clone(),
        policy_hash: canonical_hash(policy)?,
        universe: snapshot.universe,
        snapshot_hash: snapshot.canonical_hash()?,
        total_nodes: classifications.len(),
        kind_counts,
        requirement_counts,
        class_counts,
        classifications,
        epistemic_state: "measured".into(),
    })
}

/// Censuses a store twice through independent reopens and requires identical
/// receipts, so the reported classification is deterministic and read back from
/// the persisted store rather than trusted from a single pass.
pub fn census_with_readback(
    store_root: impl AsRef<Path>,
    policy: &CensusPolicy,
) -> Result<CensusReceipt, UniverseError> {
    let store_root = store_root.as_ref();
    let first_store = UniverseStore::open(store_root)?;
    let first = first_store.replay(first_store.load_snapshot()?)?;
    let receipt = run_census(&first_store, &first, policy)?;

    let second_store = UniverseStore::open(store_root)?;
    let second = second_store.replay(second_store.load_snapshot()?)?;
    let readback = run_census(&second_store, &second, policy)?;
    if readback != receipt {
        return Err(validation("census differs across independent store reopen"));
    }
    Ok(receipt)
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_store::load_seed;

    fn canonical_policy() -> CensusPolicy {
        CensusPolicy::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/assets/node-asset-census-policy.json"),
        )
        .unwrap()
    }

    fn canonical_store(root: &Path) -> UniverseStore {
        let seed = load_seed(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/ontology/canonical-ontology.json"),
        )
        .unwrap();
        let store = UniverseStore::open(root).unwrap();
        store.install_seed(&seed).unwrap();
        store
    }

    #[test]
    fn canonical_corpus_is_classified_and_conserved() {
        let temp = tempfile::tempdir().unwrap();
        let _ = canonical_store(temp.path());
        let policy = canonical_policy();
        let receipt = census_with_readback(temp.path(), &policy).unwrap();

        // The real canonical ontology has 231 Nodes; none is an Asset yet.
        assert_eq!(receipt.total_nodes, 231);
        assert_eq!(receipt.classifications.len(), 231);
        // Every Node is conserved across exactly one class and one requirement.
        assert_eq!(receipt.class_counts.values().sum::<usize>(), 231);
        assert_eq!(receipt.requirement_counts.values().sum::<usize>(), 231);
        // No Asset exists, so nothing is converted or partial yet.
        assert_eq!(receipt.class_counts["converted"], 0);
        assert_eq!(receipt.class_counts["partial"], 0);
        // Required Nodes with no Asset are blocked; the ontology_source documents
        // are the declared required Nodes.
        assert_eq!(
            receipt.class_counts["blocked"],
            receipt.requirement_counts["required"]
        );
        assert!(receipt.requirement_counts["required"] >= 1);
        assert_eq!(receipt.epistemic_state, "measured");
        assert_eq!(receipt.policy_hash.len(), 64);
    }

    #[test]
    fn unmapped_kinds_stay_unknown_not_invented() {
        let temp = tempfile::tempdir().unwrap();
        let _ = canonical_store(temp.path());
        // A policy that maps nothing must leave every Node unknown.
        let empty = CensusPolicy {
            policy_id: "empty".into(),
            default_requirement: "unknown".into(),
            rules: vec![],
        };
        let receipt = census_with_readback(temp.path(), &empty).unwrap();
        assert_eq!(receipt.class_counts["unknown"], receipt.total_nodes);
        assert_eq!(receipt.requirement_counts["unknown"], receipt.total_nodes);
    }

    #[test]
    fn non_unknown_default_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let store = canonical_store(temp.path());
        let snapshot = store.replay(store.load_snapshot().unwrap()).unwrap();
        let bad = CensusPolicy {
            policy_id: "bad".into(),
            default_requirement: "required".into(),
            rules: vec![],
        };
        assert!(matches!(
            run_census(&store, &snapshot, &bad),
            Err(UniverseError::Validation(message))
                if message.contains("default requirement must be unknown")
        ));
    }
}
