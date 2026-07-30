//! G2 phase 4 — bounded inert code-Node import pilot.
//!
//! Phase 3 (`ontology_pilot`) deliberately left every source `code_definition`
//! subtype **unresolved**: recognised, but never activated as ontology. This
//! module is the phase-4 continuation. It gives those code-bearing symbols
//! (`code_definition` and related code symbols such as functions, procedures,
//! trigger functions, and algorithm implementations) a *proper* import: each one
//! becomes an **inert, non-executable Node** that preserves full provenance
//! (source id, source revision, import batch, content hash, and an adaptation
//! receipt) and carries an explicit, load-bearing guarantee that it can never be
//! activated or dispatched as behavior.
//!
//! The guarantee is expressed three independent ways, all read back after replay:
//!
//! - every imported Node's content is stamped `executable: false`,
//!   `dispatchable: false`, `activatable: false`, `quarantined_from_activation:
//!   true` — the manifest has **no field** that can assert the opposite;
//! - every imported Node gets a `QUARANTINED_FROM` relation to a single
//!   `activation_barrier` Node, committed by the approved import ChangeSet;
//! - an engine-level guard, [`attempt_activation`], **refuses** activation for
//!   every imported code Node, and is exercised by a negative test.
//!
//! This module imports **no** code payload and makes **nothing** runnable. It
//! does not compile, shadow-execute, wire a trigger, or transfer a lease. Making
//! any of these Nodes executable is a distinct, out-of-scope act that would
//! require its own approved ChangeSet plus the shadow-execution evidence produced
//! by `code_translation` — never this inert import.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};
use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseError, UniverseId};
use universe_store::{
    EntityRecord, GraphSeed, RelationRecord, SeedEntity, SeedRelation, UniverseSnapshot,
    UniverseStore,
};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// Code-bearing source symbol kinds this pilot imports. All are treated as
/// hard-inert; none is ever made executable, regardless of kind.
const CODE_KINDS: [&str; 5] = [
    "code_definition",
    "function",
    "procedure",
    "trigger_function",
    "algorithm_implementation",
];

const SYM_SOURCE: &str = "postgres_import_source";
const SYM_CODE_NODE: &str = "imported_code_node";
const SYM_BARRIER: &str = "activation_barrier";
const SYM_CHANGESET: &str = "code_import_changeset";
const SYM_RECEIPT: &str = "import_receipt";
const SYM_GOVERNED_BY: &str = "GOVERNED_BY";
const SYM_PART_OF: &str = "PART_OF";
const SYM_QUARANTINED_FROM: &str = "QUARANTINED_FROM";
const SYM_HAS_RECEIPT: &str = "HAS_RECEIPT";

