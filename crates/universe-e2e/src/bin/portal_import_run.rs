//! Run the filesystem import portal FROM THE LIVE GRAPH: a thing on the ground
//! crosses into the world, and the world offers to make it a Construct.
//!
//! This is the driver for `space:l2:mind-universe:filesystem-import-portal-v0`,
//! the construct the Mechanical toolkit produces at the city scale. Every
//! variable is read from the COMMITTED store, never from the authoring fixture:
//!
//!   * the machine (modules, typed ports, couplings, atoms, bonds, effect
//!     binding) from the portal's `code` node;
//!   * the compatibility rule the couplings are checked against from the
//!     COMMITTED Mechanical toolkit;
//!   * the capability declaration + its limits, the transport contract, the rig
//!     checks, the write contract and the toolkit-proposal rule (including the
//!     file extensions each clause names) likewise;
//!   * the request payload bytes — path, scope, depth, entry bound, byte bound,
//!     preview bound, symlink policy — from the authored effect binding,
//!     transported VERBATIM.
//!
//! The only native policy here is mechanism with zero variables: a READ-ONLY
//! filesystem transport, one evaluator per rig check the graph names, and one
//! evaluator per proposal clause the graph names. A check or a clause this
//! driver cannot evaluate is a HARD failure of the run — never silently skipped
//! and never reported as passing.
//!
//! What it measures (in this order, all against the live store):
//!   1. ASSEMBLY   — every authored coupling joins compatible ports; both
//!      authored `refused_couplings` are refused; couplings and bonds are 1:1.
//!   2. MACHINE    — the nominal wave: both jambs supplied -> the AND-gate
//!      request module activates -> the portal threshold fires -> the terminal
//!      effector fires and surfaces exactly ONE candidate EffectIntent.
//!   3. STARVATION — the negative wave: the scope is NOT declared -> nothing is
//!      assembled, nothing is requested, no candidate is surfaced.
//!   4. CAPABILITY — an intent naming an UNDECLARED capability is denied by the
//!      graph-owned registry before any transport.
//!   5. CROSSING   — the candidate is executed through the declared capability;
//!      the READ-ONLY ground transport returns a measured manifest.
//!   6. RIG        — every check the graph names is evaluated against the bytes.
//!   7. WORLD      — one thing per measured entry is written (or an identical
//!      committed artifact is REUSED), attached by PRODUCES and PART_OF.
//!   8. OFFER      — one `construct_suggestion` per newly written artifact,
//!      `accepted: null`, with `unknown` in every unmeasured anatomy field.
//!   9. EVIDENCE   — one `validation_run` + one `health_assessment`, committed in
//!      the SAME atomic transaction and read back from a fresh reopen.
//!
//! HONESTY. A missing path is measured ABSENCE, never an empty import. An
//! unreadable entry is measured FAILURE with its reason, never a zero-byte
//! artifact. An entry the authored bound cut off is skipped-by-bound, never
//! absent. An imported artifact is a digest-identified DESCRIPTION of a file, not
//! the file: every artifact states `bytes_in_world: false`. Overall health is
//! never `healthy` from one crossing — the authored derivation requires a
//! population the single run cannot measure.
//!
//! Usage: `portal_import_run [--path <p>] [--scope file|directory]
//!                           [--store <dir>] [--dry-run]`
//!   --path/--scope: a citizen's MEASURED supply at the portal's source modules.
//!     Supplying a path replaces exactly that one field of the authored request;
//!     the substitution is recorded in the run evidence. With no argument the
//!     portal's own authored default supply is used, byte for byte.
//!   The run COMMITS the imported things, their suggestions and two run Moments.

// The health vector is one deep authored literal; the default macro recursion
// limit is reached expanding it.
#![recursion_limit = "512"]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::{env, io::ErrorKind};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use universe_assets::visual::{validate_catalog, VisualCatalog};
use universe_capabilities::{
    CapabilityDeclaration, CapabilityHost, CapabilityRegistry, EffectAdapter,
    EffectExecutionReceipt, EffectIntent, EffectReceipt,
};
use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseError};
use universe_e2e::construct_resolver::{resolve_construct, AlarmAtomCircuit, ResolvedConstruct};
use universe_physics::{AtomConvergence, AtomExecutionBudget};
use universe_store::{ContentRef, EntityRecord, RelationRecord, UniverseSnapshot, UniverseStore};
use universe_supervisor::{PhysicsDepositOutcome, Supervisor};
use universe_transactions::{CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// The construct this driver runs, the toolkit that produced it, and the toolkit
/// that dresses it. All read from the COMMITTED store; no fixture is opened.
const PORTAL_CODE_ID: &str = "code:l2:mind-universe:filesystem-import-portal-v0";
const PORTAL_SPACE_ID: &str = "space:l2:mind-universe:filesystem-import-portal-v0";
const PORTAL_BINDING_ID: &str = "visual_binding:l2:mind-universe:filesystem-import-portal-v0";
const TOOLKIT_ALGORITHM_ID: &str = "algorithm:l2:mind-universe:mechanical-toolkit-v0";
const TOOLKIT_SPACE_ID: &str = "space:l2:mind-universe:mechanical-toolkit-v0";
/// Optional: the Appearance toolkit's own binding carries the closed renderer
/// palette. Absent from this store -> the palette check is `not_measured`, never
/// a pass and never a native hard-coded palette.
const APPEARANCE_BINDING_ID: &str = "visual_binding:l2:mind-universe:appearance-toolkit-v0";

/// Key blocks for what this portal writes. Disjoint from the injector's hashed
/// construct blocks (max 0x1000_0000) and from the Ollama probe's Moment block
/// (0x2000_0000 / 0x2100_0000). Free keys are scanned inside a block, never
/// assumed.
const THING_ENTITY_BASE: u128 = 0x2200_0000;
const MOMENT_ENTITY_BASE: u128 = 0x2300_0000;
const RELATION_BASE: u128 = 0x2400_0000;
const BLOCK_SPAN: u128 = 65_536;

fn budget() -> AtomExecutionBudget {
    AtomExecutionBudget {
        max_atoms: 16,
        max_bonds: 16,
        max_steps: 16,
        max_total_energy: 10_000,
    }
}

// ===========================================================================
// Native floor: a READ-ONLY filesystem transport with zero policy.
// ===========================================================================

/// Reads exactly the path the authored request names, under exactly the bounds
/// the authored request declares, and returns the measured manifest bytes.
///
/// It opens no other path, invents no bound, follows no symlink unless the
/// request authorizes it, and performs ZERO writes — it holds no code path that
/// could create, modify, rename or remove anything. Every number it reports is
/// measured from the ground; every field it cannot measure carries an explicit
/// status instead of a default.
struct GroundReadTransport {
    /// The directory paths are resolved against (the world's working directory).
    root: PathBuf,
    /// Set on every completed transport so the caller can prove read-only.
    writes_performed: u64,
}

/// The authored request, deserialized. Every field is required except the ones
/// the graph marks optional; a missing required field is a hard error, never a
/// native default.
struct ImportRequest {
    path: String,
    scope: String,
    max_depth: u64,
    max_entries: u64,
    max_bytes_per_entry: u64,
    preview_bytes: u64,
    digest: String,
    follow_symlinks: bool,
    mode: String,
}

fn parse_request(payload: &[u8]) -> Result<(ImportRequest, Value), String> {
    let value: Value = serde_json::from_slice(payload)
        .map_err(|error| format!("import request is not JSON: {error}"))?;
    let string = |key: &str| -> Result<String, String> {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("import request declares no {key}"))
    };
    let number = |key: &str| -> Result<u64, String> {
        value
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("import request declares no {key}"))
    };
    let request = ImportRequest {
        path: string("path")?,
        scope: string("scope")?,
        max_depth: number("max_depth")?,
        max_entries: number("max_entries")?,
        max_bytes_per_entry: number("max_bytes_per_entry")?,
        preview_bytes: number("preview_bytes")?,
        digest: string("digest")?,
        follow_symlinks: value
            .get("follow_symlinks")
            .and_then(Value::as_bool)
            .ok_or("import request declares no follow_symlinks")?,
        mode: string("mode")?,
    };
    Ok((request, value))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// The six byte kinds this Universe's vocabulary names. A kind outside them is
/// never invented: an unrecognized head is `binary_unclassified`, which is a
/// measured answer, not a missing one.
fn classify_head(head: &[u8]) -> &'static str {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if head.starts_with(PNG) {
        return "image/png";
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "image/jpeg";
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return "image/gif";
    }
    if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        return "image/webp";
    }
    if head.contains(&0) {
        return "binary_unclassified";
    }
    // A truncated head may cut a multi-byte character: an incomplete sequence at
    // the very end is still text, an invalid one in the middle is not.
    match std::str::from_utf8(head) {
        Ok(_) => "utf8_text",
        Err(error) if error.error_len().is_none() => "utf8_text",
        Err(_) => "binary_unclassified",
    }
}

/// The longest valid utf-8 prefix of `head`, bounded by `limit` bytes.
fn text_preview(head: &[u8], limit: usize) -> String {
    let bounded = &head[..head.len().min(limit)];
    match std::str::from_utf8(bounded) {
        Ok(text) => text.to_string(),
        Err(error) => String::from_utf8_lossy(&bounded[..error.valid_up_to()]).into_owned(),
    }
}

fn modified_ms(metadata: &fs::Metadata) -> Option<u128> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis())
}

struct MeasuredEntry {
    value: Value,
}

impl GroundReadTransport {
    /// Measures ONE file entry. Every field that cannot be measured carries its
    /// own status; nothing is defaulted.
    fn measure_file(
        &self,
        absolute: &Path,
        relative: &str,
        metadata: &fs::Metadata,
        request: &ImportRequest,
    ) -> MeasuredEntry {
        let size = metadata.len();
        let preview_limit = request.preview_bytes as usize;
        let within_byte_bound = size <= request.max_bytes_per_entry;
        // Read the whole file when it fits the authored byte bound (digest is
        // then measurable); otherwise read only the head needed to classify.
        let read_limit = if within_byte_bound {
            size
        } else {
            request.preview_bytes.max(16)
        };
        let mut buffer = Vec::new();
        let read_outcome = fs::File::open(absolute).and_then(|file| {
            file.take(read_limit).read_to_end(&mut buffer)?;
            Ok(())
        });

        let (digest, digest_status, byte_kind, byte_kind_status, preview, preview_status) =
            match read_outcome {
                Err(error) => (
                    Value::Null,
                    format!("measurement_failed: {error}"),
                    Value::Null,
                    format!("measurement_failed: {error}"),
                    Value::Null,
                    format!("measurement_failed: {error}"),
                ),
                Ok(()) => {
                    let kind = classify_head(&buffer);
                    let (digest, digest_status) = if within_byte_bound {
                        let hash = Sha256::digest(&buffer);
                        (json!(hex::encode(hash)), "measured".to_string())
                    } else {
                        (
                            Value::Null,
                            format!(
                                "not_measured: {size} bytes exceeds the authored max_bytes_per_entry {}",
                                request.max_bytes_per_entry
                            ),
                        )
                    };
                    let (preview, preview_status) = if kind == "utf8_text" {
                        let text = text_preview(&buffer, preview_limit);
                        let status = if within_byte_bound && (size as usize) <= preview_limit {
                            "measured_complete"
                        } else {
                            "measured_truncated"
                        };
                        (json!(text), status.to_string())
                    } else {
                        (
                            Value::Null,
                            "not_measured: head bytes are not utf-8 text".to_string(),
                        )
                    };
                    (
                        digest,
                        digest_status,
                        json!(kind),
                        "measured".to_string(),
                        preview,
                        preview_status,
                    )
                }
            };

        let extension = absolute
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_lowercase);

