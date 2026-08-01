//! Reap stale MCP session bodies from the LIVE store — attributably, without
//! pretending to delete anything.
//!
//! # What this is for
//!
//! Every Claude Code session calls the MCP `arrive` verb with a sponsor. A
//! SPONSORED arrival is not merely admitted, it is EMBODIED: `World::materialize_actor`
//! (mcp/src/world.rs) writes a durable `actor` node carrying `actor:l1:mind:claude-…`
//! and joins it to the orientation beacon (Balise Zéro) with a canonical `PART_OF`
//! edge. That is the right behaviour on arrival and the wrong behaviour forever:
//! the bodies accumulate at one vantage and crowd every later observation out of
//! its budget. Measured on the canonical store at revision 292: 32 session bodies
//! against a `MAX_OBJECTS` budget of 64, filling the beacon's frame.
//!
//! # Why severing, and not deleting
//!
//! The kernel has NO entity-delete verb. It does not need one here either: what
//! makes a body occupy a frame is not its existence but its REACHABILITY.
//! `universe_supervisor::perception::gather_cluster` walks the graph outward from
//! the observation origin over relations; a node no relation reaches is never a
//! candidate, is never laid out, and never enters the sphere. Severing the edge
//! is therefore the honest analogue of removal — the same move
//! `sever_registry_root` makes on the ontology manifest — and the node, its
//! identity, its provenance and its whole history survive it.
//!
//! # Why the node is revised as well
//!
//! An edge cut with no reason attached is an unexplained hole: a later reader
//! sees a body no space contains and cannot tell an intentional reaping from a
//! dropped write. So the reaping is TWO facts committed as ONE atomic set:
//!
//! ```text
//! PutEntity (upsert, generation + 1)  -> the body now reads residency=dormant
//!                                        and carries WHY, BY WHOM, on WHAT
//!                                        CRITERION, and which edges were cut
//! TombstoneRelation (per incident edge) -> the body leaves the beacon's space
//! ```
//!
//! `PutEntity` over an existing key is an upsert that preserves the key (see
//! `relabel_construct_kind`), so identity is untouched and the body reads back as
//! the same body, dormant. Per CLAUDE.md, "the result carries its own cause —
//! readable from the node, not reconstructed by walking a history".
//!
//! # Where the policy lives: NOT here
//!
//! This binary holds no threshold, no TTL, no age rule and no idea of what
//! "stale" means. Every criterion is supplied by the caller and RECORDED verbatim
//! on each reaped node:
//!
//! ```text
//! --expired                    the body's OWN content.expires_at is in the past
//! --arrived-before-revision N  the body's OWN content.base_revision is < N
//! --session <id>               this exact session, named by the operator
//! ```
//!
//! A body carrying no `expires_at` is `unknown`, never `expired`: missing data is
//! not zero, and absence of a lifetime is not proof that a lifetime ran out. Such
//! a body is reported and left alone unless a criterion that CAN see it selects
//! it.
//!
//! Nothing is committed without `--apply`, and `--apply` requires both `--reason`
//! and `--by`: there is no anonymous reaping.
//!
//! # Concurrency
//!
//! This store has other writers (every live MCP session). Each commit attempt
//! re-reads the committed state and re-prepares against THAT revision, bounded to
//! `COMMIT_ATTEMPTS`; the base revision is never widened and no other writer's
//! state is replayed over. Same pattern as `ollama_probe_run`.
//!
//! # Usage
//!
//! ```text
//! reap_session_bodies [store-dir]
//!     [--expired] [--arrived-before-revision N] [--session <id>]...
//!     [--keep <id>]... [--now <unix>]
//!     [--apply --reason "<why>" --by "<actor canonical id>"]
//! ```
//! store-dir defaults to artifacts/ontology-registry/current/store.
//! Without `--apply` the run is a DRY RUN: it reports and writes nothing.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, error::Error, path::PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use universe_core::{EntityKey, RelationKey, Tick, UniverseError};
use universe_store::{EntityRecord, UniverseStore};
use universe_transactions::{CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// The identity convention perception keys on for an L1 inhabitant
/// (`universe_supervisor::perception::L1_ACTOR_PREFIX`). Read from the node's own
/// `canonical_id`, never from its key.
const L1_ACTOR_PREFIX: &str = "actor:l1:";
/// The content field `materialize_actor` writes naming the session a body
/// embodies. Its PRESENCE is what distinguishes a session body from any other L1
/// actor: an authored inhabitant carries no session, and must never be reaped.
const SESSION_FIELD: &str = "embodied_session";
/// The content field this binary writes to record that a body is no longer
/// resident. A plain content field — no symbol is interned, ever.
const RESIDENCY_FIELD: &str = "residency";
/// The residency level a reaped body is demoted to (CLAUDE.md, "Bounded physical
/// materialization": Hot / Sleeping / Aggregated / Dormant).
const DORMANT: &str = "dormant";
/// The field carrying the retained reason. "No revision without a retained reason."
const REAPING_FIELD: &str = "reaping";
/// Bounded optimistic-commit attempts against a store with other writers.
const COMMIT_ATTEMPTS: usize = 4;

fn main() {
    if let Err(error) = run() {
        eprintln!("REAP FAILED: {error}");
        std::process::exit(1);
    }
}

// --- what the caller asked for -------------------------------------------------

/// The criteria the OPERATOR supplies. This binary invents none of them.
#[derive(Clone, Debug, Default)]
struct Criteria {
    /// Reap a body whose own `expires_at` is strictly before `now`.
    expired: bool,
    /// Reap a body whose own `base_revision` is strictly before this.
    arrived_before_revision: Option<u64>,
    /// Reap exactly these sessions (matched against `embodied_session` or the
    /// full `canonical_id`).
    sessions: Vec<String>,
    /// Never reap these, whatever else selects them. Applied last; always wins.
    keep: Vec<String>,
    /// Wall clock the `--expired` criterion is evaluated against.
    now: u64,
}

/// What the criteria say about one body, and why. Every variant carries the
/// sentence that will be written onto the node (or printed for the reader).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    /// Selected, with the criterion that selected it.
    Reap(String),
    /// Explicitly protected by `--keep`.
    Kept(String),
    /// No criterion selected it, with the honest reason.
    Left(String),
    /// Already dormant — a re-run commits nothing for it.
    AlreadyDormant,
}

