//! The Underground toolkit's `change_ground` affordance, as a native bootstrap
//! executable: the sanctioned path by which the ground beneath the city (a
//! repository file) is changed — attributable, validated, receipted, read back.
//!
//! CLAUDE.md, "How you work here": "Do not modify files directly. Call the
//! underground-toolkit. […] Even changing the ground is a toolkit call:
//! attributable, validated, receipted, and read back."
//!
//! This is the chicken-egg bootstrap of that toolkit (it is itself authored with
//! a raw edit, once — like `place_built_position` was for the MutationBond).
//! Once it exists, ground changes go through it and stop being raw hand edits.
//!
//! It realizes the CLAUDE.md "Real effects require receipts" flow, where the
//! external effect is a filesystem write:
//!
//!   EffectIntent{change_ground}
//!     -> sealed-hatch capability gate: the ACTING capability set is READ from
//!        the actor's graph-held grant (USED) edges (universe-query
//!        `read_actor_capability_set`), then adjudicated by the live construct's
//!        SealedCapabilityPort (universe-capabilities). Authority is a graph
//!        fact, never a self-declared request field.
//!     -> validate precondition (file exists, exact old-string match)
//!     -> authorized transport (write the file)
//!     -> read back (re-hash, confirm new present / old absent)
//!     -> EffectReceipt Moment committed into the store (0 new symbols)
//!     -> independent readback of BOTH the file and the receipt
//!
//! An unauthorized caller fails CLOSED: the file is NOT written, and a REJECTION
//! receipt Moment is committed (the contract's `emit_rejection_receipt`) so the
//! refused attempt is audited, never silently dropped. The refusal records the
//! actor's actually-held capability set and the port's required capability, so a
//! reader can see exactly why admission was denied.
//!
//! Usage: `underground_change_ground <request.json> [store-dir]`
//!   request.json = {
//!     "path": "relative/or/abs/file",
//!     "old":  "exact substring to replace (must occur exactly once)",
//!     "new":  "replacement",
//!     "actor": "actor:l2:mind-universe:... (a canonical_id; authority is read
//!               from THIS actor's graph-held USED edges)",
//!     "capability": "(optional, vestigial) what the caller claims to hold;
//!                    recorded for audit but NEVER used for the decision",
//!     "justification": "why"
//!   }

use std::{collections::BTreeSet, env, error::Error, path::PathBuf};

use sha2::{Digest, Sha256};
use universe_capabilities::{MutateAdmission, SealedCapabilityPort};
use universe_core::{EntityKey, RelationKey, Tick};
use universe_query::read_actor_capability_set;
use universe_store::{
    EntityRecord, IndexedUniverseSnapshot, RelationRecord, UniverseSnapshot, UniverseStore,
};
use universe_transactions::{CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet};

const UNDERGROUND_SPACE_ID: &str = "space:l2:mind-universe:underground-toolkit-v0";
const UNDERGROUND_PORT_PREFIX: &str = "port:l2:mind-universe:underground";
/// The canonical predicate an actor's held capability set is read from (the
/// grant's authored HOLDS_CAPABILITY remaps to USED; see the grant fixture).
const GRANT_PREDICATE: &str = "USED";
/// The capability-entity content field naming the capability it confers.
const CAPABILITY_FIELD: &str = "capability";
/// Bounded relation budget for the actor's capability read (never a full scan).
const CAPABILITY_READ_BUDGET: usize = 64;

