//! READ-ONLY export of a construct from the LIVE store to `exports/<slug>.json`.
//!
//! A construct lives in the graph as a space root plus its member entities and
//! the relations between them. This module PROJECTS that committed shape into a
//! single self-describing JSON document and writes it under an exports
//! directory. It opens the store, loads the committed snapshot, replays the
//! valid event log and hydrates content; it NEVER appends an event, NEVER
//! commits a transaction and mints zero symbols.
//!
//! The document is an OBSERVATION, not the construct. Per the local-observation
//! contract it declares its origin (which store, which revision), its budget and
//! whether that budget was exhausted, and it labels every claim epistemically:
//!
//! - the graph content is `observed` (independent readback of durable bytes);
//! - the member set is `partial` — enumeration is bounded and heuristic;
//! - the runtime health is `not_measured` — an export reads a definition, it
//!   never runs the construct, so it can never say `healthy`;
//! - the implementation's own `graph_status` / `wiring_status` / `runtime_status`
//!   fields are carried under `self_report`, quarantined as self-declared and
//!   explicitly NOT evidence.
//!
//! Member enumeration mirrors the read path of `bin/construct_map`: (a) an id
//! discriminator-suffix match, (b) a bounded relation closure over
//! construct-INTERNAL predicates only (never `DEPENDS_ON` / `PART_OF`, which
//! cross construct boundaries and would flood the connected component), and
//! (c) satellite-by-content-reference for ports and bindings whose id slug
//! diverges from the construct's.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use universe_core::EntityKey;
use universe_store::{UniverseSnapshot, UniverseStore};

use crate::E2eError;

/// Bumped whenever the document shape changes in a way a reader must notice.
pub const EXPORT_FORMAT_VERSION: u16 = 1;

/// Default destination, relative to the repository root.
pub const DEFAULT_EXPORTS_DIR: &str = "exports";

/// Default LIVE store.
pub const DEFAULT_STORE_DIR: &str = "artifacts/ontology-registry/current/store";

/// Index written beside the documents when a whole-store export runs.
pub const INDEX_FILE: &str = "constructs.index.json";

/// Predicates that cross a construct's boundary. The relation closure must
/// never traverse them: every construct is linked to the shared validity
/// template by `APPLIES_IN` (conformance) and `DEPENDS_ON` (operational), and
/// to a parent scope by `PART_OF`, so following them would absorb unrelated
/// constructs into this export.
///
/// `APPLIES_IN` matters in BOTH directions and especially when the validity
/// template is itself the root: the template-guard below only skips absorbing
/// the template into someone else's member set, so an inbound conformance edge
/// would otherwise walk the closure out into every conforming construct.
const CROSS_BOUNDARY_PREDICATES: [&str; 3] = ["APPLIES_IN", "DEPENDS_ON", "PART_OF"];

/// The shared construct-validity loop template. Never absorbed as a member of
/// another construct (it is a shared definition, not anyone's part).
const VALIDITY_TEMPLATE_ID: &str = "space:l2:mind-universe:construct-validity-v0";

/// Content marker of a construct space root.
const CONSTRUCT_CONTRACT_KIND: &str = "construct";

/// What one export actually did, with measured evidence rather than a claim of
/// success: the bytes written, their hash, and whether reading the file back
/// from disk reproduced that hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportReceipt {
    pub root_id: String,
    pub name: String,
    pub path: PathBuf,
    pub store_revision: u64,
    pub member_count: usize,
    pub internal_relation_count: usize,
    pub boundary_relation_count: usize,
    pub bytes_written: usize,
    pub sha256: String,
    /// Re-read from disk after writing and re-hashed: `true` only if the bytes
    /// on disk hash to `sha256` AND parse back as JSON.
    pub readback_verified: bool,
    /// `partial` (bounded enumeration) or `budget_exhausted`.
    pub observation_status: String,
}

// ---------------------------------------------------------------------------
// Store index
// ---------------------------------------------------------------------------