/// One session body as the store currently holds it.
#[derive(Clone, Debug)]
struct Body {
    key: EntityKey,
    generation: u32,
    symbol: u32,
    /// The stored content wrapper, verbatim. Revised, never rebuilt.
    content: Value,
    canonical_id: String,
    session: String,
    base_revision: Option<u64>,
    expires_at: Option<u64>,
    residency: Option<String>,
    /// Every relation incident to this body: (key, generation, predicate name).
    edges: Vec<(RelationKey, u32, String)>,
}

/// Classify one body against the criteria. Pure: this is the whole decision, and
/// it is a function of the operator's arguments and the node's OWN data.
///
/// Order matters. `--keep` is evaluated first so a protected body can never be
/// selected by a broad criterion. An already-dormant body is reported as such
/// rather than reaped twice.
fn classify(body: &Body, criteria: &Criteria) -> Verdict {
    let names = |wanted: &str| wanted == body.session || wanted == body.canonical_id;
    if let Some(k) = criteria.keep.iter().find(|k| names(k)) {
        return Verdict::Kept(format!("protected by --keep {k}"));
    }
    if body.residency.as_deref() == Some(DORMANT) {
        return Verdict::AlreadyDormant;
    }
    if let Some(s) = criteria.sessions.iter().find(|s| names(s)) {
        return Verdict::Reap(format!("named by the operator: --session {s}"));
    }
    if criteria.expired {
        match body.expires_at {
            Some(expires_at) if expires_at < criteria.now => {
                return Verdict::Reap(format!(
                    "the body's own expires_at {expires_at} is before now {}",
                    criteria.now
                ));
            }
            Some(expires_at) => {
                return Verdict::Left(format!(
                    "live: its own expires_at {expires_at} is at or after now {}",
                    criteria.now
                ));
            }
            // Missing data is not zero. A body that never recorded a lifetime is
            // `unknown`, not `expired`, and --expired must not touch it.
            None => {
                if criteria.arrived_before_revision.is_none() {
                    return Verdict::Left(
                        "unknown: this body records no expires_at, so --expired cannot judge it"
                            .to_owned(),
                    );
                }
            }
        }
    }
    if let Some(threshold) = criteria.arrived_before_revision {
        match body.base_revision {
            Some(base) if base < threshold => {
                return Verdict::Reap(format!(
                    "arrived at base_revision {base}, before the operator threshold {threshold}"
                ));
            }
            Some(base) => {
                return Verdict::Left(format!(
                    "arrived at base_revision {base}, at or after the operator threshold {threshold}"
                ));
            }
            None => {
                return Verdict::Left(
                    "unknown: this body records no base_revision, so --arrived-before-revision \
                     cannot judge it"
                        .to_owned(),
                );
            }
        }
    }
    Verdict::Left("no criterion selected it".to_owned())
}