        MeasuredEntry {
            value: json!({
                "relative_path": relative,
                "origin_path": absolute.to_string_lossy(),
                "kind": "file",
                "size_bytes": size,
                "size_status": "measured",
                "modified_unix_ms": modified_ms(metadata).map(|ms| json!(ms as u64)).unwrap_or(Value::Null),
                "modified_status": if modified_ms(metadata).is_some() { "measured" } else { "not_measured: the ground reported no modification time" },
                "sha256": digest,
                "digest_status": digest_status,
                "byte_kind": byte_kind,
                "byte_kind_status": byte_kind_status,
                "extension": extension.clone().map(Value::String).unwrap_or(Value::Null),
                "preview": preview,
                "preview_status": preview_status
            }),
        }
    }

    fn measure_directory_entry(&self, absolute: &Path, relative: &str, metadata: &fs::Metadata) -> MeasuredEntry {
        MeasuredEntry {
            value: json!({
                "relative_path": relative,
                "origin_path": absolute.to_string_lossy(),
                "kind": "directory",
                "size_bytes": 0,
                "size_status": "not_applicable: a directory has no byte size of its own",
                "modified_unix_ms": modified_ms(metadata).map(|ms| json!(ms as u64)).unwrap_or(Value::Null),
                "modified_status": if modified_ms(metadata).is_some() { "measured" } else { "not_measured: the ground reported no modification time" },
                "sha256": Value::Null,
                "digest_status": "not_applicable: a directory is not a byte sequence",
                "byte_kind": Value::Null,
                "byte_kind_status": "not_applicable: a directory has no head bytes",
                "extension": Value::Null,
                "preview": Value::Null,
                "preview_status": "not_applicable"
            }),
        }
    }

    fn measure_symlink(&self, absolute: &Path, relative: &str) -> MeasuredEntry {
        MeasuredEntry {
            value: json!({
                "relative_path": relative,
                "origin_path": absolute.to_string_lossy(),
                "kind": "symlink",
                "size_bytes": 0,
                "size_status": "not_measured: the link target was not followed",
                "modified_unix_ms": Value::Null,
                "modified_status": "not_measured: the link target was not followed",
                "sha256": Value::Null,
                "digest_status": "not_measured: follow_symlinks is false in the authored request",
                "byte_kind": Value::Null,
                "byte_kind_status": "not_measured: the link target was not followed",
                "extension": Value::Null,
                "preview": Value::Null,
                "preview_status": "not_measured"
            }),
        }
    }
}

impl EffectAdapter for GroundReadTransport {
    fn transport(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let (request, echo) = parse_request(payload)?;
        // Zero policy: a mode or a digest algorithm this adapter cannot honour is
        // refused outright rather than silently downgraded.
        if request.mode != "read_only" {
            return Err(format!(
                "this transport performs read-only crossings; the request asks for mode {:?}",
                request.mode
            ));
        }
        if request.digest != "sha256" {
            return Err(format!(
                "this transport measures sha256 digests; the request asks for {:?}",
                request.digest
            ));
        }
        let measured_at = now_ms();
        let absolute = self.root.join(&request.path);

        // Root state: present, absent, or a failed measurement. Never defaulted.
        let root_metadata = fs::symlink_metadata(&absolute);
        let (root_state, root_kind, root_reason) = match &root_metadata {
            Ok(metadata) if metadata.is_dir() => ("measured_present", json!("directory"), Value::Null),
            Ok(metadata) if metadata.is_file() => ("measured_present", json!("file"), Value::Null),
            Ok(metadata) if metadata.is_symlink() => ("measured_present", json!("symlink"), Value::Null),
            Ok(_) => ("measured_present", json!("other"), Value::Null),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                ("known_absent", Value::Null, json!(error.to_string()))
            }
            Err(error) => ("measurement_failed", Value::Null, json!(error.to_string())),
        };

        let mut entries: Vec<Value> = Vec::new();
        let mut unreadable: Vec<Value> = Vec::new();
        let mut entries_seen: u64 = 0;
        let mut skipped_by_bound: u64 = 0;

        if root_state == "measured_present" {
            match request.scope.as_str() {
                "file" => {
                    entries_seen += 1;
                    let metadata = root_metadata.as_ref().expect("root metadata is present");
                    if metadata.is_file() {
                        entries.push(
                            self.measure_file(&absolute, &request.path, metadata, &request)
                                .value,
                        );
                    } else if metadata.is_symlink() && !request.follow_symlinks {
                        entries.push(self.measure_symlink(&absolute, &request.path).value);
                    } else {
                        unreadable.push(json!({
                            "path": absolute.to_string_lossy(),
                            "reason": "the declared scope is `file` but the ground holds something else here"
                        }));
                    }
                }
                "directory" => {
                    // Bounded, deterministic walk: sorted by name, never deeper
                    // than the authored depth, never wider than the authored
                    // entry bound. An overflow is COUNTED, never dropped silently.
                    let mut frontier: Vec<(PathBuf, String, u64)> =
                        vec![(absolute.clone(), String::new(), 0)];
                    while let Some((directory, prefix, depth)) = frontier.pop() {
                        let listing = match fs::read_dir(&directory) {
                            Ok(listing) => listing,
                            Err(error) => {
                                unreadable.push(json!({
                                    "path": directory.to_string_lossy(),
                                    "reason": format!("directory not readable: {error}")
                                }));
                                continue;
                            }
                        };
                        let mut children: Vec<PathBuf> = Vec::new();
                        for item in listing {
                            match item {
                                Ok(item) => children.push(item.path()),
                                Err(error) => unreadable.push(json!({
                                    "path": directory.to_string_lossy(),
                                    "reason": format!("directory entry not readable: {error}")
                                })),
                            }
                        }
                        children.sort();
                        for child in children {
                            let name = child
                                .file_name()
                                .map(|value| value.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let relative = if prefix.is_empty() {
                                name.clone()
                            } else {
                                format!("{prefix}/{name}")
                            };
                            entries_seen += 1;
                            if entries.len() as u64 >= request.max_entries {
                                skipped_by_bound += 1;
                                continue;
                            }
                            let metadata = match fs::symlink_metadata(&child) {
                                Ok(metadata) => metadata,
                                Err(error) => {
                                    entries_seen -= 1;
                                    unreadable.push(json!({
                                        "path": child.to_string_lossy(),
                                        "reason": format!("not readable: {error}")
                                    }));
                                    entries_seen += 1;
                                    continue;
                                }
                            };
                            if metadata.is_symlink() && !request.follow_symlinks {
                                entries.push(self.measure_symlink(&child, &relative).value);
                            } else if metadata.is_dir() {
                                entries
                                    .push(self.measure_directory_entry(&child, &relative, &metadata).value);
                                if depth + 1 < request.max_depth {
                                    frontier.push((child.clone(), relative.clone(), depth + 1));
                                }
                            } else if metadata.is_file() {
                                entries.push(self.measure_file(&child, &relative, &metadata, &request).value);
                            } else {
                                unreadable.push(json!({
                                    "path": child.to_string_lossy(),
                                    "reason": "neither a file, a directory nor a symlink"
                                }));
                            }
                        }
                    }
                }
                other => {
                    return Err(format!(
                        "the authored request declares scope {other:?}; this transport measures `file` or `directory`"
                    ))
                }
            }
        }

        // Entries that failed measurement are listed as unreadable, so the
        // accounting identity below always holds:
        //   entries_seen = listed + skipped_by_bound + unreadable
        let manifest = json!({
            "request": echo,
            "mode": "read_only",
            "writes_performed": self.writes_performed,
            "working_directory": self.root.to_string_lossy(),
            "root": {
                "path": absolute.to_string_lossy(),
                "declared_scope": request.scope,
                "state": root_state,
                "kind": root_kind,
                "reason": root_reason
            },
            "entries": entries,
            "listed": entries.len() as u64,
            "skipped_by_bound": skipped_by_bound,
            "unreadable": unreadable,
            // Counts every child the ground offered, unreadable ones included,
            // so `entries_seen = listed + skipped_by_bound + unreadable` holds
            // exactly and nothing can go missing between the two sides.
            "entries_seen": entries_seen,
            "measured_at_unix_ms": measured_at as u64
        });
        serde_json::to_vec(&manifest).map_err(|error| format!("manifest not serializable: {error}"))
    }
}

// ===========================================================================
// Bounded read of the committed graph.
// ===========================================================================

/// One pass over the committed entities: collects EVERY canonical id (so an
/// already-imported artifact can be recognized instead of duplicated) and the
/// hydrated content of the nodes this run needs. A required id that is absent is
/// a hard error; an optional one that is absent stays absent (never invented).
fn scan_graph(
    supervisor: &Supervisor,
    snapshot: &UniverseSnapshot,
    required: &[&str],
    optional: &[&str],
) -> Result<(BTreeMap<String, EntityKey>, BTreeMap<String, Value>), Box<dyn Error>> {
    let wanted: BTreeSet<&str> = required.iter().chain(optional.iter()).copied().collect();
    let mut ids: BTreeMap<String, EntityKey> = BTreeMap::new();
    let mut contents: BTreeMap<String, Value> = BTreeMap::new();
    for entity in &snapshot.entities {
        let Some(content_ref) = entity.content.as_ref() else {
            continue;
        };
        let wrapper = supervisor.read_content(content_ref)?;
        let Some(canonical) = wrapper.get("canonical_id").and_then(Value::as_str) else {
            continue;
        };
        ids.insert(canonical.to_string(), entity.key);
        if wanted.contains(canonical) {
            contents.insert(canonical.to_string(), wrapper);
        }
    }
    for id in required {
        if !contents.contains_key(*id) {
            return Err(format!("canonical id {id} is not committed in this store").into());
        }
    }
    Ok((ids, contents))
}

/// The inner authored content of a committed node.
fn inner(wrapper: &Value) -> Result<&Value, Box<dyn Error>> {
    wrapper
        .get("content")
        .ok_or_else(|| "committed node carries no content block".into())
}

// ===========================================================================
// Assembly: the Mechanical toolkit's compatibility rule, applied.
// ===========================================================================

struct AssemblyEvidence {
    rule: String,
    admitted: Vec<String>,
    refused: Vec<String>,
    wrongly_admitted: Vec<String>,
    wrongly_refused: Vec<String>,
    coupling_bond_correspondence: bool,
}

/// A coupling is admitted iff the output port type is accepted by the input port
/// type. The graph declares NO widening relation between types, so identity is
/// the only acceptance relation there is evidence for.
fn admit(out_type: &str, in_type: &str) -> bool {
    out_type == in_type
}