// Receipt key block, disjoint from every injected construct block.
const RECEIPT_ENTITY_BASE: u128 = 0x00E0_0000;
const RECEIPT_REL_BASE: u128 = 0x00E8_0000;

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Commit one EffectReceipt Moment (accepted or rejected) into the store, linked
/// from the Underground construct with PRODUCES. Interns no symbols. Returns the
/// receipt entity key + the commit receipt.
fn commit_moment(
    store: &UniverseStore,
    snapshot: &mut UniverseSnapshot,
    underground_key: EntityKey,
    content: &serde_json::Value,
    idempotency_key: String,
) -> Result<(EntityKey, CommitReceipt), Box<dyn Error>> {
    let salt = snapshot.tick.0 as u128;
    let receipt_key = EntityKey(RECEIPT_ENTITY_BASE + salt);
    if snapshot.entities.iter().any(|e| e.key == receipt_key) {
        return Err(format!("receipt key {:#x} already exists", receipt_key.0).into());
    }
    let moment_symbol = snapshot
        .symbol_id("moment")
        .ok_or("canonical symbol 'moment' is not interned in this store")?;
    let produces_symbol = snapshot
        .symbol_id("PRODUCES")
        .ok_or("canonical predicate 'PRODUCES' is not interned in this store")?;
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let base_revision = snapshot.revision;
    let commands = vec![
        UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: receipt_key,
                generation: 0,
                symbol: moment_symbol,
                content: Some(store.append_content(content)?),
            },
        },
        UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: RelationKey(RECEIPT_REL_BASE + salt),
                generation: 0,
                source: underground_key,
                target: receipt_key,
                predicate: produces_symbol,
                content: None,
            },
        },
    ];
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key,
        causal_ancestry: vec![format!("changeset:{UNDERGROUND_SPACE_ID}")],
        commands,
    };
    let transaction = UniverseTransaction::prepare(snapshot, write_set)?;
    let receipt = transaction.commit(store, snapshot, boundary_tick)?;
    Ok((receipt_key, receipt))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("CHANGE_GROUND FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let request_path = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: underground_change_ground <request.json> [store-dir]")?;
    let store_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/ontology-registry/current/store"));

    let req: serde_json::Value = serde_json::from_slice(&std::fs::read(&request_path)?)?;
    let field = |k: &str| -> Result<String, Box<dyn Error>> {
        req.get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("request is missing string field '{k}'").into())
    };
    let path = PathBuf::from(field("path")?);
    let old = field("old")?;
    let new = field("new")?;
    let actor = field("actor")?;
    // Vestigial: recorded for audit only. The decision derives authority from the
    // actor's graph-held capability set, never from this self-declared string.
    let offered_capability = req
        .get("capability")
        .and_then(|v| v.as_str())
        .unwrap_or("(none declared)")
        .to_string();
    let justification = field("justification")?;
    println!("change_ground request: path={} actor={actor}", path.display());

    // 1. Open the store; read the Underground construct, materialize its sealed
    //    hatch (the SealedCapabilityPort from the code member's `capability_gate`
    //    — DATA, not native policy), and resolve the request's actor canonical_id
    //    to its EntityKey. One bounded scan over the entity column.
    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;

    let mut underground_key: Option<EntityKey> = None;
    let mut gate_value: Option<serde_json::Value> = None;
    let mut actor_key: Option<EntityKey> = None;
    for entity in &snapshot.entities {
        let Some(ptr) = entity.content.as_ref() else { continue };
        let content = store.read_content(ptr)?;
        let Some(cid) = content.get("canonical_id").and_then(|v| v.as_str()) else { continue };
        if cid == UNDERGROUND_SPACE_ID {
            underground_key = Some(entity.key);
        }
        if cid == actor {
            actor_key = Some(entity.key);
        }
        // The sole mutation surface: the construct's declared capability_gate,
        // whose port_id lives under the underground port namespace.
        if let Some(gate) = content.pointer("/content/capability_gate") {
            if gate
                .get("port_id")
                .and_then(|v| v.as_str())
                .is_some_and(|id| id.starts_with(UNDERGROUND_PORT_PREFIX))
            {
                gate_value = Some(gate.clone());
            }
        }
    }
    let underground_key = underground_key
        .ok_or("Underground toolkit construct is not in the store; inject it before changing ground")?;
    let gate_value = gate_value.ok_or(
        "Underground construct carries no capability_gate in the store; the sealed hatch is not materializable",
    )?;
    let port: SealedCapabilityPort = serde_json::from_value(gate_value)?;
    let required_capability = port.required_mutate_capability.clone();
    println!(
        "sealed hatch: port {} requires mutate capability '{required_capability}' (read from the live construct)",
        port.port_id
    );

    // 2. Read the ACTING capability set from the actor's graph-held grant (USED)
    //    edges, then adjudicate with the sealed port. Authority is a graph fact,
    //    never the self-declared request field.
    let grant_predicate = snapshot
        .symbol_id(GRANT_PREDICATE)
        .ok_or("canonical grant predicate 'USED' is not interned in this store")?;
    // The index is built from a clone so the mutable snapshot stays free for the
    // receipt commit. Admission precedes any transaction, so this reads pre-state.
    let indexed = IndexedUniverseSnapshot::new(snapshot.clone())?;
    let (held, actor_resolved, read_status) = match actor_key {
        Some(key) => {
            let set = read_actor_capability_set(
                &indexed,
                &store,
                key,
                grant_predicate,
                CAPABILITY_FIELD,
                CAPABILITY_READ_BUDGET,
            )?;
            (set.capabilities, true, format!("{:?}", set.status))
        }
        // Unknown actor: no graph identity, hence an empty acting set. This is a
        // real fail-closed decision (an empty set never holds the authority cap).
        None => (BTreeSet::new(), false, "actor_not_in_graph".to_string()),
    };
    let held_list: Vec<String> = held.iter().cloned().collect();
    println!(
        "acting set for {actor} (resolved={actor_resolved}, read={read_status}): {held_list:?}"
    );

    // Sealed-hatch capability gate — FAIL CLOSED, but audit the refusal with the
    // actual held set and the port's required capability.
    let admission = port.resolve_mutate(&held);
    if let MutateAdmission::Rejected {
        port_id,
        required_capability,
        reason,
    } = &admission
    {
        let rejection = serde_json::json!({
            "canonical_id": format!("moment:l2:mind-universe:underground:change-ground-rejected:{}", snapshot.tick.0),
            "node_type": "moment",
            "subtype": "effect_receipt",
            "content": {
                "kind": "ground_change_rejection",
                "effect": "change_ground",
                "outcome": "rejected",
                "reason": reason,
                "toolkit": UNDERGROUND_SPACE_ID,
                "port_id": port_id,
                "path": path.display().to_string(),
                "actor": actor,
                "actor_resolved": actor_resolved,
                "capability_read_status": read_status,
                "held_capabilities": held_list,
                "offered_capability": offered_capability,
                "required_capability": required_capability,
                "justification": justification,
                "authority_source": "actor graph-held USED edges (not the request field)",
                "state_delta": "none (fail closed, ground unchanged)",
            }
        });
        let idempo = format!("effect:underground-change-ground-rejected:{}:{actor}", snapshot.tick.0);
        let (rkey, rcpt) = commit_moment(&store, &mut snapshot, underground_key, &rejection, idempo)?;
        println!("sealed hatch: REFUSED — rejection receipt committed ({:#x}) {rcpt:?}", rkey.0);
        return Err(format!(
            "actor '{actor}' does not hold '{required_capability}' in the graph \
             (held {held_list:?}) — fail closed, ground NOT changed (a manhole grants observe only)"
        )
        .into());
    }
    let admitted_capability = match &admission {
        MutateAdmission::Admitted { capability, .. } => capability.clone(),
        MutateAdmission::Rejected { .. } => unreachable!("rejection returned above"),
    };
    println!(
        "sealed hatch: ADMITTED — actor '{actor}' holds '{admitted_capability}' via a graph grant edge"
    );

    // 3. Validate precondition and apply the ground change (authorized transport).
    let before = std::fs::read(&path)?;
    let before_text = String::from_utf8(before.clone())?;
    let occurrences = before_text.matches(&old).count();
    if occurrences != 1 {
        return Err(format!(
            "precondition failed: old-string occurs {occurrences} times in {} (must be exactly 1)",
            path.display()
        )
        .into());
    }
    let sha_before = sha(&before);
    let after_text = before_text.replacen(&old, &new, 1);
    std::fs::write(&path, after_text.as_bytes())?;

    // 4. Read back the transport result: re-hash and confirm the edit landed.
    let after = std::fs::read(&path)?;
    let sha_after = sha(&after);
    let after_text_rb = String::from_utf8(after.clone())?;
    if !after_text_rb.contains(&new) || after_text_rb.matches(&old).count() != 0 {
        return Err("readback: file does not reflect the requested change".into());
    }
    println!("ground written: {} ({} -> {} bytes)", path.display(), before.len(), after.len());
    println!("  sha before: {sha_before}");
    println!("  sha after : {sha_after}");

    // 5. Commit the acceptance EffectReceipt Moment.
    let receipt_id = format!(
        "moment:l2:mind-universe:underground:change-ground:{}",
        &sha_after[..16]
    );
    let receipt_content = serde_json::json!({
        "canonical_id": receipt_id,
        "node_type": "moment",
        "subtype": "effect_receipt",
        "content": {
            "kind": "ground_change_receipt",
            "effect": "change_ground",
            "outcome": "committed",
            "toolkit": UNDERGROUND_SPACE_ID,
            "port_id": port.port_id,
            "path": path.display().to_string(),
            "actor": actor,
            "admitted_capability": admitted_capability,
            "held_capabilities": held_list,
            "offered_capability": offered_capability,
            "authority_source": "actor graph-held USED edges (not the request field)",
            "justification": justification,
            "sha_before": sha_before,
            "sha_after": sha_after,
            "bytes_before": before.len(),
            "bytes_after": after.len(),
            "provenance": "underground-toolkit.change_ground",
        }
    });
    let base_revision = snapshot.revision;
    let (receipt_key, receipt) = commit_moment(
        &store,
        &mut snapshot,
        underground_key,
        &receipt_content,
        format!("effect:underground-change-ground:{sha_after}"),
    )?;
    println!("\nEffectReceipt committed: {receipt:?}");

    // 6. Independent readback: fresh reopen, confirm the receipt Moment landed and
    //    re-hash the file to confirm the ground on disk matches the receipt.
    let fresh = UniverseStore::open(&store_dir)?;
    let after_snap = fresh.replay(fresh.load_snapshot()?)?;
    let receipt_entity = after_snap
        .entities
        .iter()
        .find(|e| e.key == receipt_key)
        .ok_or("EffectReceipt Moment not found on independent readback")?;
    let rb = fresh.read_content(receipt_entity.content.as_ref().ok_or("receipt has no content")?)?;
    let rb_sha_after = rb.pointer("/content/sha_after").and_then(|v| v.as_str()).unwrap_or("");
    let disk_sha = sha(&std::fs::read(&path)?);
    if rb_sha_after != disk_sha {
        return Err(format!(
            "readback mismatch: receipt sha_after {rb_sha_after} != on-disk sha {disk_sha}"
        )
        .into());
    }
    println!("independent readback OK:");
    println!("  receipt id : {receipt_id}");
    println!("  store rev  : {} -> {}", base_revision.0, after_snap.revision.0);
    println!("  ground on disk matches the receipt (sha {})", &disk_sha[..16]);
    println!(
        "\nRESULT: the Underground toolkit changed the ground at {} — attributable to {actor}, \
         validated, receipted (moment in store), and read back on both faces (file + graph).",
        path.display()
    );
    Ok(())
}