// --- arguments -----------------------------------------------------------------

struct Args {
    store_dir: PathBuf,
    criteria: Criteria,
    apply: bool,
    reason: Option<String>,
    by: Option<String>,
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut args = Args {
        store_dir: PathBuf::from("artifacts/ontology-registry/current/store"),
        criteria: Criteria {
            now,
            ..Criteria::default()
        },
        apply: false,
        reason: None,
        by: None,
    };
    let raw: Vec<String> = env::args().skip(1).collect();
    let mut positional_seen = false;
    let mut i = 0;
    while i < raw.len() {
        let arg = raw[i].as_str();
        let mut next = |what: &str| -> Result<String, Box<dyn Error>> {
            i += 1;
            raw.get(i)
                .cloned()
                .ok_or_else(|| format!("{what} requires a value").into())
        };
        match arg {
            "--expired" => args.criteria.expired = true,
            "--arrived-before-revision" => {
                args.criteria.arrived_before_revision = Some(next(arg)?.parse()?);
            }
            "--session" => args.criteria.sessions.push(next(arg)?),
            "--keep" => args.criteria.keep.push(next(arg)?),
            "--now" => args.criteria.now = next(arg)?.parse()?,
            "--reason" => args.reason = Some(next(arg)?),
            "--by" => args.by = Some(next(arg)?),
            "--apply" => args.apply = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown flag {other}").into());
            }
            positional => {
                if positional_seen {
                    return Err(format!("unexpected second store dir {positional}").into());
                }
                args.store_dir = PathBuf::from(positional);
                positional_seen = true;
            }
        }
        i += 1;
    }
    if args.apply {
        // No anonymous reaping, and no revision without a retained reason.
        if args.reason.as_deref().unwrap_or("").trim().is_empty() {
            return Err("--apply requires --reason \"<why>\": a revision with no retained reason \
                        is not admissible"
                .into());
        }
        if args.by.as_deref().unwrap_or("").trim().is_empty() {
            return Err("--apply requires --by \"<actor canonical id>\": every persistent change \
                        is attributable"
                .into());
        }
    }
    if !args.criteria.expired
        && args.criteria.arrived_before_revision.is_none()
        && args.criteria.sessions.is_empty()
    {
        return Err("no criterion given. Pass at least one of --expired, \
                    --arrived-before-revision <N>, --session <id>. This tool holds no default \
                    idea of what 'stale' means."
            .into());
    }
    Ok(args)
}

// --- the run -------------------------------------------------------------------