/// Offsets from the seed relation range to the ChangeSet-committed ranges, so the
/// three relation families (seed provenance, membership, quarantine) never
/// collide.
const MEMBERSHIP_RELATION_OFFSET: u128 = 0x100;
const QUARANTINE_RELATION_OFFSET: u128 = 0x200;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PilotSource {
    pub atom: EntityKey,
    pub authority_id: String,
    pub source_graph_scope: Vec<String>,
    pub observed_at: String,
    pub source_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApprovedChangeSet {
    pub atom: EntityKey,
    pub change_id: String,
    pub authority: String,
    pub status: String,
}

/// One source code symbol to import as an inert Node. Note there is deliberately
/// no `executable`/`activatable` field: the manifest cannot assert executability;
/// the engine always writes it `false`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeNode {
    pub atom: EntityKey,
    pub source_label: String,
    pub source_id: String,
    pub source_graph: String,
    pub code_kind: String,
    pub content_sha256: String,
    pub source_revision: u64,
    pub justification: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodePilotManifest {
    pub contract_version: u16,
    pub universe: UniverseId,
    pub source: PilotSource,
    pub changeset: ApprovedChangeSet,
    pub barrier_atom: EntityKey,
    pub import_batch: String,
    pub nodes: Vec<CodeNode>,
    pub receipt_atom: EntityKey,
    pub receipt_relation: RelationKey,
    pub relation_key_start: RelationKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodePilotEvidence {
    pub change_id: String,
    pub import_batch: String,
    pub universe: UniverseId,
    pub total_nodes: usize,
    /// Imported code Nodes present and inert after replay (must equal total).
    pub imported_inert: usize,
    /// Nodes carrying a `QUARANTINED_FROM` relation to the barrier (must equal total).
    pub quarantined_from_activation: usize,
    /// Nodes that are `PART_OF` the import ChangeSet (must equal total).
    pub changeset_members: usize,
    /// Nodes whose stored provenance is complete (must equal total).
    pub provenance_complete: usize,
    /// Nodes for which the engine guard refused activation (must equal total).
    pub activation_attempts_refused: usize,
    /// All three MUST be zero: nothing is made executable, dispatchable, or activated.
    pub executable_count: usize,
    pub dispatchable_count: usize,
    pub activated_count: usize,
    pub pre_receipt_snapshot_hash: String,
    pub final_snapshot_hash: String,
    pub final_revision: Revision,
    pub final_tick: Tick,
    pub content_records_read_back: usize,
    pub receipt_atom: EntityKey,
}

pub fn load_manifest(path: impl AsRef<Path>) -> Result<CodePilotManifest, UniverseError> {
    let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| UniverseError::CorruptContent(error.to_string()))
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn validate_manifest(manifest: &CodePilotManifest) -> Result<(), UniverseError> {
    if manifest.contract_version != 0 {
        return Err(UniverseError::UnsupportedVersion(manifest.contract_version));
    }
    if !manifest.changeset.status.starts_with("approved") {
        return Err(validation("code import ChangeSet is not approved"));
    }
    if manifest.source.source_graph_scope.is_empty() || manifest.source.source_revision == 0 {
        return Err(validation("code pilot source scope or revision is missing"));
    }
    if manifest.import_batch.trim().is_empty() {
        return Err(validation("code pilot declares no import batch"));
    }
    if manifest.nodes.is_empty() {
        return Err(validation("code pilot declares no code Node"));
    }

    // The barrier, source, changeset, and receipt identities must not collide
    // with any code Node identity.
    let mut atoms = BTreeSet::new();
    for reserved in [
        manifest.source.atom,
        manifest.changeset.atom,
        manifest.barrier_atom,
        manifest.receipt_atom,
    ] {
        if !atoms.insert(reserved) {
            return Err(validation("code pilot reuses a reserved Atom identity"));
        }
    }

    let source_scope: BTreeSet<_> = manifest.source.source_graph_scope.iter().collect();
    for node in &manifest.nodes {
        if !atoms.insert(node.atom) {
            return Err(validation(format!(
                "code Node {} reuses an Atom identity",
                node.source_label
            )));
        }
        if !CODE_KINDS.contains(&node.code_kind.as_str()) {
            return Err(validation(format!(
                "code Node {} has unknown code kind {}",
                node.source_label, node.code_kind
            )));
        }
        // Exact source-graph scope is mandatory: a code Node cannot be imported
        // from a graph outside the declared, read-only source scope.
        if !source_scope.contains(&node.source_graph) {
            return Err(validation(format!(
                "code Node {} imports from a graph outside the declared source scope",
                node.source_label
            )));
        }
        if node.source_id.trim().is_empty() {
            return Err(validation(format!(
                "code Node {} has no source id",
                node.source_label
            )));
        }
        if !valid_hash(&node.content_sha256) {
            return Err(validation(format!(
                "code Node {} has an invalid content hash",
                node.source_label
            )));
        }
        if node.source_revision == 0 {
            return Err(validation(format!(
                "code Node {} has no pinned source revision",
                node.source_label
            )));
        }
        if node.justification.trim().is_empty() {
            return Err(validation(format!(
                "code Node {} is not justified",
                node.source_label
            )));
        }
    }
    Ok(())
}

/// The deterministic adaptation receipt. It carries no live snapshot hash, so a
/// re-run produces byte-identical content and the import stays idempotent. Public
/// so the runner and the bin write the exact same bytes that are stored.
pub fn receipt_content(manifest: &CodePilotManifest) -> Value {
    json!({
        "kind": "adaptation_receipt",
        "phase": "g2_phase4_inert_code_import",
        "change_id": manifest.changeset.change_id,
        "import_batch": manifest.import_batch,
        "status": "measured_inert_code_import",
        "information_status": "measured",
        "code_imported_inert": true,
        "code_payload_imported": false,
        "code_executable": false,
        "code_dispatchable": false,
        "code_activated": false,
        "trigger_wired": false,
        "lease_transferred": false,
        "shadow_executed": false,
        "total_nodes": manifest.nodes.len(),
        "nodes": manifest
            .nodes
            .iter()
            .map(|node| {
                json!({
                    "node": node.atom,
                    "source_label": node.source_label,
                    "source_id": node.source_id,
                    "source_graph": node.source_graph,
                    "code_kind": node.code_kind,
                    "content_sha256": node.content_sha256,
                    "source_revision": node.source_revision,
                    "quarantined_from_activation": true,
                    "executable": false,
                })
            })
            .collect::<Vec<_>>(),
    })
}

pub fn materialize_seed(manifest: &CodePilotManifest) -> Result<GraphSeed, UniverseError> {
    validate_manifest(manifest)?;
    let symbols = vec![
        SYM_SOURCE.to_owned(),
        SYM_CODE_NODE.to_owned(),
        SYM_BARRIER.to_owned(),
        SYM_CHANGESET.to_owned(),
        SYM_RECEIPT.to_owned(),
        SYM_GOVERNED_BY.to_owned(),
        SYM_PART_OF.to_owned(),
        SYM_QUARANTINED_FROM.to_owned(),
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
                "source_revision": manifest.source.source_revision,
                "read_only": true,
                "credentials_stored": false,
            }),
        ),
        entity(
            manifest.changeset.atom,
            SYM_CHANGESET,
            json!({
                "kind": "code_import_changeset",
                "change_id": manifest.changeset.change_id,
                "authority": manifest.changeset.authority,
                "status": manifest.changeset.status,
                "import_batch": manifest.import_batch,
                "act": "inert_code_node_import",
            }),
        ),
        // The activation barrier: a single Node that every imported code Node is
        // quarantined FROM. Its presence makes the non-executability guarantee an
        // explicit graph relation, not just a per-Node flag.
        entity(
            manifest.barrier_atom,
            SYM_BARRIER,
            json!({
                "kind": "activation_barrier",
                "guarantee": "imported code Nodes are non-executable and cannot be activated or dispatched",
                "crossing_requires": "a distinct approved ChangeSet plus shadow-execution evidence, out of scope for this inert import",
            }),
        ),
    ];
    for node in &manifest.nodes {
        entities.push(entity(
            node.atom,
            SYM_CODE_NODE,
            json!({
                "kind": "imported_code_node",
                "source_label": node.source_label,
                "source_id": node.source_id,
                "source_graph": node.source_graph,
                "code_kind": node.code_kind,
                "content_sha256": node.content_sha256,
                "source_revision": node.source_revision,
                "import_batch": manifest.import_batch,
                "justification": node.justification,
                // The load-bearing inertness stamps. The manifest cannot set
                // these; the engine writes them unconditionally.
                "payload_imported": false,
                "executable": false,
                "dispatchable": false,
                "activatable": false,
                "quarantined_from_activation": true,
                "information_status": "observed",
            }),
        ));
    }

    // Seed installs the source, changeset, barrier, and inert code Nodes, each
    // GOVERNED_BY the read-only source. The import act itself — ChangeSet
    // membership and the quarantine relations — is committed later by
    // run_code_pilot so it is attributable and idempotent.
    let mut next = manifest.relation_key_start.0;
    let mut relations = vec![relation(
        &mut next,
        manifest.changeset.atom,
        manifest.source.atom,
        SYM_GOVERNED_BY,
    )];
    for node in &manifest.nodes {
        relations.push(relation(
            &mut next,
            node.atom,
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

/// Engine-level activation guard. Given an imported code Node's stored content,
/// this **refuses** activation. There is no input for which it returns `Ok` for
/// an imported code Node: inertness is not negotiable and cannot be overridden by
/// content flags. Returns `Ok(())` only for content that is not an imported code
/// Node at all (defensive; such content is not this pilot's concern).
pub fn attempt_activation(content: &Value) -> Result<(), UniverseError> {
    let is_code_node = content.get("kind").and_then(Value::as_str) == Some("imported_code_node");
    if !is_code_node {
        return Ok(());
    }
    let label = content
        .get("source_label")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    Err(UniverseError::Validation(format!(
        "activation refused: imported code Node {label} is quarantined from activation and can never be made executable by this pilot"
    )))
}

pub fn run_code_pilot(
    manifest: &CodePilotManifest,
    output: impl AsRef<Path>,
) -> Result<CodePilotEvidence, UniverseError> {
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

    // Independent reopen for the import act, mirroring the other pilots.
    let independent_store = UniverseStore::open(store_root)?;
    let mut independent = independent_store.replay(independent_store.load_snapshot()?)?;

    let receipt = receipt_content(manifest);

    // The approved import ChangeSet is applied exactly once; a resumed run finds
    // its key already present and re-issues no command.
    let import_key = format!("{}:import", manifest.changeset.change_id);
    if !independent.event_keys.contains(&import_key) {
        let part_of = symbol(&independent, SYM_PART_OF)?;
        let quarantined_from = symbol(&independent, SYM_QUARANTINED_FROM)?;
        let has_receipt = symbol(&independent, SYM_HAS_RECEIPT)?;
        let receipt_symbol = symbol(&independent, SYM_RECEIPT)?;
        let receipt_ref = independent_store.append_content(&receipt)?;

        let mut commands = Vec::new();
        let mut membership_key = manifest.relation_key_start.0 + MEMBERSHIP_RELATION_OFFSET;
        let mut quarantine_key = manifest.relation_key_start.0 + QUARANTINE_RELATION_OFFSET;
        for node in &manifest.nodes {
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(membership_key),
                    generation: 0,
                    source: node.atom,
                    target: manifest.changeset.atom,
                    predicate: part_of,
                    content: Some(independent_store.append_content(&json!({
                        "kind": "import_relation",
                        "role": "changeset_membership",
                        "justification": "Imported inert code Node authorized by the approved, scoped code-import ChangeSet."
                    }))?),
                },
            });
            membership_key += 1;
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(quarantine_key),
                    generation: 0,
                    source: node.atom,
                    target: manifest.barrier_atom,
                    predicate: quarantined_from,
                    content: Some(independent_store.append_content(&json!({
                        "kind": "import_relation",
                        "role": "activation_quarantine",
                        "justification": "Imported code Node is quarantined from activation by the barrier; crossing it is a distinct out-of-scope act."
                    }))?),
                },
            });
            quarantine_key += 1;
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
                    "justification": "Independent readback produced this measured inert-code-import receipt; no code was executed or activated."
                }))?),
            },
        });

        let transaction = UniverseTransaction::prepare(
            &independent,
            UniverseWriteSet {
                base_revision: independent.revision,
                idempotency_key: import_key,
                causal_ancestry: vec![manifest.changeset.change_id.clone()],
                commands,
            },
        )?;
        let tick = Tick(independent.tick.0 + 1);
        transaction.commit(&independent_store, &mut independent, tick)?;
    }

    // Final independent replay and verification.
    let final_store = UniverseStore::open(store_root)?;
    let final_snapshot = final_store.replay(final_store.load_snapshot()?)?;
    let part_of = symbol(&final_snapshot, SYM_PART_OF)?;
    let quarantined_from = symbol(&final_snapshot, SYM_QUARANTINED_FROM)?;

    let mut imported_inert = 0;
    let mut quarantined_from_activation = 0;
    let mut changeset_members = 0;
    let mut provenance_complete = 0;
    let mut activation_attempts_refused = 0;
    let mut executable_count = 0;
    let mut dispatchable_count = 0;
    let mut activated_count = 0;

    for node in &manifest.nodes {
        let content = final_snapshot
            .entities
            .iter()
            .find(|entity| entity.key == node.atom)
            .and_then(|entity| entity.content.as_ref())
            .ok_or_else(|| {
                validation(format!(
                    "code Node {} missing after replay",
                    node.source_label
                ))
            })?;
        let value = final_store.read_content(content)?;

        // Inertness stamps, read back from the store (never trusted from memory).
        let executable = value.get("executable") == Some(&Value::Bool(true));
        let dispatchable = value.get("dispatchable") == Some(&Value::Bool(true));
        let activated = value.get("activatable") == Some(&Value::Bool(true))
            || value.get("activated") == Some(&Value::Bool(true))
            || value.get("activated_for_later_execution") == Some(&Value::Bool(true));
        if executable {
            executable_count += 1;
        }
        if dispatchable {
            dispatchable_count += 1;
        }
        if activated {
            activated_count += 1;
        }
        if !executable
            && !dispatchable
            && !activated
            && value.get("quarantined_from_activation") == Some(&Value::Bool(true))
            && value.get("payload_imported") == Some(&Value::Bool(false))
        {
            imported_inert += 1;
        }

        // Provenance completeness, measured from stored content.
        let provenance_ok = value.get("source_id").and_then(Value::as_str)
            == Some(node.source_id.as_str())
            && value.get("content_sha256").and_then(Value::as_str)
                == Some(node.content_sha256.as_str())
            && value.get("source_revision").and_then(Value::as_u64) == Some(node.source_revision)
            && value.get("import_batch").and_then(Value::as_str)
                == Some(manifest.import_batch.as_str())
            && value.get("code_kind").and_then(Value::as_str) == Some(node.code_kind.as_str());
        if provenance_ok {
            provenance_complete += 1;
        }

        // The engine guard must refuse activation for every imported code Node.
        if attempt_activation(&value).is_err() {
            activation_attempts_refused += 1;
        }

        // Explicit relations: PART_OF the ChangeSet and QUARANTINED_FROM the barrier.
        let is_member = final_snapshot.relations.iter().any(|relation| {
            relation.source == node.atom
                && relation.target == manifest.changeset.atom
                && relation.predicate == part_of
        });
        if is_member {
            changeset_members += 1;
        }
        let is_quarantined = final_snapshot.relations.iter().any(|relation| {
            relation.source == node.atom
                && relation.target == manifest.barrier_atom
                && relation.predicate == quarantined_from
        });
        if is_quarantined {
            quarantined_from_activation += 1;
        }
    }

    let total = manifest.nodes.len();
    if executable_count != 0 || dispatchable_count != 0 || activated_count != 0 {
        return Err(UniverseError::CorruptContent(
            "code pilot produced an executable, dispatchable, or activated code Node".into(),
        ));
    }
    if imported_inert != total
        || quarantined_from_activation != total
        || changeset_members != total
        || provenance_complete != total
        || activation_attempts_refused != total
    {
        return Err(UniverseError::CorruptContent(
            "code pilot readback disagrees with the expected inert-import invariants".into(),
        ));
    }

    // The stored receipt must byte-match the deterministic receipt content.
    let receipt_entity = final_snapshot
        .entities
        .iter()
        .find(|entity| entity.key == manifest.receipt_atom)
        .and_then(|entity| entity.content.as_ref())
        .ok_or_else(|| validation("inert-code-import receipt is missing after replay"))?;
    if final_store.read_content(receipt_entity)? != receipt {
        return Err(UniverseError::CorruptContent(
            "inert-code-import receipt differs after replay".into(),
        ));
    }

    let content_records_read_back = read_all_content(&final_store, &final_snapshot)?;
    Ok(CodePilotEvidence {
        change_id: manifest.changeset.change_id.clone(),
        import_batch: manifest.import_batch.clone(),
        universe: final_snapshot.universe,
        total_nodes: total,
        imported_inert,
        quarantined_from_activation,
        changeset_members,
        provenance_complete,
        activation_attempts_refused,
        executable_count: 0,
        dispatchable_count: 0,
        activated_count: 0,
        pre_receipt_snapshot_hash,
        final_snapshot_hash: final_snapshot.canonical_hash()?,
        final_revision: final_snapshot.revision,
        final_tick: final_snapshot.tick,
        content_records_read_back,
        receipt_atom: manifest.receipt_atom,
    })
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
        .ok_or_else(|| validation(format!("code pilot symbol {value} is not interned")))
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> CodePilotManifest {
        serde_json::from_str(include_str!(
            "../../../fixtures/import/postgres-code-pilot.json"
        ))
        .unwrap()
    }

    #[test]
    fn code_nodes_import_inert_with_provenance_and_read_back() {
        let manifest = manifest();
        let temp = tempfile::tempdir().unwrap();
        let evidence = run_code_pilot(&manifest, temp.path()).unwrap();

        assert_eq!(evidence.total_nodes, 5);
        assert_eq!(evidence.imported_inert, 5);
        assert_eq!(evidence.quarantined_from_activation, 5);
        assert_eq!(evidence.changeset_members, 5);
        assert_eq!(evidence.provenance_complete, 5);
        assert_eq!(evidence.activation_attempts_refused, 5);
        // The load-bearing inertness guarantee.
        assert_eq!(evidence.executable_count, 0);
        assert_eq!(evidence.dispatchable_count, 0);
        assert_eq!(evidence.activated_count, 0);
    }

    #[test]
    fn rerun_is_idempotent() {
        let manifest = manifest();
        let temp = tempfile::tempdir().unwrap();
        let first = run_code_pilot(&manifest, temp.path()).unwrap();
        let second = run_code_pilot(&manifest, temp.path()).unwrap();
        assert_eq!(first.final_revision, second.final_revision);
        assert_eq!(first.final_snapshot_hash, second.final_snapshot_hash);
        assert_eq!(first.final_tick, second.final_tick);
    }

    #[test]
    fn activation_of_an_imported_code_node_is_refused() {
        // NEGATIVE test: import the Nodes, then read one back from the store and
        // attempt to activate it. The engine guard must refuse — there is no path
        // that makes an imported code Node executable.
        let manifest = manifest();
        let temp = tempfile::tempdir().unwrap();
        run_code_pilot(&manifest, temp.path()).unwrap();

        let store = UniverseStore::open(temp.path()).unwrap();
        let snapshot = store.replay(store.load_snapshot().unwrap()).unwrap();
        let node = &manifest.nodes[0];
        let content_ref = snapshot
            .entities
            .iter()
            .find(|entity| entity.key == node.atom)
            .and_then(|entity| entity.content.as_ref())
            .expect("imported code Node exists after replay");
        let content = store.read_content(content_ref).unwrap();

        // The stored Node is stamped non-executable...
        assert_eq!(content.get("executable"), Some(&Value::Bool(false)));
        assert_eq!(content.get("dispatchable"), Some(&Value::Bool(false)));
        assert_eq!(
            content.get("quarantined_from_activation"),
            Some(&Value::Bool(true))
        );
        // ...and the engine refuses to activate it.
        let refusal = attempt_activation(&content);
        assert!(matches!(
            refusal,
            Err(UniverseError::Validation(message)) if message.contains("activation refused")
        ));
    }

    #[test]
    fn activation_guard_refuses_even_if_content_claims_executable() {
        // Inertness is not overridable: even a (hypothetical) code-Node content
        // that falsely claims executable is still refused, because the guard keys
        // on the imported_code_node kind, not on a spoofable flag.
        let spoofed = json!({
            "kind": "imported_code_node",
            "source_label": "spoofed",
            "executable": true,
            "activatable": true,
        });
        assert!(attempt_activation(&spoofed).is_err());
    }

    #[test]
    fn unknown_code_kind_is_rejected() {
        let mut manifest = manifest();
        manifest.nodes[0].code_kind = "sql_view".into();
        assert!(matches!(
            validate_manifest(&manifest),
            Err(UniverseError::Validation(message)) if message.contains("unknown code kind")
        ));
    }

    #[test]
    fn code_node_outside_source_scope_is_rejected() {
        let mut manifest = manifest();
        manifest.nodes[0].source_graph = "l2:some-other-graph".into();
        assert!(matches!(
            validate_manifest(&manifest),
            Err(UniverseError::Validation(message)) if message.contains("outside the declared source scope")
        ));
    }

    #[test]
    fn invalid_content_hash_is_rejected() {
        let mut manifest = manifest();
        manifest.nodes[0].content_sha256 = "not-a-hash".into();
        assert!(matches!(
            validate_manifest(&manifest),
            Err(UniverseError::Validation(message)) if message.contains("invalid content hash")
        ));
    }
}