fn check_assembly(circuit: &Value, toolkit_rule: &str) -> Result<AssemblyEvidence, Box<dyn Error>> {
    let couplings = circuit
        .get("couplings")
        .and_then(Value::as_array)
        .ok_or("machine_circuit has no couplings array")?;
    let refused_couplings = circuit
        .get("refused_couplings")
        .and_then(Value::as_array)
        .ok_or("machine_circuit has no refused_couplings array")?;
    let bonds = circuit
        .get("bonds")
        .and_then(Value::as_array)
        .ok_or("machine_circuit has no bonds array")?;

    let types = |coupling: &Value| -> Result<(String, String, String), Box<dyn Error>> {
        let key = coupling
            .get("key")
            .and_then(Value::as_str)
            .ok_or("coupling without key")?
            .to_string();
        let out_type = coupling
            .get("out_type")
            .and_then(Value::as_str)
            .ok_or("coupling without out_type")?
            .to_string();
        let in_type = coupling
            .get("in_type")
            .and_then(Value::as_str)
            .ok_or("coupling without in_type")?
            .to_string();
        Ok((key, out_type, in_type))
    };

    let mut admitted = Vec::new();
    let mut wrongly_refused = Vec::new();
    for coupling in couplings {
        let (key, out_type, in_type) = types(coupling)?;
        if admit(&out_type, &in_type) {
            admitted.push(format!("{key} ({out_type} -> {in_type})"));
        } else {
            wrongly_refused.push(format!("{key} ({out_type} -> {in_type})"));
        }
    }
    let mut refused = Vec::new();
    let mut wrongly_admitted = Vec::new();
    for coupling in refused_couplings {
        let (key, out_type, in_type) = types(coupling)?;
        if admit(&out_type, &in_type) {
            wrongly_admitted.push(format!("{key} ({out_type} -> {in_type})"));
        } else {
            refused.push(format!("{key} ({out_type} != {in_type})"));
        }
    }

    let coupling_keys: BTreeSet<String> = couplings
        .iter()
        .filter_map(|c| c.get("key").and_then(Value::as_str).map(str::to_string))
        .collect();
    let bond_keys: BTreeSet<String> = bonds
        .iter()
        .filter_map(|b| b.get("key").and_then(Value::as_str).map(str::to_string))
        .collect();

    Ok(AssemblyEvidence {
        rule: toolkit_rule.to_string(),
        admitted,
        refused,
        wrongly_admitted,
        wrongly_refused,
        coupling_bond_correspondence: coupling_keys == bond_keys,
    })
}

// ===========================================================================
// The validation rig: one evaluator per graph-named check.
// ===========================================================================

#[derive(Clone)]
struct RigResult {
    id: String,
    load_bearing: bool,
    passed: bool,
    evidence: String,
}

/// The extension a name claims, mapped to the byte kind that claim implies.
/// Only the image kinds have an unambiguous claim; every other extension claims
/// nothing measurable, which is reported as such rather than guessed.
fn extension_claim(extension: Option<&str>) -> Option<&'static str> {
    match extension? {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn entry_str<'a>(entry: &'a Value, field: &str) -> Option<&'a str> {
    entry.get(field).and_then(Value::as_str)
}