fn run() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    println!("store dir: {}", args.store_dir.display());
    println!(
        "criteria: expired={} arrived_before_revision={:?} sessions={:?} keep={:?} now={}",
        args.criteria.expired,
        args.criteria.arrived_before_revision,
        args.criteria.sessions,
        args.criteria.keep,
        args.criteria.now
    );
    println!(
        "mode: {}",
        if args.apply {
            "APPLY (commits one atomic transaction)"
        } else {
            "DRY RUN (reports only, writes nothing)"
        }
    );

    let store = UniverseStore::open(&args.store_dir)?;
    let snapshot = store.replay(store.load_snapshot()?)?;
    println!(
        "\nbase revision: {} | entities: {} | relations: {}",
        snapshot.revision.0,
        snapshot.entities.len(),
        snapshot.relations.len()
    );

    let bodies = read_session_bodies(&store, &snapshot)?;
    println!("session bodies present: {}", bodies.len());

    // Classify every body, print the verdict for each, and collect the selected.
    let mut selected: Vec<(Body, String)> = Vec::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for body in &bodies {
        let verdict = classify(body, &args.criteria);
        let (tag, why) = match &verdict {
            Verdict::Reap(why) => ("REAP", why.as_str()),
            Verdict::Kept(why) => ("KEEP", why.as_str()),
            Verdict::Left(why) => ("LEAVE", why.as_str()),
            Verdict::AlreadyDormant => ("DORMANT", "already dormant; nothing to commit"),
        };
        *counts.entry(tag).or_default() += 1;
        println!("  {tag:<8} {}  ({why})", body.canonical_id);
        if let Verdict::Reap(why) = verdict {
            selected.push((body.clone(), why));
        }
    }
    println!(
        "\nverdicts: {}",
        counts
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ")
    );

    if selected.is_empty() {
        println!("\nNothing selected. No event written.");
        return Ok(());
    }
    let edge_count: usize = selected.iter().map(|(b, _)| b.edges.len()).sum();
    println!(
        "selected {} body(ies), severing {edge_count} edge(s); each body is REVISED in place \
         (residency={DORMANT}) and RETAINED — nothing is deleted.",
        selected.len()
    );

    if !args.apply {
        println!(
            "\nDRY RUN: nothing was written. Re-run with --apply --reason \"<why>\" --by \"<actor>\" \
             to commit."
        );
        return Ok(());
    }

    let reason = args.reason.expect("checked in parse_args");
    let by = args.by.expect("checked in parse_args");

    // One idempotency key for THIS selection: a re-run over the same bodies with
    // the same reason is AlreadyCommitted rather than a second event.
    let mut hasher = Sha256::new();
    hasher.update(reason.as_bytes());
    hasher.update(by.as_bytes());
    for (body, _) in &selected {
        hasher.update(body.key.to_string().as_bytes());
    }
    let idempotency_key = format!("reap:session-bodies:v0:{}", hex::encode(hasher.finalize()));
    println!("idempotency key: {idempotency_key}");

    // Optimistic commit with bounded retries. Each attempt RE-READS the committed
    // state: this store has other writers (every live MCP session), so the base
    // revision can move between reading it and committing. The content refs are
    // appended once — content is content-addressed and independent of revision.
    let mut commands = Vec::with_capacity(selected.len() * 2);
    for (body, why) in &selected {
        let mut content = body.content.clone();
        let object = content
            .as_object_mut()
            .ok_or_else(|| format!("body {} content is not an object", body.canonical_id))?;
        object.insert(RESIDENCY_FIELD.to_owned(), json!(DORMANT));
        object.insert(
            REAPING_FIELD.to_owned(),
            json!({
                "reaped_by": by,
                "reason": reason,
                "criterion": why,
                "at_revision": snapshot.revision.0,
                "at_unix": args.criteria.now,
                "severed_edges": body
                    .edges
                    .iter()
                    .map(|(key, _, predicate)| json!({
                        "relation": key.to_string(),
                        "predicate": predicate,
                    }))
                    .collect::<Vec<_>>(),
                "note": "This body is RETAINED, not deleted: the kernel has no entity-delete and \
needs none here. Its edges were severed, so no observation reaches it from the space it used to \
occupy. Re-reading this node is what proves it; nothing about its identity or provenance changed.",
            }),
        );
        let content_ref = store.append_content(&content)?;
        commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: body.key,
                generation: body
                    .generation
                    .checked_add(1)
                    .ok_or("entity generation overflow")?,
                symbol: body.symbol,
                content: Some(content_ref),
            },
        });
        for (relation, generation, _) in &body.edges {
            commands.push(UniverseCommand::TombstoneRelation {
                relation: *relation,
                generation: *generation,
            });
        }
    }
    let command_count = commands.len();

    let mut committed: Option<CommitReceipt> = None;
    let mut last_conflict = None;
    for attempt in 1..=COMMIT_ATTEMPTS {
        let mut live = store.replay(store.load_snapshot()?)?;
        let write_set = UniverseWriteSet {
            base_revision: live.revision,
            idempotency_key: idempotency_key.clone(),
            commands: commands.clone(),
        };
        let boundary_tick = Tick(live.tick.0 + 1);
        let transaction = UniverseTransaction::prepare(&live, write_set)?;
        match transaction.commit(&store, &mut live, boundary_tick) {
            Ok(receipt) => {
                committed = Some(receipt);
                break;
            }
            Err(UniverseError::RevisionConflict { expected, actual }) => {
                println!(
                    "  commit attempt {attempt}/{COMMIT_ATTEMPTS}: another writer moved the store \
                     ({} -> {}); re-reading the committed state and retrying",
                    expected.0, actual.0
                );
                last_conflict = Some((expected, actual));
            }
            Err(other) => return Err(other.into()),
        }
    }
    let receipt = committed.ok_or_else(|| {
        format!(
            "the reaping did not commit in {COMMIT_ATTEMPTS} attempts; the last conflict was \
             {last_conflict:?} (a concurrent writer holds the store). Nothing was committed."
        )
    })?;
    println!("\ncommitted {command_count} command(s) as ONE atomic set");
    println!("commit receipt: {receipt:?}");

    // --- INDEPENDENT READBACK: a fresh reopen, never the handle we wrote with.
    let fresh = UniverseStore::open(&args.store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    println!("\n-- independent readback (fresh reopen) --");
    println!(
        "revision advanced: {} -> {}",
        snapshot.revision.0, after.revision.0
    );

    let mut failures = Vec::new();
    for (body, _) in &selected {
        let Some(entity) = after.entities.iter().find(|e| e.key == body.key) else {
            failures.push(format!(
                "{} is ABSENT on readback — a reaping must never delete a body",
                body.canonical_id
            ));
            continue;
        };
        let content = match entity.content.as_ref() {
            Some(pointer) => fresh.read_content(pointer)?,
            None => {
                failures.push(format!("{} has no content on readback", body.canonical_id));
                continue;
            }
        };
        let residency = content.get(RESIDENCY_FIELD).and_then(Value::as_str);
        if residency != Some(DORMANT) {
            failures.push(format!(
                "{} reads residency={residency:?}, expected {DORMANT:?}",
                body.canonical_id
            ));
        }
        if content.get("canonical_id").and_then(Value::as_str) != Some(body.canonical_id.as_str()) {
            failures.push(format!("{} lost its canonical_id on readback", body.canonical_id));
        }
        if content.get(REAPING_FIELD).and_then(|r| r.get("reason")).is_none() {
            failures.push(format!("{} carries no retained reason", body.canonical_id));
        }
        let remaining = after
            .relations
            .iter()
            .filter(|r| r.source == body.key || r.target == body.key)
            .count();
        if remaining != 0 {
            failures.push(format!(
                "{} still has {remaining} incident relation(s) — it remains reachable",
                body.canonical_id
            ));
        }
        println!(
            "  {}  present={} gen={} residency={:?} incident_edges={remaining}",
            body.canonical_id,
            true,
            entity.generation,
            residency
        );
    }

    // A body the operator PROTECTED must be untouched — proof that reaping is
    // selective, not a sweep.
    for body in &bodies {
        if !matches!(classify(body, &args.criteria), Verdict::Kept(_)) {
            continue;
        }
        let remaining = after
            .relations
            .iter()
            .filter(|r| r.source == body.key || r.target == body.key)
            .count();
        println!(
            "  KEPT {}  incident_edges={remaining} (must be > 0: it is still perceptible)",
            body.canonical_id
        );
        if remaining == 0 {
            failures.push(format!(
                "protected body {} lost its edges — --keep was not honoured",
                body.canonical_id
            ));
        }
    }

    if !failures.is_empty() {
        for failure in &failures {
            eprintln!("  READBACK FAILURE: {failure}");
        }
        return Err(format!("{} readback check(s) failed", failures.len()).into());
    }

    println!(
        "\nRESULT: {} session body(ies) demoted to residency={DORMANT} and severed from their \
         space ({command_count} commands, one atomic set).",
        selected.len()
    );
    println!(
        "        Every one of them still EXISTS, still carries its canonical_id and provenance, \
         and now carries"
    );
    println!(
        "        why it was reaped and by whom. No observation reaches them from the space they \
         occupied."
    );
    Ok(())
}