/// The committed snapshot plus canonical-id lookup tables, hydrated once.
struct StoreIndex {
    /// The vantage this observation was taken from, carried for the document
    /// header (a snapshot does not know which directory it came from).
    store_display: String,
    snapshot: UniverseSnapshot,
    id_to_key: BTreeMap<String, EntityKey>,
    key_to_id: BTreeMap<EntityKey, String>,
    key_to_wrapper: BTreeMap<EntityKey, Value>,
    key_to_sha: BTreeMap<EntityKey, String>,
    key_to_generation: BTreeMap<EntityKey, u32>,
    key_to_symbol: BTreeMap<EntityKey, u32>,
}

impl StoreIndex {
    /// READ-ONLY handshake: open, load the committed snapshot, replay the valid
    /// event log into it, hydrate every content-carrying entity. No write path
    /// is touched.
    fn open_at(store_dir: &Path) -> Result<Self, E2eError> {
        let store = UniverseStore::open(store_dir)?;
        let snapshot = store.replay(store.load_snapshot()?)?;

        let mut id_to_key = BTreeMap::new();
        let mut key_to_id = BTreeMap::new();
        let mut key_to_wrapper = BTreeMap::new();
        let mut key_to_sha = BTreeMap::new();
        let mut key_to_generation = BTreeMap::new();
        let mut key_to_symbol = BTreeMap::new();
        for entity in &snapshot.entities {
            key_to_generation.insert(entity.key, entity.generation);
            key_to_symbol.insert(entity.key, entity.symbol);
            let Some(ptr) = entity.content.as_ref() else {
                continue;
            };
            let wrapper = store.read_content(ptr)?;
            let Some(canonical_id) = wrapper.get("canonical_id").and_then(Value::as_str) else {
                continue;
            };
            id_to_key.insert(canonical_id.to_string(), entity.key);
            key_to_id.insert(entity.key, canonical_id.to_string());
            key_to_sha.insert(entity.key, ptr.sha256.clone());
            key_to_wrapper.insert(entity.key, wrapper);
        }

        Ok(Self {
            store_display: store_dir.display().to_string(),
            snapshot,
            id_to_key,
            key_to_id,
            key_to_wrapper,
            key_to_sha,
            key_to_generation,
            key_to_symbol,
        })
    }

    fn symbol_name(&self, symbol: u32) -> &str {
        self.snapshot
            .symbols
            .get(symbol as usize)
            .map(String::as_str)
            .unwrap_or("<unknown-symbol>")
    }

    fn entity_symbol(&self, key: EntityKey) -> &str {
        self.key_to_symbol
            .get(&key)
            .map(|symbol| self.symbol_name(*symbol))
            .unwrap_or("<unknown-symbol>")
    }

    fn generation(&self, key: EntityKey) -> u32 {
        self.key_to_generation.get(&key).copied().unwrap_or_default()
    }