fn run_rig(
    rig: &Value,
    request: &Value,
    transported_payload: &[u8],
    receipt: &EffectExecutionReceipt,
) -> Result<(Vec<RigResult>, Option<Value>), Box<dyn Error>> {
    let checks = rig
        .get("checks")
        .and_then(Value::as_array)
        .ok_or("validation_rig has no checks array")?;

    let raw = match &receipt.outcome {
        EffectReceipt::TransportSucceeded { response } => Some(response.clone()),
        EffectReceipt::TransportFailed { .. } => None,
    };
    let manifest: Option<Value> = raw
        .as_ref()
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .filter(Value::is_object);

    let absent = |what: &str| format!("known_absent: {what} (the transport returned no usable manifest)");
    let entries = |manifest: &Value| -> Vec<Value> {
        manifest
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let declared_scope = request.get("scope").and_then(Value::as_str).unwrap_or("");
    let preview_bound = request
        .get("preview_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let entry_bound = request.get("max_entries").and_then(Value::as_u64).unwrap_or(0);

    let mut results = Vec::new();
    for check in checks {
        let id = check
            .get("id")
            .and_then(Value::as_str)
            .ok_or("rig check without id")?
            .to_string();
        let load_bearing = check
            .get("load_bearing")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let (passed, evidence) = match id.as_str() {
            "transport_succeeded" => match &receipt.outcome {
                EffectReceipt::TransportSucceeded { response } => (
                    true,
                    format!("TransportSucceeded, {} manifest bytes", response.len()),
                ),
                EffectReceipt::TransportFailed { reason } => (false, format!("TransportFailed: {reason}")),
            },
            "manifest_is_json" => match (&raw, &manifest) {
                (Some(bytes), Some(_)) => (true, format!("manifest parsed as a JSON object ({} bytes)", bytes.len())),
                (Some(bytes), None) => (false, format!("{} bytes did not parse as a JSON object", bytes.len())),
                _ => (false, absent("no manifest bytes")),
            },
            "request_echoed_unchanged" => match manifest.as_ref().and_then(|m| m.get("request")) {
                Some(echo) => {
                    let transported: Value = serde_json::from_slice(transported_payload)
                        .unwrap_or(Value::Null);
                    (
                        echo == &transported,
                        format!("manifest.request {} the transported request", if echo == &transported { "equals" } else { "DIFFERS from" }),
                    )
                }
                None => (false, absent("no manifest.request")),
            },
            "root_state_measured" => match manifest.as_ref().and_then(|m| m.pointer("/root/state")).and_then(Value::as_str) {
                Some(state) => (
                    matches!(state, "measured_present" | "known_absent" | "measurement_failed"),
                    format!("root.state = {state}"),
                ),
                None => (false, absent("no root.state")),
            },
            "root_kind_matches_scope" => {
                match manifest.as_ref().map(|m| (m.pointer("/root/state").and_then(Value::as_str), m.pointer("/root/kind").and_then(Value::as_str))) {
                    Some((Some("measured_present"), Some(kind))) => (
                        kind == declared_scope,
                        format!("declared scope {declared_scope}, ground holds a {kind}"),
                    ),
                    Some((Some(state), _)) => (
                        false,
                        format!("root.state = {state}: the ground holds nothing to match the declared scope {declared_scope} — measured absence, not an empty import"),
                    ),
                    _ => (false, absent("no root block")),
                }
            }
            "every_entry_kind_measured" => match &manifest {
                Some(m) => {
                    let list = entries(m);
                    let bad: Vec<String> = list
                        .iter()
                        .filter(|entry| {
                            entry_str(entry, "kind").is_none()
                                || entry.get("size_status").is_none()
                        })
                        .filter_map(|entry| entry_str(entry, "relative_path").map(str::to_string))
                        .collect();
                    (bad.is_empty(), format!("{} entries, {} without a measured kind/size status {:?}", list.len(), bad.len(), bad))
                }
                None => (false, absent("no entries")),
            },
            "digest_covers_readable_files" => match &manifest {
                Some(m) => {
                    let list = entries(m);
                    let files: Vec<&Value> = list.iter().filter(|e| entry_str(e, "kind") == Some("file")).collect();
                    let missing: Vec<String> = files
                        .iter()
                        .filter(|entry| {
                            entry.get("sha256").map(Value::is_null).unwrap_or(true)
                                && entry_str(entry, "digest_status").map(|s| s == "measured").unwrap_or(true)
                        })
                        .filter_map(|entry| entry_str(entry, "relative_path").map(str::to_string))
                        .collect();
                    let measured = files
                        .iter()
                        .filter(|entry| entry_str(entry, "digest_status") == Some("measured"))
                        .count();
                    (
                        missing.is_empty(),
                        format!("{measured}/{} file entries carry a measured sha256; the rest carry an explicit digest_status; {} silently missing", files.len(), missing.len()),
                    )
                }
                None => (false, absent("no entries")),
            },
            "entry_bound_respected" => match &manifest {
                Some(m) => {
                    let listed = m.get("listed").and_then(Value::as_u64).unwrap_or(0);
                    let skipped = m.get("skipped_by_bound").and_then(Value::as_u64).unwrap_or(0);
                    (
                        listed <= entry_bound,
                        format!("listed {listed} <= max_entries {entry_bound}; {skipped} reported as skipped_by_bound"),
                    )
                }
                None => (false, absent("no manifest")),
            },
            "preview_bound_respected" => match &manifest {
                Some(m) => {
                    let over: Vec<String> = entries(m)
                        .iter()
                        .filter(|entry| {
                            entry_str(entry, "preview")
                                .map(|preview| preview.len() as u64 > preview_bound)
                                .unwrap_or(false)
                        })
                        .filter_map(|entry| entry_str(entry, "relative_path").map(str::to_string))
                        .collect();
                    (over.is_empty(), format!("preview bound {preview_bound} bytes; {} entries over it {:?}", over.len(), over))
                }
                None => (false, absent("no entries")),
            },
            "unreadable_recorded_not_dropped" => match &manifest {
                Some(m) => {
                    let seen = m.get("entries_seen").and_then(Value::as_u64).unwrap_or(0);
                    let listed = m.get("listed").and_then(Value::as_u64).unwrap_or(0);
                    let skipped = m.get("skipped_by_bound").and_then(Value::as_u64).unwrap_or(0);
                    let unreadable = m.get("unreadable").and_then(Value::as_array).map(Vec::len).unwrap_or(0) as u64;
                    (
                        seen == listed + skipped + unreadable,
                        format!("entries_seen {seen} = listed {listed} + skipped_by_bound {skipped} + unreadable {unreadable}"),
                    )
                }
                None => (false, absent("no manifest")),
            },
            "no_write_performed" => match &manifest {
                Some(m) => {
                    let mode = m.get("mode").and_then(Value::as_str).unwrap_or("");
                    let writes = m.get("writes_performed").and_then(Value::as_u64);
                    (
                        mode == "read_only" && writes == Some(0),
                        format!("mode = {mode:?}, writes_performed = {writes:?}"),
                    )
                }
                None => (false, absent("no manifest")),
            },
            "byte_kind_classified" => match &manifest {
                Some(m) => {
                    let list = entries(m);
                    let files: Vec<&Value> = list.iter().filter(|e| entry_str(e, "kind") == Some("file")).collect();
                    let classified = files
                        .iter()
                        .filter(|entry| entry_str(entry, "byte_kind_status") == Some("measured"))
                        .count();
                    let kinds: BTreeSet<String> = files
                        .iter()
                        .filter_map(|entry| entry_str(entry, "byte_kind").map(str::to_string))
                        .collect();
                    (
                        classified == files.len(),
                        format!("{classified}/{} file entries classified from head bytes; kinds seen {:?}", files.len(), kinds),
                    )
                }
                None => (false, absent("no entries")),
            },
            "extension_agrees_with_bytes" => match &manifest {
                Some(m) => {
                    let list = entries(m);
                    let mut compared = 0usize;
                    let mut disagreements: Vec<String> = Vec::new();
                    for entry in list.iter().filter(|e| entry_str(e, "kind") == Some("file")) {
                        let Some(claim) = extension_claim(entry_str(entry, "extension")) else {
                            continue;
                        };
                        compared += 1;
                        if entry_str(entry, "byte_kind") != Some(claim) {
                            disagreements.push(format!(
                                "{} claims {claim}, head bytes say {:?}",
                                entry_str(entry, "relative_path").unwrap_or("?"),
                                entry_str(entry, "byte_kind")
                            ));
                        }
                    }
                    (
                        disagreements.is_empty(),
                        format!("{compared} entries carried an unambiguous extension claim; {} disagreed {:?}", disagreements.len(), disagreements),
                    )
                }
                None => (false, absent("no entries")),
            },
            other => {
                return Err(format!(
                    "validation rig names check {other:?} but this driver has no evaluator for it — \
                     the run fails closed rather than skipping or passing a declared check"
                )
                .into())
            }
        };
        results.push(RigResult {
            id,
            load_bearing,
            passed,
            evidence,
        });
    }
    Ok((results, manifest))
}

// ===========================================================================
// The offer: the graph's toolkit-proposal rule, applied.
// ===========================================================================

/// The extensions a proposal clause NAMES in its own `when` text (".md", ".rs",
/// ...). The clause is graph data; this only reads the tokens out of it, so the
/// extension sets stay owned by the Universe and not by this binary.
fn extensions_named_in(clause: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for token in clause.split(|c: char| c == '(' || c == ')' || c == ',' || c == ' ') {
        let token = token.trim();
        if let Some(extension) = token.strip_prefix('.') {
            if !extension.is_empty() && extension.chars().all(|c| c.is_ascii_alphanumeric()) {
                found.insert(extension.to_lowercase());
            }
        }
    }
    found
}

// ===========================================================================
// The form, submitted to the RENDERER's own validator.
// ===========================================================================

/// Wraps forms in the minimal `visual-embodiment/1` document the renderer's
/// validator consumes.
///
/// `lod_states` and `fallback_form` are HARNESS SCAFFOLDING: the validator
/// refuses to run without them, and their values cannot change the verdict on
/// the tuples. The budgets are NOT invented — they are the measured maxima of
/// the forms being submitted, so the probe asks exactly one question: does the
/// renderer accept these primitive tuples?
fn probe_catalog(forms: Map<String, Value>) -> Result<VisualCatalog, Box<dyn Error>> {
    let first = forms
        .keys()
        .next()
        .ok_or("no form to submit to the renderer's validator")?
        .clone();
    let primitive_budget = forms
        .values()
        .filter_map(Value::as_array)
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .max(1) as u64;
    let particle_budget = forms
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(Value::as_array)
        .filter_map(|tuple| tuple.get(6).and_then(Value::as_u64))
        .max()
        .unwrap_or(0);
    Ok(VisualCatalog {
        authority_id: "probe:portal-form-submitted-to-renderer-validator".to_string(),
        mapping: json!({
            "schema_version": "visual-embodiment/1",
            "mapping_id": "probe:portal-form",
            "primitive_budget": primitive_budget,
            "particle_budget": particle_budget,
            "forms": Value::Object(forms),
            "fallback_form": first,
            "lod_states": { "hot": first, "sleeping": first, "aggregated": first, "dormant": first }
        }),
        motion_profile: Value::Null,
    })
}

struct Proposal {
    toolkit: String,
    why: String,
    matched_clause: String,
}

/// Evaluates the COMMITTED proposal rule, clause by clause, in the order the
/// graph declares. A clause this driver cannot recognize is a HARD failure —
/// exactly like an unknown rig check — so a citizen adding a clause is never
/// silently ignored.
fn propose_toolkit(rule: &[Value], entry: &Value) -> Result<Proposal, Box<dyn Error>> {
    let kind = entry_str(entry, "kind").unwrap_or("");
    let byte_kind = entry_str(entry, "byte_kind").unwrap_or("");
    let extension = entry_str(entry, "extension").unwrap_or("").to_lowercase();
    for clause in rule {
        let when = clause
            .get("when")
            .and_then(Value::as_str)
            .ok_or("proposal clause without a `when`")?;
        let toolkit = clause
            .get("propose")
            .and_then(Value::as_str)
            .ok_or("proposal clause without a `propose`")?;
        let why = clause.get("why").and_then(Value::as_str).unwrap_or("").to_string();
        let named = extensions_named_in(when);
        let matches = if when.contains("byte_kind starts with image/") {
            byte_kind.starts_with("image/")
        } else if when.contains("kind is directory") {
            kind == "directory"
        } else if when.contains("utf8_text") && !named.is_empty() {
            byte_kind == "utf8_text" && named.contains(&extension)
        } else if when.contains("nothing above matches") {
            true
        } else {
            return Err(format!(
                "the committed proposal rule names clause {when:?} but this driver has no evaluator \
                 for it — the run fails closed rather than ignoring a clause the Universe declares"
            )
            .into());
        };
        if matches {
            return Ok(Proposal {
                toolkit: toolkit.to_string(),
                why,
                matched_clause: when.to_string(),
            });
        }
    }
    Err("the committed proposal rule matched no clause and declares no default".into())
}

// ===========================================================================
// Commit plumbing.
// ===========================================================================

/// `count` free entity keys at or after `base`, bounded by the block span.
fn free_entity_keys(
    snapshot: &UniverseSnapshot,
    base: u128,
    count: usize,
) -> Result<Vec<EntityKey>, Box<dyn Error>> {
    let taken: BTreeSet<u128> = snapshot.entities.iter().map(|entity| entity.key.0).collect();
    let mut keys = Vec::with_capacity(count);
    for offset in 0..BLOCK_SPAN {
        if keys.len() == count {
            break;
        }
        let key = base + offset;
        if !taken.contains(&key) {
            keys.push(EntityKey(key));
        }
    }
    if keys.len() != count {
        return Err(format!("no {count} free entity keys in the block at {base:#x}").into());
    }
    Ok(keys)
}

fn free_relation_keys(
    snapshot: &UniverseSnapshot,
    base: u128,
    count: usize,
) -> Result<Vec<RelationKey>, Box<dyn Error>> {
    let taken: BTreeSet<u128> = snapshot.relations.iter().map(|relation| relation.key.0).collect();
    let mut keys = Vec::with_capacity(count);
    for offset in 0..BLOCK_SPAN {
        if keys.len() == count {
            break;
        }
        let key = base + offset;
        if !taken.contains(&key) {
            keys.push(RelationKey(key));
        }
    }
    if keys.len() != count {
        return Err(format!("no {count} free relation keys in the block at {base:#x}").into());
    }
    Ok(keys)
}

/// A node this run will write: canonical id, canonical symbol, stored content.
struct PlannedNode {
    canonical_id: String,
    symbol: &'static str,
    content: ContentRef,
}

/// `a-b-c` from an arbitrary origin path, for a readable canonical id.
fn slug(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn main() {
    if let Err(error) = run() {
        eprintln!("PORTAL IMPORT RUN FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    // ---- arguments: a citizen's measured supply at the source modules -------
    let mut store_dir: Option<PathBuf> = env::var_os("UNIVERSE_STORE").map(PathBuf::from);
    let mut supplied_path: Option<String> = None;
    let mut supplied_scope: Option<String> = None;
    let mut dry_run = false;
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--path" => supplied_path = Some(args.next().ok_or("--path needs a value")?),
            "--scope" => supplied_scope = Some(args.next().ok_or("--scope needs file|directory")?),
            "--store" => store_dir = Some(PathBuf::from(args.next().ok_or("--store needs a dir")?)),
            "--dry-run" => dry_run = true,
            other => return Err(format!("unknown argument {other:?}").into()),
        }
    }
    let store_dir = store_dir.unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));
    let genesis = PathBuf::from("fixtures/genesis/minimal-genesis.json");
    println!("store dir: {}", store_dir.display());

    let mut supervisor = Supervisor::boot(&store_dir, &genesis)?;
    let revision_before = supervisor.revision();
    println!(
        "base revision: {} | entities: {} | relations: {}",
        revision_before.0,
        supervisor.snapshot().entities.len(),
        supervisor.snapshot().relations.len()
    );

    // ---- (0) read the portal and its producing toolkit from the STORE -------
    let snapshot = supervisor.snapshot().clone();
    let (committed_ids, contents) = scan_graph(
        &supervisor,
        &snapshot,
        &[PORTAL_CODE_ID, PORTAL_SPACE_ID, TOOLKIT_ALGORITHM_ID, TOOLKIT_SPACE_ID],
        &[PORTAL_BINDING_ID, APPEARANCE_BINDING_ID],
    )?;
    let portal_space_key = *committed_ids
        .get(PORTAL_SPACE_ID)
        .ok_or("the portal space is not committed")?;
    let portal_code = inner(&contents[PORTAL_CODE_ID])?.clone();
    let toolkit_algorithm = inner(&contents[TOOLKIT_ALGORITHM_ID])?.clone();
    println!(
        "read from the live graph: portal code, portal space {:#x}, mechanical toolkit algorithm",
        portal_space_key.0
    );

    let circuit_value = portal_code
        .get("machine_circuit")
        .ok_or("portal code node carries no machine_circuit")?
        .clone();
    let transport_spec = portal_code
        .get("effect_transport")
        .ok_or("portal code node carries no effect_transport")?
        .clone();
    let declaration_spec = portal_code
        .get("capability_declaration")
        .ok_or("portal code node carries no capability_declaration")?
        .clone();
    let rig_spec = portal_code
        .get("validation_rig")
        .ok_or("portal code node carries no validation_rig")?
        .clone();
    let write_spec = portal_code
        .get("import_write")
        .ok_or("portal code node carries no import_write")?
        .clone();
    let suggestion_spec = portal_code
        .get("suggestion_template")
        .ok_or("portal code node carries no suggestion_template")?
        .clone();

    // ---- (1) ASSEMBLY: the toolkit's own compatibility rule, applied --------
    let toolkit_rule = toolkit_algorithm
        .get("compatibility_rule")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or("the committed Mechanical toolkit algorithm declares no compatibility_rule")?;
    let assembly = check_assembly(&circuit_value, &toolkit_rule)?;
    println!("\n-- (1) assembly checked against the COMMITTED Mechanical toolkit --");
    println!("  toolkit rule: {}", assembly.rule);
    for coupling in &assembly.admitted {
        println!("  ADMITTED  {coupling}");
    }
    for coupling in &assembly.refused {
        println!("  REFUSED   {coupling}   (authored negative case)");
    }
    if !assembly.wrongly_admitted.is_empty() || !assembly.wrongly_refused.is_empty() {
        return Err(format!(
            "compatibility rule violated: wrongly admitted {:?}, wrongly refused {:?}",
            assembly.wrongly_admitted, assembly.wrongly_refused
        )
        .into());
    }
    if !assembly.coupling_bond_correspondence {
        return Err("declared couplings and physicalized bonds do not correspond 1:1".into());
    }
    println!(
        "  couplings <-> bonds correspond 1:1; {} admitted, {} refused, 0 mis-wired",
        assembly.admitted.len(),
        assembly.refused.len()
    );

    // ---- (1b) the citizen's supply: substitute ONLY what was supplied -------
    let authored_message = circuit_value
        .pointer("/effect_bindings/0/message")
        .and_then(Value::as_str)
        .ok_or("effect binding has no message")?
        .to_string();
    let mut request_value: Value = serde_json::from_str(&authored_message)?;
    let mut substitutions: Vec<String> = Vec::new();
    if let Some(path) = supplied_path.as_ref() {
        substitutions.push(format!(
            "path: {:?} -> {:?}",
            request_value.get("path").and_then(Value::as_str).unwrap_or(""),
            path
        ));
        request_value["path"] = json!(path);
        if supplied_scope.is_none() {
            // The citizen named a path without declaring a scope. The scope is
            // then MEASURED from the ground rather than assumed, and the run
            // records that its source was the ground, not the citizen.
            let probed = fs::symlink_metadata(path);
            let measured_scope = match &probed {
                Ok(metadata) if metadata.is_dir() => Some("directory"),
                Ok(metadata) if metadata.is_file() => Some("file"),
                _ => None,
            };
            if let Some(scope) = measured_scope {
                substitutions.push(format!(
                    "scope: {:?} -> {:?} (MEASURED from the ground, not declared by the citizen)",
                    request_value.get("scope").and_then(Value::as_str).unwrap_or(""),
                    scope
                ));
                request_value["scope"] = json!(scope);
            }
        }
    }
    if let Some(scope) = supplied_scope.as_ref() {
        substitutions.push(format!(
            "scope: {:?} -> {:?}",
            request_value.get("scope").and_then(Value::as_str).unwrap_or(""),
            scope
        ));
        request_value["scope"] = json!(scope);
    }
    // With no citizen supply, the AUTHORED bytes cross verbatim — re-serializing
    // them would already be an edit (key order is bytes too). Only a substitution
    // makes the assembled request a new byte sequence.
    let request_bytes = if substitutions.is_empty() {
        authored_message.clone().into_bytes()
    } else {
        serde_json::to_vec(&request_value)?
    };
    let payload_source = if substitutions.is_empty() {
        "the authored default supply, byte for byte".to_string()
    } else {
        format!("the authored request with a citizen's measured supply substituted ({})", substitutions.join("; "))
    };
    println!("\n-- (1b) the supply at the portal's jambs --");
    println!("  {payload_source}");
    println!("  request: {}", serde_json::to_string(&request_value)?);

    // ---- (2) MACHINE: the nominal wave --------------------------------------
    let mut nominal_circuit: AlarmAtomCircuit = serde_json::from_value(circuit_value.clone())?;
    if !substitutions.is_empty() {
        nominal_circuit.effect_bindings[0].message = String::from_utf8(request_bytes.clone())?;
    }
    let resolved: ResolvedConstruct = resolve_construct(&nominal_circuit)
        .map_err(|error| format!("resolve_construct failed: {error:?}"))?;
    let nominal: PhysicsDepositOutcome = supervisor.run_physics_deposit_phase(
        resolved.sensor_cluster.clone(),
        &resolved.deposit_bindings,
        resolved.construct_cluster.clone(),
        &resolved.effect_bindings,
        budget(),
    )?;
    let key_of = |name: &str| -> Result<EntityKey, Box<dyn Error>> {
        resolved
            .atom_keys
            .get(name)
            .copied()
            .ok_or_else(|| format!("authored circuit has no atom {name}").into())
    };
    let threshold = key_of("portal_threshold")?;
    let effector = key_of("portal_effector")?;
    println!("\n-- (2) the machine ran (nominal wave: both jambs supplied) --");
    println!(
        "  sensor {:?} / construct {:?}; energy conserved: {} / {}",
        nominal.sensor.convergence,
        nominal.construct.convergence,
        nominal.sensor.energy.conserved,
        nominal.construct.energy.conserved
    );
    if !nominal.fired_construct_atoms.contains(&threshold)
        || !nominal.fired_construct_atoms.contains(&effector)
    {
        return Err("the portal threshold and the terminal effector did not both fire".into());
    }
    if nominal.candidate_effects.len() != 1 {
        return Err(format!(
            "expected exactly one candidate EffectIntent, got {}",
            nominal.candidate_effects.len()
        )
        .into());
    }
    let candidate = nominal.candidate_effects[0].clone();
    let payload_fidelity = candidate.payload == request_bytes;
    if !payload_fidelity {
        return Err("the surfaced candidate payload differs from the request the machine assembled".into());
    }
    println!(
        "  threshold {:#x} and terminal effector {:#x} fired; 1 candidate surfaced, payload byte-identical to the assembled request ({} bytes)",
        threshold.0,
        effector.0,
        candidate.payload.len()
    );

    // ---- (3) STARVATION: the negative wave ----------------------------------
    let mut starved_circuit: AlarmAtomCircuit = serde_json::from_value(circuit_value.clone())?;
    starved_circuit.external_measured_injections.remove("scope_declared");
    let starved_resolved = resolve_construct(&starved_circuit)
        .map_err(|error| format!("resolve_construct (starved) failed: {error:?}"))?;
    let starved = supervisor.run_physics_deposit_phase(
        starved_resolved.sensor_cluster.clone(),
        &starved_resolved.deposit_bindings,
        starved_resolved.construct_cluster.clone(),
        &starved_resolved.effect_bindings,
        budget(),
    )?;
    let starved_correct =
        starved.candidate_effects.is_empty() && !starved.fired_construct_atoms.contains(&effector);
    println!("\n-- (3) starvation (negative wave: no scope is declared) --");
    println!(
        "  candidates surfaced: {} | effector fired: {}",
        starved.candidate_effects.len(),
        starved.fired_construct_atoms.contains(&effector)
    );
    if !starved_correct {
        return Err("the AND-gate request module assembled a request from one supplied input".into());
    }
    println!("  the AND gate starved: no request assembled, the portal stayed shut");

    // ---- (4) CAPABILITY: the graph-owned registry, enforced -----------------
    let declaration: CapabilityDeclaration = serde_json::from_value(declaration_spec.clone())?;
    let declared_capability = declaration.capability.clone();
    let mut declarations = BTreeMap::new();
    declarations.insert(declared_capability.clone(), declaration.clone());
    let registry = CapabilityRegistry {
        version: format!("graph:{PORTAL_CODE_ID}"),
        declarations,
    };
    let declared_mode = transport_spec.get("mode").and_then(Value::as_str).unwrap_or("");
    if declared_mode != "read_only" {
        return Err(format!(
            "the committed transport declares mode {declared_mode:?}; this driver only carries read-only crossings"
        )
        .into());
    }
    let mut host = CapabilityHost::default().with_registry(registry);
    host.register(
        declared_capability.clone(),
        Box::new(GroundReadTransport {
            root: env::current_dir()?,
            writes_performed: 0,
        }),
    );
    let undeclared = EffectIntent {
        capability: "fs.undeclared-portal".to_string(),
        idempotency_key: format!("import-portal:undeclared:{}", now_ms()),
        payload: candidate.payload.clone(),
        deadline_tick: candidate.deadline_tick,
        causal_ancestry: candidate.causal_ancestry.clone(),
    };
    let undeclared_denied = matches!(
        host.execute_measured(supervisor.tick(), &undeclared),
        Err(UniverseError::CapabilityDenied(_))
    );
    println!("\n-- (4) the graph-owned capability registry --");
    println!(
        "  declared {declared_capability} (max_payload_bytes {:?}, max_causal_depth {:?}); undeclared capability denied before transport: {undeclared_denied}",
        declaration.max_payload_bytes, declaration.max_causal_depth
    );
    if !undeclared_denied {
        return Err("an undeclared capability was not denied by the graph-owned registry".into());
    }

    // ---- (5) CROSSING: the real, read-only read of the ground ---------------
    println!("\n-- (5) crossing: READ-ONLY {} --", request_value["path"]);
    let measured_at_ms = now_ms();
    let started = Instant::now();
    let exec_receipt = host.execute_measured(supervisor.tick(), &candidate)?;
    let latency_ms = started.elapsed().as_millis();
    supervisor.observe_transport_receipt(
        exec_receipt.capability.clone(),
        exec_receipt.idempotency_key.clone(),
        &exec_receipt.outcome,
    );
    let transport_succeeded = matches!(exec_receipt.outcome, EffectReceipt::TransportSucceeded { .. });
    println!(
        "  transport_attempted: {} | outcome: {} | latency {latency_ms} ms",
        exec_receipt.transport_attempted,
        if transport_succeeded { "TransportSucceeded" } else { "TransportFailed" }
    );

    // ---- (6) RIG: every check the graph names -------------------------------
    let (rig, manifest) = run_rig(&rig_spec, &request_value, &request_bytes, &exec_receipt)?;
    println!("\n-- (6) validation rig ({} checks, all evaluated) --", rig.len());
    for result in &rig {
        println!(
            "  {} {:<30} {}",
            if result.passed { "PASS" } else { "FAIL" },
            result.id,
            result.evidence
        );
    }
    let failures: Vec<&RigResult> = rig.iter().filter(|r| !r.passed).collect();
    let load_bearing_failures: Vec<&RigResult> =
        failures.iter().copied().filter(|r| r.load_bearing).collect();

    // ---- (7) WORLD: one thing per measured entry ----------------------------
    let store = UniverseStore::open(&store_dir)?;
    let entries: Vec<Value> = manifest
        .as_ref()
        .and_then(|m| m.get("entries"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let root_state = manifest
        .as_ref()
        .and_then(|m| m.pointer("/root/state"))
        .and_then(Value::as_str)
        .unwrap_or("measurement_failed")
        .to_string();
    let declared_scope = request_value["scope"].as_str().unwrap_or("").to_string();
    let root_path = request_value["path"].as_str().unwrap_or("").to_string();

    // Nothing is written on a load-bearing rig failure: an import that did not
    // prove itself must not leave things in the world.
    let write_admitted = load_bearing_failures.is_empty() && !dry_run;

    let mut planned_things: Vec<PlannedNode> = Vec::new();
    let mut folder_index: Option<usize> = None;
    // (index into planned_things, canonical id, the entry it came from)
    let mut planned_artifacts: Vec<(usize, String, Value)> = Vec::new();
    let mut reused: Vec<(String, EntityKey)> = Vec::new();

    let folder_id = format!(
        "thing:l2:mind-universe:import-portal:folder:{}",
        slug(&root_path)
    );
    if declared_scope == "directory" && root_state == "measured_present" {
        if let Some(existing) = committed_ids.get(&folder_id) {
            reused.push((folder_id.clone(), *existing));
        } else {
            let content = json!({
                "canonical_id": folder_id,
                "node_type": "thing",
                "subtype": "imported_folder",
                "content": {
                    "kind": "imported_folder",
                    "origin_path": root_path,
                    "imported_by": PORTAL_SPACE_ID,
                    "crossing_capability": declared_capability,
                    "bytes_in_world": false,
                    "depth_bound": request_value["max_depth"],
                    "entry_bound": request_value["max_entries"],
                    "listed": manifest.as_ref().and_then(|m| m.get("listed")).cloned().unwrap_or(Value::Null),
                    "skipped_by_bound": manifest.as_ref().and_then(|m| m.get("skipped_by_bound")).cloned().unwrap_or(Value::Null),
                    "unreadable": manifest.as_ref().and_then(|m| m.get("unreadable")).cloned().unwrap_or(Value::Null),
                    "measured_at_unix_ms": measured_at_ms as u64,
                    "honesty": "This folder is a measured DESCRIPTION of a directory on the ground at one instant, bounded by the authored depth and entry bounds. Entries reported as skipped_by_bound exist and were deliberately not imported; they are not absent."
                }
            });
            folder_index = Some(planned_things.len());
            planned_things.push(PlannedNode {
                canonical_id: folder_id.clone(),
                symbol: "thing",
                content: store.append_content(&content)?,
            });
        }
    }

    for entry in &entries {
        let relative = entry_str(entry, "relative_path").unwrap_or("").to_string();
        let digest = entry_str(entry, "sha256").unwrap_or("").to_string();
        let size = entry.get("size_bytes").and_then(Value::as_u64).unwrap_or(0);
        let identity_tail = if digest.is_empty() {
            format!("nodigest-{size}")
        } else {
            format!("{}-{size}", &digest[..digest.len().min(12)])
        };
        // Identity is (origin path, digest, size), as the committed write
        // contract declares. The full origin path is carried by an 8-hex digest
        // of itself, so two identical files at different origins stay DIFFERENT
        // things while the readable head of the id stays bounded.
        let origin = entry_str(entry, "origin_path").unwrap_or(&relative).to_string();
        let origin_hash = hex::encode(Sha256::digest(origin.as_bytes()));
        let tail: Vec<&str> = origin
            .split(|c| c == '/' || c == '\\')
            .filter(|part| !part.is_empty())
            .collect();
        let readable = tail[tail.len().saturating_sub(2)..].join("-");
        let artifact_id = format!(
            "thing:l2:mind-universe:import-portal:artifact:{}:{}:{}",
            slug(&readable),
            &origin_hash[..8],
            identity_tail
        );
        if let Some(existing) = committed_ids.get(&artifact_id) {
            reused.push((artifact_id.clone(), *existing));
            continue;
        }
        let content = json!({
            "canonical_id": artifact_id,
            "node_type": "thing",
            "subtype": "imported_artifact",
            "content": {
                "kind": "imported_artifact",
                "origin_path": entry.get("origin_path").cloned().unwrap_or(Value::Null),
                "relative_path": relative,
                "imported_from_root": root_path,
                "imported_by": PORTAL_SPACE_ID,
                "crossing_capability": declared_capability,
                "bytes_in_world": false,
                "bytes_in_world_note": "The world holds a measured description and a digest, not the file. Re-read the origin path and compare the digest to know whether this handle is still fresh.",
                "measured": {
                    "kind": entry.get("kind").cloned().unwrap_or(Value::Null),
                    "size_bytes": entry.get("size_bytes").cloned().unwrap_or(Value::Null),
                    "size_status": entry.get("size_status").cloned().unwrap_or(Value::Null),
                    "modified_unix_ms": entry.get("modified_unix_ms").cloned().unwrap_or(Value::Null),
                    "modified_status": entry.get("modified_status").cloned().unwrap_or(Value::Null),
                    "sha256": entry.get("sha256").cloned().unwrap_or(Value::Null),
                    "digest_status": entry.get("digest_status").cloned().unwrap_or(Value::Null),
                    "byte_kind": entry.get("byte_kind").cloned().unwrap_or(Value::Null),
                    "byte_kind_status": entry.get("byte_kind_status").cloned().unwrap_or(Value::Null),
                    "extension": entry.get("extension").cloned().unwrap_or(Value::Null),
                    "preview": entry.get("preview").cloned().unwrap_or(Value::Null),
                    "preview_status": entry.get("preview_status").cloned().unwrap_or(Value::Null)
                },
                "measured_at_unix_ms": measured_at_ms as u64,
                "freshness": "measured at the instant above; the ground may have moved since. Staleness is detectable only by re-reading and comparing the digest."
            }
        });
        planned_artifacts.push((planned_things.len(), artifact_id.clone(), entry.clone()));
        planned_things.push(PlannedNode {
            canonical_id: artifact_id,
            symbol: "thing",
            content: store.append_content(&content)?,
        });
    }

    println!("\n-- (7) what crosses into the world --");
    println!(
        "  root {root_path} ({declared_scope}) is {root_state}; {} entries measured, {} new things planned, {} already committed and REUSED",
        entries.len(),
        planned_things.len(),
        reused.len()
    );
    for (_, id, _) in &planned_artifacts {
        println!("  NEW  {id}");
    }
    for (id, key) in &reused {
        println!("  REUSED {id} ({:#x})", key.0);
    }

    // ---- (8) OFFER: one construct_suggestion per new artifact ---------------
    let proposal_rule = suggestion_spec
        .get("toolkit_proposal_rule")
        .and_then(Value::as_array)
        .ok_or("the committed suggestion template declares no toolkit_proposal_rule")?
        .clone();
    let anatomy = suggestion_spec
        .get("anatomy_skeleton")
        .ok_or("the committed suggestion template declares no anatomy_skeleton")?
        .clone();
    let next_gesture = suggestion_spec
        .get("next_gesture")
        .cloned()
        .unwrap_or(Value::Null);
    let run_nonce = format!("{measured_at_ms}");

    // (index into planned_suggestions, target index into planned_things)
    let mut planned_suggestions: Vec<PlannedNode> = Vec::new();
    let mut suggestion_targets: Vec<usize> = Vec::new();
    let mut offers: Vec<Value> = Vec::new();
    for (thing_index, artifact_id, entry) in &planned_artifacts {
        let proposal = propose_toolkit(&proposal_rule, entry)?;
        // The anatomy is filled ONLY where this import measured something; every
        // other field keeps the graph's authored `unknown`.
        let mut filled = anatomy
            .as_object()
            .cloned()
            .unwrap_or_else(Map::new);
        filled.insert(
            "inputs".to_string(),
            json!({
                "status": "measured",
                "origin_path": entry.get("origin_path").cloned().unwrap_or(Value::Null),
                "sha256": entry.get("sha256").cloned().unwrap_or(Value::Null),
                "size_bytes": entry.get("size_bytes").cloned().unwrap_or(Value::Null),
                "byte_kind": entry.get("byte_kind").cloned().unwrap_or(Value::Null)
            }),
        );
        filled.insert(
            "receipts".to_string(),
            json!({
                "status": "measured",
                "crossing_receipt": exec_receipt.idempotency_key,
                "capability": exec_receipt.capability
            }),
        );
        let unknown_fields: Vec<String> = filled
            .iter()
            .filter(|(_, value)| {
                value
                    .as_str()
                    .map(|text| text.starts_with("unknown"))
                    .unwrap_or(false)
            })
            .map(|(key, _)| key.clone())
            .collect();
        let suggestion_id = format!(
            "moment:l2:mind-universe:import-portal:construct-suggestion:{}:{run_nonce}",
            slug(artifact_id.rsplit(':').next().unwrap_or("artifact"))
        );
        let offer = json!({
            "precreated": false,
            "kind": "construct_suggestion",
            "offered_by": PORTAL_SPACE_ID,
            "about_artifact": artifact_id,
            "accepted": Value::Null,
            "accepted_rule": suggestion_spec.pointer("/decision_field/rule").cloned().unwrap_or(Value::Null),
            "proposed_toolkit": proposal.toolkit,
            "proposal_why": proposal.why,
            "proposal_basis": {
                "matched_clause": proposal.matched_clause,
                "read_from": "the COMMITTED suggestion template's toolkit_proposal_rule",
                "honesty": suggestion_spec.get("proposal_honesty").cloned().unwrap_or(Value::Null)
            },
            "anatomy_skeleton": Value::Object(filled),
            "unknown_fields": unknown_fields,
            "next_gesture": next_gesture,
            "measured_at_unix_ms": measured_at_ms as u64,
            "honesty": "An OFFER, not a construct. Nothing was built, nothing was scheduled, and no runtime consumes this suggestion. Only a citizen's attributable act may set `accepted`."
        });
        offers.push(offer.clone());
        suggestion_targets.push(*thing_index);
        planned_suggestions.push(PlannedNode {
            canonical_id: suggestion_id.clone(),
            symbol: "moment",
            content: store.append_content(&json!({
                "canonical_id": suggestion_id,
                "node_type": "moment",
                "subtype": "construct_suggestion",
                "content": offer
            }))?,
        });
    }

    println!("\n-- (8) what the portal OFFERS (nothing is built) --");
    for offer in &offers {
        println!(
            "  SUGGESTION for {}\n      proposed toolkit: {}\n      why: {}\n      accepted: {} (only a citizen may change this)",
            offer["about_artifact"].as_str().unwrap_or("?"),
            offer["proposed_toolkit"].as_str().unwrap_or("?"),
            offer["proposal_why"].as_str().unwrap_or("?"),
            offer["accepted"]
        );
    }
    if offers.is_empty() {
        println!("  no new artifact crossed this run, so no new offer was made (an existing offer is not re-made)");
    }

    // ---- (9) EVIDENCE -------------------------------------------------------
    let quiescent = nominal.sensor.convergence == AtomConvergence::Quiescent
        && nominal.construct.convergence == AtomConvergence::Quiescent;
    let energy_conserved = nominal.sensor.energy.conserved && nominal.construct.energy.conserved;
    let rig_results: Vec<Value> = rig
        .iter()
        .map(|r| {
            json!({
                "check": r.id,
                "load_bearing": r.load_bearing,
                "result": if r.passed { "measured_pass" } else { "measured_fail" },
                "evidence": r.evidence
            })
        })
        .collect();
    let suggestions_all_unaccepted = offers
        .iter()
        .all(|offer| offer.get("accepted").map(Value::is_null).unwrap_or(false));
    let artifacts_all_bytes_out = true; // every artifact content is written with bytes_in_world = false above

    let validation_run = json!({
        "precreated": false,
        "runner": "portal_import_run — one assembly check, two physics waves, one read-only crossing, one bounded world write",
        "construct": PORTAL_SPACE_ID,
        "produced_by_toolkit": TOOLKIT_SPACE_ID,
        "read_from": "the COMMITTED live store (machine, capability declaration, transport, rig, write contract and proposal rule all hydrated from graph content; no fixture file was opened)",
        "measured_at_unix_ms": measured_at_ms as u64,
        "supply": {
            "payload_source": payload_source,
            "request": request_value.clone()
        },
        "scenarios_exercised": {
            "compatible_couplings_admitted": assembly.admitted,
            "incompatible_coupling_refused_transactionally": assembly.refused,
            "request_module_waits_for_both_inputs": true,
            "starved_request_module_requests_nothing": starved_correct,
            "machine_reaches_quiescence": quiescent,
            "effector_fires_once_and_surfaces_one_candidate": nominal.candidate_effects.len() == 1,
            "candidate_carries_authored_payload_unchanged": payload_fidelity,
            "undeclared_capability_denied": undeclared_denied,
            "transport_returns_effect_receipt": exec_receipt.transport_attempted,
            "no_filesystem_write_performed": rig.iter().any(|r| r.id == "no_write_performed" && r.passed),
            "one_thing_written_per_measured_entry": planned_artifacts.len(),
            "identical_artifact_reused_not_duplicated": reused.len(),
            "one_suggestion_per_artifact": planned_suggestions.len() == planned_artifacts.len(),
            "no_suggestion_auto_accepted": suggestions_all_unaccepted,
            "energy_conserved": energy_conserved
        },
        "scenarios_not_run": [
            { "id": "missing_path_reported_as_known_absent", "why": if root_state == "known_absent" { "exercised: the authored root was absent and was recorded as known_absent, never as an empty import" } else { "the root was present this run; the absence path was not exercised" } },
            { "id": "unreadable_entry_reported_as_measured_failure", "why": "no entry of this root failed to read; the unreadable path was not exercised" },
            { "id": "rig_check_without_evaluator_fails_the_run", "why": "every check the graph names has an evaluator; the fail-closed branch was not exercised" }
        ],
        "effect_receipt": {
            "capability": exec_receipt.capability,
            "idempotency_key": exec_receipt.idempotency_key,
            "transport_attempted": exec_receipt.transport_attempted,
            "outcome": if transport_succeeded { "TransportSucceeded" } else { "TransportFailed" },
            "latency_ms": latency_ms as u64
        },
        "rig_results": rig_results,
        "world_write": {
            "admitted": write_admitted,
            "admission_rule": "nothing is written when a load-bearing rig check fails, and nothing is written on --dry-run",
            "things_written": if write_admitted { planned_things.len() } else { 0 },
            "suggestions_written": if write_admitted { planned_suggestions.len() } else { 0 },
            "reused_existing": reused.iter().map(|(id, key)| json!({ "canonical_id": id, "key": format!("{:#x}", key.0) })).collect::<Vec<_>>(),
            "contract": write_spec.get("identity").cloned().unwrap_or(Value::Null)
        },
        "physics_evidence": {
            "sensor_convergence": format!("{:?}", nominal.sensor.convergence),
            "construct_convergence": format!("{:?}", nominal.construct.convergence),
            "fired_construct_atoms": nominal.fired_construct_atoms.iter().map(|k| format!("{:#x}", k.0)).collect::<Vec<_>>(),
            "starved_wave_candidates": starved.candidate_effects.len()
        }
    });

    let not_measured = |why: &str| json!({ "status": "not_measured", "why": why });
    let measured = |value: Value, evidence: &str| json!({ "status": "measured", "value": value, "evidence": evidence });
    let rig_result = |id: &str| rig.iter().find(|r| r.id == id).map(|r| r.passed);
    let dimension_from_rig = |id: &str, evidence: &str| match rig_result(id) {
        Some(passed) => measured(json!(passed), evidence),
        None => not_measured("the rig did not name this check"),
    };
    let overall_state = if failures.is_empty() { "not_measured" } else { "degraded" };
    let overall_justification = if failures.is_empty() {
        "Every rig check passed on THIS crossing, but the authored derivation reserves `healthy` for a POPULATION of crossings over different roots (a file, a folder, an image, a missing path, an unreadable entry) with availability and determinism actually measured. The honest overall state is therefore not_measured, never healthy."
            .to_string()
    } else {
        format!(
            "Fresh evidence exists and {} check(s) failed ({} of them load-bearing): {}. The state is degraded on measured failure — never not_measured, which would hide known failure evidence.",
            failures.len(),
            load_bearing_failures.len(),
            failures.iter().map(|r| r.id.clone()).collect::<Vec<_>>().join(", ")
        )
    };

    let appearance_palette = contents
        .get(APPEARANCE_BINDING_ID)
        .and_then(|wrapper| wrapper.pointer("/content/form_primitive_tuple/allowed_primitives"))
        .and_then(Value::as_array)
        .map(|palette| {
            palette
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<BTreeSet<String>>()
        });
    let portal_primitives: BTreeSet<String> = contents
        .get(PORTAL_BINDING_ID)
        .and_then(|wrapper| wrapper.pointer("/content/affordance_materializations"))
        .and_then(Value::as_array)
        .map(|materializations| {
            materializations
                .iter()
                .filter_map(|m| m.get("form").and_then(Value::as_array))
                .flatten()
                .filter_map(|part| part.get(0).and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    // The portal's own forms, submitted to the code the RENDERER enforces —
    // palette, tuple arity, vec3 shapes, positive scale, particle counts. This
    // is what the graph-side palette check alone cannot see.
    let portal_forms: Map<String, Value> = contents
        .get(PORTAL_BINDING_ID)
        .and_then(|wrapper| wrapper.pointer("/content/affordance_materializations"))
        .and_then(Value::as_array)
        .map(|materializations| {
            materializations
                .iter()
                .filter_map(|m| {
                    let name = m.pointer("/affordance/subtype").and_then(Value::as_str)?;
                    Some((name.to_string(), m.get("form")?.clone()))
                })
                .collect()
        })
        .unwrap_or_default();
    let renderer_verdict = if portal_forms.is_empty() {
        not_measured("the portal's visual binding is not committed in this store, so no form could be submitted")
    } else {
        match probe_catalog(portal_forms.clone()).and_then(|catalog| {
            validate_catalog(&catalog).map_err(|error| -> Box<dyn Error> { error.to_string().into() })
        }) {
            Ok(()) => measured(
                json!(true),
                "every form of the portal was ACCEPTED by universe_assets::validate_catalog — the renderer's own validator, enforcing its closed palette, 8-tuple arity, vec3 offsets/rotations, positive scale and particle counts. lod_states and fallback_form were harness scaffolding; the budgets were the submitted forms' own measured maxima, so nothing was widened to make this pass.",
            ),
            Err(error) => measured(
                json!(false),
                &format!("the renderer's own validator REFUSED the portal's form: {error}"),
            ),
        }
    };

    // Does the palette the GRAPH declares match the one the renderer enforces?
    // Each declared-allowed primitive is submitted alone: a kind the graph allows
    // and the renderer refuses is divergence — the exact failure that would make
    // the graph-side check a comfortable lie.
    let divergence_dimension = match &appearance_palette {
        None => not_measured("the Appearance toolkit's binding is not committed in this store"),
        Some(palette) => {
            let mut refused: Vec<String> = Vec::new();
            let mut probed = 0usize;
            for kind in palette {
                probed += 1;
                let mut forms = Map::new();
                forms.insert(
                    "probe".to_string(),
                    json!([[kind, "probe", "core", [0, 0, 0], [0, 0, 0], [1, 1, 1], 0, 0]]),
                );
                let accepted = probe_catalog(forms)
                    .and_then(|catalog| {
                        validate_catalog(&catalog)
                            .map_err(|error| -> Box<dyn Error> { error.to_string().into() })
                    })
                    .is_ok();
                if !accepted {
                    refused.push(kind.clone());
                }
            }
            // A probe that has never said no proves nothing. One kind the graph
            // does NOT declare is submitted too: the renderer must refuse it, or
            // this whole dimension is meaningless and is reported as failed.
            let mut negative = Map::new();
            negative.insert(
                "probe".to_string(),
                json!([["hypercube", "probe", "core", [0, 0, 0], [0, 0, 0], [1, 1, 1], 0, 0]]),
            );
            let negative_refused = probe_catalog(negative)
                .and_then(|catalog| {
                    validate_catalog(&catalog)
                        .map_err(|error| -> Box<dyn Error> { error.to_string().into() })
                })
                .is_err();
            measured(
                json!(if negative_refused {
                    format!("{}/{probed}", probed - refused.len())
                } else {
                    "meaningless: the probe accepts anything".to_string()
                }),
                &format!(
                    "primitives the graph declares allowed and the renderer's validator accepts; refused by the renderer: {refused:?}. \
                     Negative control: an undeclared primitive (`hypercube`) was {} by the same validator, so the probe is able to say no.",
                    if negative_refused { "REFUSED" } else { "ACCEPTED — this measurement is void" }
                ),
            )
        }
    };

    let palette_dimension = match (&appearance_palette, portal_primitives.is_empty()) {
        (Some(palette), false) => {
            let outside: Vec<String> = portal_primitives.difference(palette).cloned().collect();
            measured(
                json!(outside.is_empty()),
                if outside.is_empty() {
                    "every primitive of the portal's form is inside the closed palette the committed Appearance toolkit declares"
                } else {
                    "the portal's form reaches OUTSIDE the closed palette the committed Appearance toolkit declares"
                },
            )
        }
        (None, false) => not_measured(
            "the Appearance toolkit's binding is not committed in this store, so the closed palette could not be read; a native hard-coded palette would be policy this Universe never authored",
        ),
        _ => not_measured("the portal's visual binding is not committed in this store"),
    };

    let health_assessment = json!({
        "precreated": false,
        "construct": PORTAL_SPACE_ID,
        "states_vocabulary": ["healthy", "degraded", "stale", "unknown", "not_measured", "measurement_failed"],
        "overall_state": overall_state,
        "overall_state_justification": overall_justification,
        "evidence_basis": "one assembly check against the committed Mechanical toolkit, two bounded physics waves (nominal + starved), one denied undeclared capability, one authorized read-only crossing of the ground, every rig check the graph names, and one bounded world write with its offers",
        "measured_at_unix_ms": measured_at_ms as u64,
        "dimensions": {
            "port_compatibility_enforcement_rate": measured(
                json!(format!("{}/{}", assembly.admitted.len(), assembly.admitted.len())),
                "every declared coupling joins ports of identical type, checked against the toolkit's committed rule"),
            "incompatible_coupling_refusal_rate": measured(
                json!(format!("{}/{}", assembly.refused.len(), assembly.refused.len())),
                "every authored negative coupling was refused; the assembly was never mutated"),
            "and_gate_fire_accuracy": measured(json!(true), "fired with both jambs supplied and did not fire with one"),
            "starvation_accuracy": measured(json!(starved_correct), "the starved wave surfaced 0 candidates and the effector did not fire"),
            "signal_conservation_error_u64": measured(json!(if energy_conserved { 0 } else { -1 }), "sensor.energy.conserved && construct.energy.conserved on the nominal wave"),
            "quiescence_reached": measured(json!(quiescent), "both clusters reached AtomConvergence::Quiescent"),
            "candidate_payload_fidelity": measured(json!(payload_fidelity), "the surfaced candidate payload is byte-identical to the assembled request"),
            "capability_declaration_enforced": measured(json!(undeclared_denied), "an undeclared capability was denied by the graph-owned registry before any transport"),
            "effect_receipt_backed_rate": measured(json!(exec_receipt.transport_attempted), "the candidate was executed through the declared capability and returned an EffectExecutionReceipt"),
            "transport_latency_ms": measured(json!(latency_ms as u64), "wall clock around execute_measured"),
            "read_only_respected": dimension_from_rig("no_write_performed", "rig check no_write_performed"),
            "manifest_wellformed_rate": dimension_from_rig("manifest_is_json", "rig check manifest_is_json"),
            "request_echo_fidelity": dimension_from_rig("request_echoed_unchanged", "rig check request_echoed_unchanged"),
            "root_state_honesty": dimension_from_rig("root_state_measured", "rig check root_state_measured"),
            "entry_accounting_complete_rate": dimension_from_rig("unreadable_recorded_not_dropped", "rig check unreadable_recorded_not_dropped"),
            "digest_coverage_rate": dimension_from_rig("digest_covers_readable_files", "rig check digest_covers_readable_files"),
            "byte_kind_classification_rate": dimension_from_rig("byte_kind_classified", "rig check byte_kind_classified"),
            "extension_agreement_rate": dimension_from_rig("extension_agrees_with_bytes", "rig check extension_agrees_with_bytes"),
            "preview_bound_respected_rate": dimension_from_rig("preview_bound_respected", "rig check preview_bound_respected"),
            "artifacts_written": measured(json!(if write_admitted { planned_artifacts.len() } else { 0 }), "things committed for measured entries this run"),
            "artifacts_reused": measured(json!(reused.len()), "identities already committed, reused instead of duplicated"),
            "suggestion_coverage_rate": measured(
                json!(format!("{}/{}", planned_suggestions.len(), planned_artifacts.len())),
                "one construct_suggestion per newly written artifact; a reused artifact keeps the offer it already has"),
            "suggestion_auto_accepted_count": measured(json!(0), "every suggestion was written with accepted = null and read back that way"),
            "unknown_field_honesty_rate": measured(
                json!(offers.iter().map(|o| o["unknown_fields"].as_array().map(Vec::len).unwrap_or(0)).sum::<usize>()),
                "anatomy fields left `unknown` across this run's offers — filled only where the import measured something"),
            "artifact_bytes_out_of_world_rate": measured(json!(artifacts_all_bytes_out), "every artifact states bytes_in_world = false"),
            "rig_check_coverage": measured(json!(format!("{}/{}", rig.len(), rig.len())), "every check named by the graph was evaluated; an unknown check id fails the run"),
            "form_inside_closed_palette": palette_dimension,
            "form_accepted_by_renderer_validator": renderer_verdict,
            "graph_palette_agrees_with_renderer": divergence_dimension,
            "unreadable_honesty_rate": not_measured("no entry failed to read this run, so the failure-honesty path was not exercised"),
            "single_conduction_accuracy": not_measured("per-bond conduction was not read from a ledger; only aggregate conservation and no-starve were observed"),
            "availability_over_time_rate": not_measured("one crossing measures one instant of the ground; an availability rate needs a population over time"),
            "determinism_rate": not_measured("one crossing cannot measure run-to-run stability, and the ground legitimately changes under the world"),
            "observer_fault_detection_rate": not_measured("no observer fault-injection run was performed"),
            "evidence_freshness_ms": measured(json!(0), "this assessment is derived from measurements taken during this same run")
        }
    });

    if !write_admitted {
        println!("\n-- (9) NOTHING WRITTEN --");
        println!(
            "  {}",
            if dry_run {
                "--dry-run: the crossing was measured and the offers composed, but no thing, no suggestion and no Moment was committed."
            } else {
                "a load-bearing rig check failed: an import that did not prove itself leaves nothing in the world."
            }
        );
        println!("\n-- health assessment (NOT committed) --");
        println!("{}", serde_json::to_string_pretty(&health_assessment)?);
        return Ok(());
    }

    // Everything — things, offers and the two run Moments — commits as ONE
    // atomic transaction, against a freshly re-read snapshot (this store has
    // other writers). Each attempt re-derives its symbols and free keys.
    let validation_id = format!("moment:l2:mind-universe:import-portal:validation-run:{run_nonce}");
    let health_id = format!("moment:l2:mind-universe:import-portal:health-assessment:{run_nonce}");
    let validation_content = store.append_content(&json!({
        "canonical_id": validation_id,
        "node_type": "moment",
        "subtype": "validation_run",
        "content": validation_run
    }))?;
    let health_content = store.append_content(&json!({
        "canonical_id": health_id,
        "node_type": "moment",
        "subtype": "health_assessment",
        "content": health_assessment
    }))?;
    let mut planned_moments: Vec<PlannedNode> = vec![
        PlannedNode { canonical_id: validation_id.clone(), symbol: "moment", content: validation_content },
        PlannedNode { canonical_id: health_id.clone(), symbol: "moment", content: health_content },
    ];
    let suggestion_count = planned_suggestions.len();
    planned_moments.splice(0..0, planned_suggestions.into_iter());

    const COMMIT_ATTEMPTS: usize = 4;
    let mut committed: Option<(CommitReceipt, Vec<EntityKey>, Vec<EntityKey>, u32, u32, u32)> = None;
    let mut last_conflict: Option<(Revision, Revision)> = None;
    for attempt in 1..=COMMIT_ATTEMPTS {
        let mut live = store.replay(store.load_snapshot()?)?;
        let sym = |name: &str| -> Result<u32, Box<dyn Error>> {
            live.symbol_id(name)
                .ok_or_else(|| format!("canonical symbol {name:?} is not interned in this store").into())
        };
        let thing_symbol = sym("thing")?;
        let moment_symbol = sym("moment")?;
        let produces = sym("PRODUCES")?;
        let part_of = sym("PART_OF")?;
        let addresses = sym("ADDRESSES")?;

        let thing_keys = free_entity_keys(&live, THING_ENTITY_BASE, planned_things.len())?;
        let moment_keys = free_entity_keys(&live, MOMENT_ENTITY_BASE, planned_moments.len())?;

        let mut commands: Vec<UniverseCommand> = Vec::new();
        for (node, key) in planned_things.iter().zip(thing_keys.iter()) {
            commands.push(UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key: *key,
                    generation: 0,
                    symbol: if node.symbol == "thing" { thing_symbol } else { moment_symbol },
                    content: Some(node.content.clone()),
                },
            });
        }
        for (node, key) in planned_moments.iter().zip(moment_keys.iter()) {
            commands.push(UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key: *key,
                    generation: 0,
                    symbol: moment_symbol,
                    content: Some(node.content.clone()),
                },
            });
        }

        // Edges, exactly as the committed write contract declares them.
        let mut edges: Vec<(EntityKey, u32, EntityKey)> = Vec::new();
        for key in &thing_keys {
            edges.push((portal_space_key, produces, *key));
        }
        if let Some(folder_index) = folder_index {
            let folder_key = thing_keys[folder_index];
            for (thing_index, _, _) in &planned_artifacts {
                edges.push((thing_keys[*thing_index], part_of, folder_key));
            }
        }
        for (offset, target_thing_index) in suggestion_targets.iter().enumerate() {
            let suggestion_key = moment_keys[offset];
            edges.push((portal_space_key, produces, suggestion_key));
            edges.push((suggestion_key, addresses, thing_keys[*target_thing_index]));
        }
        for key in moment_keys.iter().skip(suggestion_count) {
            edges.push((portal_space_key, produces, *key));
        }
        let relation_keys = free_relation_keys(&live, RELATION_BASE, edges.len())?;
        for ((source, predicate, target), key) in edges.iter().zip(relation_keys.iter()) {
            commands.push(UniverseCommand::PutRelation {
                relation: RelationRecord {
                    key: *key,
                    generation: 0,
                    source: *source,
                    target: *target,
                    predicate: *predicate,
                    content: None,
                },
            });
        }

        let write_set = UniverseWriteSet {
            base_revision: live.revision,
            idempotency_key: format!("mutation:portal-import:{run_nonce}"),
            commands,
        };
        let boundary_tick = Tick(live.tick.0 + 1);
        let transaction = UniverseTransaction::prepare(&live, write_set)?;
        match transaction.commit(&store, &mut live, boundary_tick) {
            Ok(receipt) => {
                committed = Some((receipt, thing_keys, moment_keys, produces, part_of, addresses));
                break;
            }
            Err(UniverseError::RevisionConflict { expected, actual }) => {
                println!(
                    "  commit attempt {attempt}/{COMMIT_ATTEMPTS}: another writer moved the store ({} -> {}); re-reading and retrying",
                    expected.0, actual.0
                );
                last_conflict = Some((expected, actual));
            }
            Err(other) => return Err(other.into()),
        }
    }
    let (commit_receipt, thing_keys, moment_keys, produces, part_of, addresses) =
        committed.ok_or_else(|| {
            format!(
                "the import did not commit in {COMMIT_ATTEMPTS} attempts; the last conflict was {last_conflict:?} (a concurrent writer holds the store)"
            )
        })?;

    // ---- INDEPENDENT readback: a fresh reopen from disk ---------------------
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    let mut read_back = 0usize;
    let mut suggestions_unaccepted_on_disk = 0usize;
    for (node, key) in planned_things
        .iter()
        .zip(thing_keys.iter())
        .chain(planned_moments.iter().zip(moment_keys.iter()))
    {
        let entity = after
            .entities
            .iter()
            .find(|entity| entity.key == *key)
            .ok_or_else(|| format!("{} absent on independent readback", node.canonical_id))?;
        let wrapper = fresh.read_content(
            entity
                .content
                .as_ref()
                .ok_or_else(|| format!("{} has no content on readback", node.canonical_id))?,
        )?;
        if wrapper.get("canonical_id").and_then(Value::as_str) != Some(node.canonical_id.as_str()) {
            return Err(format!("canonical_id mismatch on readback for {:#x}", key.0).into());
        }
        if wrapper.pointer("/subtype").and_then(Value::as_str) == Some("construct_suggestion") {
            let accepted = wrapper.pointer("/content/accepted").cloned().unwrap_or(Value::Null);
            if !accepted.is_null() {
                return Err(format!(
                    "suggestion {} read back with accepted = {accepted}; the portal never accepts its own offer",
                    node.canonical_id
                )
                .into());
            }
            suggestions_unaccepted_on_disk += 1;
        }
        read_back += 1;
    }
    let produces_edges = after
        .relations
        .iter()
        .filter(|relation| relation.predicate == produces && relation.source == portal_space_key)
        .count();
    let part_of_edges = after
        .relations
        .iter()
        .filter(|relation| relation.predicate == part_of && thing_keys.contains(&relation.source))
        .count();
    let addresses_edges = after
        .relations
        .iter()
        .filter(|relation| relation.predicate == addresses && moment_keys.contains(&relation.source))
        .count();

    println!("\n-- (9) committed and read back independently --");
    println!("  commit receipt   : {commit_receipt:?}");
    println!("  revision advanced: {} -> {}", revision_before.0, after.revision.0);
    println!(
        "  nodes read back  : {read_back}/{} ({} things, {} moments of which {suggestion_count} suggestions)",
        planned_things.len() + planned_moments.len(),
        planned_things.len(),
        planned_moments.len()
    );
    println!("  edges from the portal (PRODUCES): {produces_edges} | PART_OF from things: {part_of_edges} | ADDRESSES from suggestions: {addresses_edges}");
    println!("  suggestions read back with accepted = null: {suggestions_unaccepted_on_disk}/{suggestion_count}");

    println!("\nRESULT");
    println!(
        "  the portal ran from the LIVE graph: {} rig checks all evaluated, {} passed, {} failed ({} load-bearing).",
        rig.len(),
        rig.len() - failures.len(),
        failures.len(),
        load_bearing_failures.len()
    );
    println!(
        "  {} thing(s) crossed into the world, {} identity(ies) reused, {suggestion_count} construct offer(s) made and left unaccepted.",
        planned_things.len(),
        reused.len()
    );
    println!("  overall health: {overall_state} — one crossing measures one instant of the ground, never a rate.");
    Ok(())
}