/// Read every SESSION body the store holds: an entity whose own content carries
/// an `actor:l1:` canonical id AND an `embodied_session`. Both conditions are
/// read from DATA — the key block is never used as a discriminator, and an
/// authored L1 inhabitant (which embodies no session) is never a candidate.
///
/// Content is read lazily, one entity at a time; entities carrying none are
/// skipped.
fn read_session_bodies(
    store: &UniverseStore,
    snapshot: &universe_store::UniverseSnapshot,
) -> Result<Vec<Body>, Box<dyn Error>> {
    let predicate_name = |index: u32| {
        snapshot
            .symbols
            .get(index as usize)
            .cloned()
            .unwrap_or_else(|| format!("symbol#{index}"))
    };
    let mut bodies = Vec::new();
    for entity in &snapshot.entities {
        let Some(pointer) = entity.content.as_ref() else {
            continue;
        };
        let content = store.read_content(pointer)?;
        let Some(canonical_id) = content.get("canonical_id").and_then(Value::as_str) else {
            continue;
        };
        if !canonical_id.starts_with(L1_ACTOR_PREFIX) {
            continue;
        }
        let Some(session) = content.get(SESSION_FIELD).and_then(Value::as_str) else {
            // An L1 actor that embodies no session is an authored inhabitant, not
            // a session body. Never a candidate.
            continue;
        };
        let edges = snapshot
            .relations
            .iter()
            .filter(|r| r.source == entity.key || r.target == entity.key)
            .map(|r| (r.key, r.generation, predicate_name(r.predicate)))
            .collect();
        bodies.push(Body {
            key: entity.key,
            generation: entity.generation,
            symbol: entity.symbol,
            canonical_id: canonical_id.to_owned(),
            session: session.to_owned(),
            base_revision: content.get("base_revision").and_then(Value::as_u64),
            expires_at: content.get("expires_at").and_then(Value::as_u64),
            residency: content
                .get(RESIDENCY_FIELD)
                .and_then(Value::as_str)
                .map(str::to_owned),
            content,
            edges,
        });
    }
    bodies.sort_by(|a, b| a.canonical_id.cmp(&b.canonical_id));
    Ok(bodies)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(session: &str, base_revision: Option<u64>, expires_at: Option<u64>) -> Body {
        Body {
            key: EntityKey(0x0AC0),
            generation: 0,
            symbol: 0,
            content: json!({ "canonical_id": format!("actor:l1:mind:claude-{session}") }),
            canonical_id: format!("actor:l1:mind:claude-{session}"),
            session: session.to_owned(),
            base_revision,
            expires_at,
            residency: None,
            edges: vec![(RelationKey(0x0AC1), 0, "PART_OF".to_owned())],
        }
    }

    #[test]
    fn no_criterion_selects_nothing() {
        let verdict = classify(&body("s1", Some(10), Some(0)), &Criteria::default());
        assert!(matches!(verdict, Verdict::Left(_)), "{verdict:?}");
    }

    #[test]
    fn a_body_with_no_expiry_is_unknown_never_expired() {
        // The measured case: every body written before the expiry fix carries no
        // `expires_at`. Missing data is not zero, and it is not "long ago".
        let criteria = Criteria {
            expired: true,
            now: 1_000_000,
            ..Criteria::default()
        };
        let verdict = classify(&body("legacy", Some(44), None), &criteria);
        match verdict {
            Verdict::Left(why) => assert!(why.contains("unknown"), "{why}"),
            other => panic!("an unrecorded lifetime must never be read as expired: {other:?}"),
        }
    }

    #[test]
    fn expired_is_judged_against_the_bodys_own_expires_at() {
        let criteria = Criteria {
            expired: true,
            now: 1_000,
            ..Criteria::default()
        };
        assert!(matches!(
            classify(&body("old", Some(1), Some(999)), &criteria),
            Verdict::Reap(_)
        ));
        assert!(matches!(
            classify(&body("live", Some(1), Some(1_001)), &criteria),
            Verdict::Left(_)
        ));
        // Exactly `now` is not yet past: expiry is strict.
        assert!(matches!(
            classify(&body("edge", Some(1), Some(1_000)), &criteria),
            Verdict::Left(_)
        ));
    }

    #[test]
    fn the_revision_threshold_is_the_operators_never_a_default() {
        let criteria = Criteria {
            arrived_before_revision: Some(100),
            ..Criteria::default()
        };
        match classify(&body("old", Some(44), None), &criteria) {
            Verdict::Reap(why) => {
                assert!(why.contains("44") && why.contains("100"), "{why}");
            }
            other => panic!("expected a reap naming both numbers: {other:?}"),
        }
        assert!(matches!(
            classify(&body("new", Some(290), None), &criteria),
            Verdict::Left(_)
        ));
    }

    #[test]
    fn keep_wins_over_every_criterion() {
        let criteria = Criteria {
            expired: true,
            arrived_before_revision: Some(u64::MAX),
            sessions: vec!["mine".to_owned()],
            keep: vec!["mine".to_owned()],
            now: u64::MAX,
        };
        // Selected by --session AND by both broad criteria, yet protected.
        assert!(matches!(
            classify(&body("mine", Some(1), Some(1)), &criteria),
            Verdict::Kept(_)
        ));
    }

    #[test]
    fn keep_matches_the_canonical_id_too() {
        let criteria = Criteria {
            arrived_before_revision: Some(u64::MAX),
            keep: vec!["actor:l1:mind:claude-mine".to_owned()],
            ..Criteria::default()
        };
        assert!(matches!(
            classify(&body("mine", Some(1), None), &criteria),
            Verdict::Kept(_)
        ));
    }

    #[test]
    fn an_already_dormant_body_is_not_reaped_twice() {
        let mut b = body("done", Some(1), Some(1));
        b.residency = Some(DORMANT.to_owned());
        let criteria = Criteria {
            expired: true,
            now: u64::MAX,
            ..Criteria::default()
        };
        assert_eq!(classify(&b, &criteria), Verdict::AlreadyDormant);
    }

    #[test]
    fn a_named_session_is_reaped_whatever_its_data_says() {
        // The operator naming a session IS the criterion; it is recorded as such.
        let criteria = Criteria {
            sessions: vec!["junk".to_owned()],
            ..Criteria::default()
        };
        match classify(&body("junk", None, None), &criteria) {
            Verdict::Reap(why) => assert!(why.contains("--session junk"), "{why}"),
            other => panic!("expected a named reap: {other:?}"),
        }
    }

    #[test]
    fn expired_and_revision_threshold_compose_without_swallowing_unknowns() {
        // With BOTH criteria, a body carrying no expiry falls through to the
        // revision test rather than being judged by a lifetime it never recorded.
        let criteria = Criteria {
            expired: true,
            arrived_before_revision: Some(100),
            now: 1_000,
            ..Criteria::default()
        };
        assert!(matches!(
            classify(&body("legacy-old", Some(44), None), &criteria),
            Verdict::Reap(_)
        ));
        assert!(matches!(
            classify(&body("legacy-new", Some(290), None), &criteria),
            Verdict::Left(_)
        ));
    }
}