    /// Every construct space root in the store, in canonical-id order.
    fn construct_roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = self
            .key_to_wrapper
            .values()
            .filter(|wrapper| {
                wrapper
                    .pointer("/content/contractKind")
                    .and_then(Value::as_str)
                    == Some(CONSTRUCT_CONTRACT_KIND)
            })
            .filter_map(|wrapper| {
                wrapper
                    .get("canonical_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }
}

// ---------------------------------------------------------------------------
// Member enumeration (bounded)
// ---------------------------------------------------------------------------

/// Everything after the first ':' — the discriminator suffix a construct's
/// members share (e.g. `l2:mind-universe:energy-pen-v0`).
fn discriminator(canonical_id: &str) -> &str {
    canonical_id
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(canonical_id)
}

fn is_satellite_subtype(subtype: &str) -> bool {
    matches!(
        subtype,
        "capability_port" | "physicalization_binding" | "broadcast_port"
    )
}

/// Collect every string value in a JSON tree (used to find the canonical ids a
/// member's content names — e.g. a code member's `authoring_gate.port_id`).
fn collect_strings(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::String(s) => {
            out.insert(s.clone());
        }
        Value::Array(items) => items.iter().for_each(|item| collect_strings(item, out)),
        Value::Object(map) => map.values().for_each(|item| collect_strings(item, out)),
        _ => {}
    }
}

struct Enumeration {
    keys: BTreeSet<EntityKey>,
    budget: usize,
    iterations_used: usize,
    budget_exhausted: bool,
}

/// Bounded three-stage member enumeration. See the module docs for why the
/// closure excludes the cross-boundary predicates and the shared template.
fn enumerate_members(index: &StoreIndex, root_id: &str) -> Enumeration {
    // (a) discriminator-suffix match: the space root plus every role member,
    //     including those carrying no stored relation.
    let suffix = discriminator(root_id).to_string();
    let mut keys: BTreeSet<EntityKey> = index
        .id_to_key
        .iter()
        .filter(|(canonical_id, _)| discriminator(canonical_id) == suffix)
        .map(|(_, key)| *key)
        .collect();

    // (b) bounded relation closure over construct-INTERNAL predicates only.
    let cross_boundary: BTreeSet<u32> = CROSS_BOUNDARY_PREDICATES
        .into_iter()
        .filter_map(|predicate| index.snapshot.symbol_id(predicate))
        .collect();
    let template_key = index.id_to_key.get(VALIDITY_TEMPLATE_ID).copied();
    let budget = index.snapshot.entities.len() + 8;
    let mut iterations_used = 0usize;
    let mut budget_exhausted = false;
    loop {
        if iterations_used >= budget {
            budget_exhausted = true;
            break;
        }
        iterations_used += 1;
        let mut added = false;
        for relation in &index.snapshot.relations {
            if cross_boundary.contains(&relation.predicate) {
                continue;
            }
            let source_in = keys.contains(&relation.source);
            let target_in = keys.contains(&relation.target);
            if source_in ^ target_in {
                let outside = if source_in {
                    relation.target
                } else {
                    relation.source
                };
                // Never absorb the shared validity template into a member set:
                // it is a definition every construct conforms to, not a part of
                // any one of them. (When it IS the root, it is already inside.)
                if Some(outside) == template_key {
                    continue;
                }
                if keys.insert(outside) {
                    added = true;
                }
            }
        }
        if !added {
            break;
        }
    }

    // (c) satellite-by-content-reference: ports and bindings a bound member's
    //     content names by canonical id but that no internal relation reaches.
    //     Only satellite kinds are admitted, never a `space`, so a wiki-link in
    //     a lineage string can never drag another construct in.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for key in &keys {
        if let Some(wrapper) = index.key_to_wrapper.get(key) {
            collect_strings(
                wrapper.get("content").unwrap_or(&Value::Null),
                &mut referenced,
            );
        }
    }
    for canonical_id in &referenced {
        let Some(key) = index.id_to_key.get(canonical_id) else {
            continue;
        };
        let subtype = index
            .key_to_wrapper
            .get(key)
            .and_then(|wrapper| wrapper.get("subtype"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let is_satellite = is_satellite_subtype(subtype)
            || canonical_id.starts_with("port:")
            || canonical_id.starts_with("broadcast_port:");
        if is_satellite {
            keys.insert(*key);
        }
    }

    Enumeration {
        keys,
        budget,
        iterations_used,
        budget_exhausted,
    }
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

struct ExportSummary {
    name: String,
    member_count: usize,
    internal_relation_count: usize,
    boundary_relation_count: usize,
    observation_status: String,
    revision: u64,
}

/// Build the export document for one construct. Pure: reads the index, writes
/// nothing.
fn build_document(index: &StoreIndex, root_id: &str) -> Result<(Value, ExportSummary), E2eError> {
    let root_key = *index.id_to_key.get(root_id).ok_or_else(|| {
        E2eError::Contract(format!("construct root {root_id} not present in the store"))
    })?;

    let enumeration = enumerate_members(index, root_id);

    // Members, in canonical-id order, each carrying its durable content hash so
    // a reader can tie the exported content back to the bytes in the store.
    let mut member_ids: Vec<(String, EntityKey)> = enumeration
        .keys
        .iter()
        .filter_map(|key| index.key_to_id.get(key).map(|id| (id.clone(), *key)))
        .collect();
    member_ids.sort();

    let root_wrapper = index
        .key_to_wrapper
        .get(&root_key)
        .cloned()
        .unwrap_or(Value::Null);
    let root_content = root_wrapper.get("content").cloned().unwrap_or(Value::Null);

    let runtime_moment_subtypes: BTreeSet<String> = root_content
        .get("runtime_moment_subtypes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let mut members = Vec::new();
    let mut runtime_moment_present = false;
    let mut self_report = Value::Null;

    for (canonical_id, key) in &member_ids {
        let wrapper = index.key_to_wrapper.get(key).cloned().unwrap_or(Value::Null);
        let subtype = str_field(&wrapper, "subtype").unwrap_or("").to_string();
        if runtime_moment_subtypes.contains(&subtype) {
            runtime_moment_present = true;
        }
        let content = wrapper.get("content").cloned().unwrap_or(Value::Null);
        if subtype == "implementation" {
            self_report = json!({
                "source": canonical_id,
                "graph_status": str_field(&content, "graph_status"),
                "wiring_status": str_field(&content, "wiring_status"),
                "runtime_status": str_field(&content, "runtime_status"),
                "verification_status": str_field(&content, "verification_status"),
                "epistemic": "self_reported",
                "note": "These fields are the implementation's OWN account of itself. They are carried here so a reader can see what the construct claims about itself; they are not evidence, and nothing in this document was derived from them."
            });
        }
        members.push(json!({
            "canonical_id": canonical_id,
            "entity_key": key,
            "generation": index.generation(*key),
            "entity_symbol": index.entity_symbol(*key),
            "node_type": str_field(&wrapper, "node_type"),
            "subtype": subtype,
            "name": str_field(&content, "name"),
            "content_sha256": index.key_to_sha.get(key),
            "content": content,
        }));
    }

    // Relations. The scan is exhaustive over the snapshot's relation set, so the
    // edge view is complete GIVEN the member set; the member set is the bounded
    // part. An edge with exactly one endpoint inside is a boundary edge — kept,
    // because what a construct is wired to is part of reading it.
    let unresolved = "<unresolved-entity>".to_string();
    let mut internal_relations = Vec::new();
    let mut boundary_relations = Vec::new();
    for relation in &index.snapshot.relations {
        let source_in = enumeration.keys.contains(&relation.source);
        let target_in = enumeration.keys.contains(&relation.target);
        if !source_in && !target_in {
            continue;
        }
        let source = index.key_to_id.get(&relation.source).unwrap_or(&unresolved);
        let target = index.key_to_id.get(&relation.target).unwrap_or(&unresolved);
        let mut edge = json!({
            "predicate": index.symbol_name(relation.predicate),
            "source": source,
            "target": target,
        });
        if source_in && target_in {
            internal_relations.push(edge);
        } else {
            edge["direction"] = json!(if source_in { "outgoing" } else { "incoming" });
            boundary_relations.push(edge);
        }
    }

    let name = str_field(&root_content, "name")
        .unwrap_or(root_id)
        .to_string();

    let observation_status = if enumeration.budget_exhausted {
        "budget_exhausted"
    } else {
        "partial"
    };

    let document = json!({
        "export_format_version": EXPORT_FORMAT_VERSION,
        "kind": "construct_export",
        "observation": {
            "origin": {
                "store": index.store_display,
                "universe": index.snapshot.universe,
                "revision": index.snapshot.revision.0,
                "tick": index.snapshot.tick,
                "vantage": "read-only replay of the durable store projection (open + load_snapshot + replay + read_content); no event appended, no transaction committed"
            },
            "exported_at_unix_s": now_unix_s(),
            "status": observation_status,
            "budget": {
                "max_closure_iterations": enumeration.budget,
                "closure_iterations_used": enumeration.iterations_used,
                "entities_in_snapshot": index.snapshot.entities.len(),
                "relations_in_snapshot": index.snapshot.relations.len()
            },
            "epistemic": {
                "graph_content": "observed",
                "member_set": "partial",
                "relations": "observed",
                "runtime_health": "not_measured",
                "objective_satisfaction": "not_measured",
                "notes": [
                    "The member set comes from a bounded heuristic (id-suffix match + construct-internal relation closure + satellite content references). A member the heuristic does not reach is `unknown`, never `known_absent`.",
                    "This document is a definition read from the graph. It carries no runtime evidence: nothing here can say `healthy`, `correct` or `objective satisfied`. Absence of a runtime moment is reported, not interpreted.",
                    "The store is a durable projection, not the truth. The revision above states which view this was taken from; a later revision may hold something else."
                ]
            }
        },
        "construct": {
            "root_id": root_id,
            "entity_key": root_key,
            "name": name,
            "node_type": str_field(&root_wrapper, "node_type"),
            "subtype": str_field(&root_wrapper, "subtype"),
            "contract_kind": root_content.get("contractKind"),
            "dominant_scale": str_field(&root_content, "dominant_scale"),
            "purpose": str_field(&root_content, "purpose"),
            "lineage": str_field(&root_content, "lineage"),
            "runtime_moment_subtypes": runtime_moment_subtypes.iter().collect::<Vec<_>>(),
            "runtime_moment_present": runtime_moment_present,
            "root_content": root_content
        },
        "self_report": self_report,
        "members": members,
        "internal_relations": internal_relations,
        "boundary_relations": boundary_relations
    });

    let summary = ExportSummary {
        name,
        member_count: member_ids.len(),
        internal_relation_count: document["internal_relations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        boundary_relation_count: document["boundary_relations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        observation_status: observation_status.to_string(),
        revision: index.snapshot.revision.0,
    };
    Ok((document, summary))
}

fn now_unix_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// File naming + writing
// ---------------------------------------------------------------------------

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// `space:l2:mind-universe:construct-validity-v0` -> `construct-validity-v0`.
fn slug(canonical_id: &str) -> String {
    let last = canonical_id.rsplit(':').next().unwrap_or(canonical_id);
    let cleaned = sanitize(last);
    if cleaned.is_empty() {
        "construct".to_string()
    } else {
        cleaned
    }
}

/// Full-id fallback, used when the short slug is already held by a DIFFERENT
/// construct — an export must never silently overwrite another construct.
fn full_slug(canonical_id: &str) -> String {
    sanitize(canonical_id)
}

/// Choose the destination path, refusing to clobber a different construct.
fn destination(exports_dir: &Path, root_id: &str) -> PathBuf {
    let short = exports_dir.join(format!("{}.json", slug(root_id)));
    let held_by_another = fs::read_to_string(&short)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value
                .pointer("/construct/root_id")
                .and_then(Value::as_str)
                .map(|existing| existing != root_id)
        })
        .unwrap_or(false);
    if held_by_another {
        exports_dir.join(format!("{}.json", full_slug(root_id)))
    } else {
        short
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn io<E: std::fmt::Display>(context: &str, error: E) -> E2eError {
    E2eError::Io(format!("{context}: {error}"))
}

/// Write `document` and prove it landed: re-read the file from disk, re-hash it
/// and re-parse it. `readback_verified` is measured, never assumed.
fn write_document(path: &Path, document: &Value) -> Result<(usize, String, bool), E2eError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| io("create exports directory", error))?;
    }
    let mut bytes =
        serde_json::to_vec_pretty(document).map_err(|error| io("serialize export document", error))?;
    bytes.push(b'\n');
    let expected = sha256_hex(&bytes);
    fs::write(path, &bytes).map_err(|error| io("write export document", error))?;

    let observed = fs::read(path).map_err(|error| io("read export document back", error))?;
    let readback_verified =
        sha256_hex(&observed) == expected && serde_json::from_slice::<Value>(&observed).is_ok();
    Ok((bytes.len(), expected, readback_verified))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Export ONE construct, by canonical root id, to `<exports_dir>/<slug>.json`.
///
/// READ-ONLY on the Universe: the only write is the export file itself. The
/// returned receipt reports what was written and whether reading it back
/// reproduced those bytes.
pub fn export_construct(
    store_dir: &Path,
    root_id: &str,
    exports_dir: &Path,
) -> Result<ExportReceipt, E2eError> {
    let index = StoreIndex::open_at(store_dir)?;
    export_from_index(&index, root_id, exports_dir)
}

/// Export EVERY construct in the store (every root whose content declares
/// `contractKind: "construct"`), plus a `constructs.index.json` listing them.
///
/// The store is opened and hydrated once, so the cost is a single pass however
/// many constructs come out of it.
pub fn export_all_constructs(
    store_dir: &Path,
    exports_dir: &Path,
) -> Result<Vec<ExportReceipt>, E2eError> {
    let index = StoreIndex::open_at(store_dir)?;
    let roots = index.construct_roots();

    let mut receipts = Vec::new();
    for root_id in &roots {
        receipts.push(export_from_index(&index, root_id, exports_dir)?);
    }

    let manifest = json!({
        "export_format_version": EXPORT_FORMAT_VERSION,
        "kind": "construct_export_index",
        "observation": {
            "origin": {
                "store": index.store_display,
                "revision": index.snapshot.revision.0,
                "vantage": "read-only replay of the durable store projection"
            },
            "status": "partial",
            "exported_at_unix_s": now_unix_s(),
            "epistemic": {
                "construct_roots": "observed",
                "note": "Roots are the entities whose content declares contractKind=construct in THIS snapshot. A construct absent from this revision is `unknown` here, not `known_absent`."
            }
        },
        "constructs": receipts
    });
    write_document(&exports_dir.join(INDEX_FILE), &manifest)?;

    Ok(receipts)
}

fn export_from_index(
    index: &StoreIndex,
    root_id: &str,
    exports_dir: &Path,
) -> Result<ExportReceipt, E2eError> {
    let (document, summary) = build_document(index, root_id)?;
    let path = destination(exports_dir, root_id);
    let (bytes_written, sha256, readback_verified) = write_document(&path, &document)?;
    Ok(ExportReceipt {
        root_id: root_id.to_string(),
        name: summary.name,
        path,
        store_revision: summary.revision,
        member_count: summary.member_count,
        internal_relation_count: summary.internal_relation_count,
        boundary_relation_count: summary.boundary_relation_count,
        bytes_written,
        sha256,
        readback_verified,
        observation_status: summary.observation_status,
    })
}

/// Every construct root the store currently holds, without exporting anything.
pub fn list_construct_roots(store_dir: &Path) -> Result<Vec<String>, E2eError> {
    Ok(StoreIndex::open_at(store_dir)?.construct_roots())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_takes_the_last_segment() {
        assert_eq!(
            slug("space:l2:mind-universe:construct-validity-v0"),
            "construct-validity-v0"
        );
        assert_eq!(slug("port:l2:sky:authoring-v0"), "authoring-v0");
    }

    #[test]
    fn full_slug_keeps_the_whole_id_addressable() {
        assert_eq!(
            full_slug("space:l2:mind-universe:energy-pen-v0"),
            "space-l2-mind-universe-energy-pen-v0"
        );
    }

    #[test]
    fn discriminator_drops_only_the_kind_prefix() {
        assert_eq!(
            discriminator("space:l2:mind-universe:energy-pen-v0"),
            "l2:mind-universe:energy-pen-v0"
        );
    }

    #[test]
    fn collect_strings_walks_nested_content() {
        let value = json!({"a": "x", "b": ["y", {"c": "z"}], "d": 3});
        let mut out = BTreeSet::new();
        collect_strings(&value, &mut out);
        let expected: BTreeSet<String> = ["x", "y", "z"].into_iter().map(str::to_string).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn readback_is_measured_against_the_bytes_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("doc.json");
        let (bytes, sha, verified) =
            write_document(&path, &json!({"hello": "world"})).expect("write");
        assert!(verified);
        let on_disk = fs::read(&path).expect("read");
        assert_eq!(bytes, on_disk.len());
        assert_eq!(sha, sha256_hex(&on_disk));
    }

    #[test]
    fn destination_never_clobbers_another_construct() {
        let dir = tempfile::tempdir().expect("tempdir");
        let taken = dir.path().join("shared-v0.json");
        fs::write(
            &taken,
            serde_json::to_vec(&json!({"construct": {"root_id": "space:l2:other:shared-v0"}}))
                .expect("serialize"),
        )
        .expect("write");

        // A different construct with the same short slug takes the full-id path.
        assert_eq!(
            destination(dir.path(), "space:l2:mine:shared-v0"),
            dir.path().join("space-l2-mine-shared-v0.json")
        );
        // The same construct reuses its own file, so re-exporting updates it
        // rather than accumulating documents.
        assert_eq!(destination(dir.path(), "space:l2:other:shared-v0"), taken);
    }
}
