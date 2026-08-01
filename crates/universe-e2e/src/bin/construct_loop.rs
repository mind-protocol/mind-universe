//! The serial wake-queue loop: constructs run AS constructs — resolved from the
//! graph, woken by events, drained serially, dormant costs nothing.
//!
//! Phase 1 landed the pieces: a `construct_resolver` turns an authored
//! `alarm_atom_circuit` into runtime `PhysicsWaveInputs`, and `inject_house_alarm`
//! lands the construct into a scratch store as a graph object. But the resolved
//! construct was still driven by a bespoke bin. This closes the loop: a GENERIC
//! serial loop drains a wake-queue, so a construct PRESENT IN THE GRAPH runs as a
//! construct — its physics resolved from its COMMITTED graph content (via
//! `Supervisor::read_content`), driven through the SAME generic
//! `Supervisor::advance_driving_physics_wave` + a `PhysicsWaveSelector`. The loop
//! knows nothing about house-alarms; the selector supplies the wave from the
//! graph.
//!
//! What is proven, per tick:
//!   * DORMANT ticks (no wake) run ZERO waves — `TickOutcome.physics_wave` is
//!     `None`, no candidate, and the committed store is byte-identical. Dormant
//!     constructs cost nothing; the loop waits on the queue, it never polls.
//!   * A WOKEN tick (the driver calls `selector.wake(id)`, modelling a physics
//!     event crossing the sensor) runs exactly one wave: the resolved construct
//!     fires (its trigger AND its terminal emitter), exactly ONE notify candidate
//!     surfaces, and the queue drains to empty.
//!   * It REPEATS: dormant, wake again, fires again — the queue is drained
//!     serially, one woken construct per turn, and re-arms.
//!   * The committed store is byte-identical across the WHOLE loop: a PhysicsEvent
//!     never mutates the store; candidates are proposals, never commits.
//!
//! HONEST BOUNDARIES (inherited from Phase 1):
//!   * The crossing is SIMULATED. The authored circuit seeds the sensor as an
//!     `external_measured_injection`, so a woken tick's sensor fires. On a live
//!     world that energy MUST arrive from the real physics step via the
//!     physics-event -> atom-deposit bridge. A woken tick here stands in for that
//!     crossing; the wake-queue is exactly where a real crossing would enqueue.
//!   * With no pending transactions, an advanced tick commits nothing and the
//!     committed tick does not move — the honest consequence of "only what commits
//!     stays authoritative". The loop is still serial and still drains the queue.
//!
//! Usage: `construct_loop [scratch-store-dir]`
//!   scratch-store-dir defaults to a fresh unique dir under the system temp dir.
//!   NEVER pass the live store: this boots a fresh Genesis and needs an empty dir.

use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

use universe_core::{EntityKey, RelationKey, Tick, UniverseError};
use universe_e2e::canonical::{canonical_predicate, entity_symbol, new_symbols};
use universe_e2e::construct_registry::find_construct_in_snapshot;
use universe_e2e::wave_selector::GraphWaveSelector;
use universe_physics::AtomExecutionBudget;
use universe_store::{EntityRecord, RelationRecord, UniverseSnapshot, UniverseStore};
use universe_supervisor::{PhaseHook, Supervisor, TickPhase};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// Bounded hydration ceiling for the graph-native construct finder. The scratch
/// store holds minimal genesis plus ~15 construct nodes; this is comfortably
/// above that and far below anything resembling a whole-universe scan.
const MAX_HYDRATIONS: usize = 512;

/// The wake schedule: `false` = a dormant tick (no wake), `true` = a woken tick
/// (the driver enqueues the construct, modelling a sensor crossing). Two woken
/// ticks separated by dormant ticks prove the queue drains serially and re-arms.
const SCHEDULE: [bool; 6] = [false, false, true, false, false, true];

/// A trivial no-op phase hook — the loop asserts on the wave, not on phases.
struct NoopHook;
impl PhaseHook for NoopHook {
    fn run(&mut self, _phase: TickPhase, _snapshot: &UniverseSnapshot) -> Result<(), UniverseError> {
        Ok(())
    }
}

/// The caller-supplied (graph-authority) execution budget every wave is bounded
/// by. Mirrors `house_alarm_resolved`: a bounded, finite budget the caller brings.
fn budget() -> AtomExecutionBudget {
    AtomExecutionBudget {
        max_atoms: 16,
        max_bonds: 16,
        max_steps: 16,
        max_total_energy: 10_000,
    }
}

/// The measured facts of one advanced tick.
#[derive(Clone, Copy, Debug)]
struct TickRecord {
    /// Whether the driver enqueued the construct before this tick.
    woken: bool,
    /// Whether the tick ran a physics wave (`TickOutcome.physics_wave.is_some()`).
    wave: bool,
    /// Whether the resolved construct's trigger atom fired this tick.
    fired_trigger: bool,
    /// Whether the resolved construct's terminal emitter atom fired this tick.
    fired_emitter: bool,
    /// How many CANDIDATE effects surfaced this tick.
    candidates: usize,
    /// The wake-queue depth after this tick (drained serially -> 0).
    queue_after: usize,
}

/// The measured outcome of the whole serial loop.
struct LoopOutput {
    /// The `code` node whose COMMITTED content furnished the construct's physics.
    code_node: EntityKey,
    sensor_atoms: usize,
    construct_atoms: usize,
    deposits: usize,
    atom_key_count: usize,
    ticks: Vec<TickRecord>,
    /// The committed store was byte-identical after EVERY tick of the loop.
    store_byte_identical: bool,
}

/// Set up a scratch store containing the injected construct, boot a supervisor,
/// resolve the construct FROM ITS COMMITTED CONTENT, and run the serial
/// wake-queue loop. Returns the measured facts; asserts live in the caller so the
/// bin and the `#[test]` share one execution path.
fn drive_loop(store_dir: &Path, genesis: &Path, fixture: &Path) -> Result<LoopOutput, Box<dyn Error>> {
    // Set up the store so it CONTAINS the construct (Phase-1 inject path).
    inject_house_alarm_into(store_dir, genesis, fixture)?;

    // Boot a supervisor over the store that now contains the construct.
    let mut supervisor = Supervisor::boot(store_dir, genesis)?;

    // Resolve the construct FROM THE COMMITTED GRAPH CONTENT — not the fixture
    // file, not hand-built constants. The finder hydrates committed content via
    // `Supervisor::read_content` and picks the node carrying an alarm_atom_circuit.
    let snapshot = supervisor.snapshot().clone();
    let registered = find_construct_in_snapshot(&supervisor, &snapshot, MAX_HYDRATIONS)
        .map_err(|error| format!("find_construct_in_snapshot failed: {error:?}"))?;
    let alarm_trigger = *registered
        .resolved
        .atom_keys
        .get("alarm_trigger")
        .ok_or("resolved circuit has no alarm_trigger atom")?;
    let notify_emitter = *registered
        .resolved
        .atom_keys
        .get("notify_emitter")
        .ok_or("resolved circuit has no notify_emitter atom")?;

    // Build the generic selector and register the resolved construct under its id.
    let mut selector = GraphWaveSelector::new(budget());
    selector.register(registered.code_node, registered.resolved.clone());

    // The committed store BEFORE the loop begins. It must not move for any tick.
    let bytes_before = read_all_files(store_dir)?;
    let mut hook = NoopHook;

    let mut ticks = Vec::with_capacity(SCHEDULE.len());
    let mut store_byte_identical = true;
    for &woken in &SCHEDULE {
        // A woken tick: the driver delivers a physics event by enqueuing the
        // construct (models the sensor crossing). A dormant tick enqueues nothing.
        if woken {
            selector.wake(registered.code_node);
        }
        // The GENERIC advance drives the wave from the selector — the supervisor
        // never names a construct. `&self` wave; the store is not mutated.
        let outcome = supervisor.advance_driving_physics_wave(&mut hook, &mut selector)?;
        let wave = outcome.physics_wave.as_ref();
        ticks.push(TickRecord {
            woken,
            wave: wave.is_some(),
            fired_trigger: wave.is_some_and(|w| w.fired_construct_atoms.contains(&alarm_trigger)),
            fired_emitter: wave.is_some_and(|w| w.fired_construct_atoms.contains(&notify_emitter)),
            candidates: wave.map_or(0, |w| w.candidate_effects.len()),
            queue_after: selector.queue_len(),
        });
        // A PhysicsEvent never mutates the store — prove it byte-for-byte, per tick.
        if read_all_files(store_dir)? != bytes_before {
            store_byte_identical = false;
        }
    }

    Ok(LoopOutput {
        code_node: registered.code_node,
        sensor_atoms: registered.resolved.sensor_cluster.atoms.len(),
        construct_atoms: registered.resolved.construct_cluster.atoms.len(),
        deposits: registered.resolved.deposit_bindings.len(),
        atom_key_count: registered.resolved.atom_keys.len(),
        ticks,
        store_byte_identical,
    })
}

/// Assert the loop's measured facts match the wake schedule. Shared by the bin
/// and the `#[test]` so both prove exactly the same thing.
fn assert_expectations(out: &LoopOutput) -> Result<(), String> {
    if out.ticks.len() != SCHEDULE.len() {
        return Err(format!("expected {} ticks, got {}", SCHEDULE.len(), out.ticks.len()));
    }
    let mut woken_fired = 0usize;
    for (index, tick) in out.ticks.iter().enumerate() {
        if tick.queue_after != 0 {
            return Err(format!("tick {index}: queue not drained ({} left)", tick.queue_after));
        }
        if tick.woken {
            // A woken tick fires the construct and surfaces exactly one candidate.
            if !tick.wave {
                return Err(format!("tick {index}: woken but ran no wave"));
            }
            if !tick.fired_trigger {
                return Err(format!("tick {index}: woken but the trigger atom did not fire"));
            }
            if !tick.fired_emitter {
                return Err(format!("tick {index}: woken but the terminal emitter did not fire"));
            }
            if tick.candidates != 1 {
                return Err(format!(
                    "tick {index}: woken but {} candidates surfaced (expected exactly 1)",
                    tick.candidates
                ));
            }
            woken_fired += 1;
        } else {
            // A dormant tick runs no wave, no candidate — dormant costs nothing.
            if tick.wave {
                return Err(format!("tick {index}: dormant but ran a wave"));
            }
            if tick.candidates != 0 {
                return Err(format!("tick {index}: dormant but {} candidates surfaced", tick.candidates));
            }
        }
    }
    let expected_woken = SCHEDULE.iter().filter(|&&w| w).count();
    if woken_fired != expected_woken {
        return Err(format!("expected {expected_woken} woken-and-fired ticks, got {woken_fired}"));
    }
    if !out.store_byte_identical {
        return Err("committed store changed during the loop — a PhysicsEvent must not mutate it".into());
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("CONSTRUCT LOOP FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let store_dir_arg = env::args_os().nth(1).map(PathBuf::from);
    let store_dir = store_dir_arg.clone().unwrap_or_else(default_scratch_store);
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let genesis = repo.join("fixtures/genesis/minimal-genesis.json");
    let fixture = repo.join("fixtures/ontology/lumina-prime-house-alarm-v0.json");
    fs::create_dir_all(&store_dir)?;
    println!("scratch store: {}", store_dir.display());
    println!("genesis      : {}", genesis.display());
    println!("construct    : {}", fixture.display());

    let out = drive_loop(&store_dir, &genesis, &fixture)?;

    println!("\n-- construct resolved FROM COMMITTED GRAPH CONTENT (read_content, not the fixture file) --");
    println!("code node (wake id) : {:#x}", out.code_node.0);
    println!(
        "resolved clusters   : sensor {} atoms, construct {} atoms, {} deposit(s), {} atom keys",
        out.sensor_atoms, out.construct_atoms, out.deposits, out.atom_key_count
    );

    println!("\n-- serial wake-queue loop ({} ticks) --", out.ticks.len());
    for (index, tick) in out.ticks.iter().enumerate() {
        if tick.woken {
            println!(
                "  tick {index}: WOKEN  -> wave={} trigger_fired={} emitter_fired={} candidates={} queue_after={}",
                tick.wave, tick.fired_trigger, tick.fired_emitter, tick.candidates, tick.queue_after
            );
        } else {
            println!(
                "  tick {index}: dormant-> wave={} candidates={} queue_after={}  (construct never ran)",
                tick.wave, tick.candidates, tick.queue_after
            );
        }
    }
    println!(
        "\ncommitted store byte-identical across the WHOLE loop: {}",
        out.store_byte_identical
    );

    assert_expectations(&out).map_err(|error| format!("loop expectations failed: {error}"))?;

    println!("\n=================================================================================");
    println!("constructs run AS constructs — resolved from the graph, woken by events,");
    println!("drained serially, dormant costs nothing.");
    println!("=================================================================================");
    println!("  (a) the construct's wave came from COMMITTED graph content (read_content),");
    println!("      not the fixture file and not hand-built AtomSpec/AtomBond constants;");
    println!("  (b) dormant ticks ran ZERO waves with the store byte-identical;");
    println!("  (c) a woken tick fired the construct (trigger + terminal emitter) and");
    println!("      surfaced exactly ONE notify candidate, then drained the queue to empty;");
    println!("  (d) it repeated — dormant, wake, fire again — the queue drains serially and re-arms;");
    println!("  (e) ZERO new symbols + committed store byte-identical across the loop.");
    println!("  HONEST BOUNDARY: the crossing is simulated (a woken tick stands in for a real");
    println!("  sensor collision); no store commit means the committed tick does not advance.");

    if store_dir_arg.is_none() {
        let _ = fs::remove_dir_all(&store_dir);
    }
    Ok(())
}

/// Inject the Lumina Prime house-alarm construct into a fresh SCRATCH store as a
/// graph object — ONE atomic transaction, ZERO non-canonical symbols. Mirrors
/// `bin/inject_house_alarm` (the Phase-1 inject path) so this loop runs over a
/// store that actually CONTAINS the construct. Reuses the shared canonical remap
/// (`universe_e2e::canonical`); an authored predicate absent from that table is a
/// hard error, never minted.
fn inject_house_alarm_into(
    store_dir: &Path,
    genesis: &Path,
    fixture: &Path,
) -> Result<(), Box<dyn Error>> {
    // Disjoint key block for this construct (same window as inject_house_alarm).
    const ENTITY_BASE: u128 = 0xD000;
    const REL_BASE: u128 = 0xDD00;

    fs::create_dir_all(store_dir)?;
    // Boot genesis (writes the checkpoint), scoped so the store handle drops
    // before we reopen the store for the injection.
    {
        Supervisor::boot(store_dir, genesis)?;
    }

    // Parse the portable projection.
    let doc: Value = serde_json::from_slice(&fs::read(fixture)?)?;
    let root_id = doc
        .get("id")
        .and_then(Value::as_str)
        .ok_or("fixture has no top-level id")?
        .to_string();

    struct Node {
        id: String,
        node_type: String,
        subtype: String,
        content: Value,
    }
    let node_from = |v: &Value| -> Result<Node, Box<dyn Error>> {
        Ok(Node {
            id: v
                .get("id")
                .and_then(Value::as_str)
                .ok_or("node without id")?
                .to_string(),
            node_type: v
                .get("node_type")
                .and_then(Value::as_str)
                .unwrap_or("thing")
                .to_string(),
            subtype: v
                .get("subtype")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            content: v.get("content").cloned().unwrap_or(Value::Null),
        })
    };
    let mut nodes: Vec<Node> = vec![node_from(&doc)?];
    for member in doc
        .get("members")
        .and_then(Value::as_array)
        .ok_or("fixture has no members array")?
    {
        nodes.push(node_from(member)?);
    }

    // id -> EntityKey (ordered, deterministic).
    let mut id_to_key: BTreeMap<String, EntityKey> = BTreeMap::new();
    for (index, node) in nodes.iter().enumerate() {
        if id_to_key
            .insert(node.id.clone(), EntityKey(ENTITY_BASE + index as u128))
            .is_some()
        {
            return Err(format!("duplicate node id {}", node.id).into());
        }
    }
    let _ = &root_id; // root is one of the nodes; kept for parity/readability.

    // Partition relations: keep only those whose BOTH endpoints are injected,
    // remapping every authored predicate through the shared canonical table.
    struct Rel {
        source: EntityKey,
        target: EntityKey,
        predicate: String,
    }
    let empty: Vec<Value> = Vec::new();
    let mut kept: Vec<Rel> = Vec::new();
    for r in doc.get("relations").and_then(Value::as_array).unwrap_or(&empty) {
        let source = r.get("source").and_then(Value::as_str).unwrap_or("");
        let target = r.get("target").and_then(Value::as_str).unwrap_or("");
        let authored = r.get("predicate").and_then(Value::as_str).unwrap_or("");
        let (predicate, swap) = canonical_predicate(authored).ok_or_else(|| {
            format!("authored predicate {authored} has no canonical mapping (fail-closed)")
        })?;
        if let (Some(s), Some(t)) = (id_to_key.get(source), id_to_key.get(target)) {
            let (src, tgt) = if swap { (*t, *s) } else { (*s, *t) };
            kept.push(Rel {
                source: src,
                target: tgt,
                predicate: predicate.to_string(),
            });
        }
        // Endpoints not in the injected set (e.g. the parent city) are dropped,
        // exactly as inject_house_alarm reports — never dangled.
    }

    // Open the scratch store and replay to the authoritative snapshot.
    let store = UniverseStore::open(store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let base_revision = snapshot.revision;

    // Requested symbols: node_type/subtype symbols + remapped predicates. The
    // hard conformance bar: NONE of them may be non-canonical.
    let mut requested: Vec<String> = Vec::new();
    for node in &nodes {
        requested.push(entity_symbol(&node.node_type, &node.subtype));
    }
    for r in &kept {
        requested.push(r.predicate.clone());
    }
    requested.sort();
    requested.dedup();
    let non_canonical = new_symbols(requested.iter().map(String::as_str));
    if !non_canonical.is_empty() {
        return Err(format!(
            "conformance violation: injection would use NON-canonical symbols {non_canonical:?}"
        )
        .into());
    }

    let plan = snapshot.plan_symbol_interning(&requested)?;
    let sym = |name: &str| -> Result<u32, Box<dyn Error>> {
        plan.assignments
            .get(name)
            .copied()
            .ok_or_else(|| format!("symbol {name} was not planned").into())
    };

    // Build the atomic write-set: intern canonical symbols, then space + members,
    // then intra-graph edges — one atomic set.
    let mut commands = Vec::new();
    if !plan.additions.is_empty() {
        commands.push(UniverseCommand::InternSymbols {
            symbols: plan.additions.clone(),
        });
    }
    for node in &nodes {
        let content = serde_json::json!({
            "canonical_id": node.id,
            "node_type": node.node_type,
            "subtype": node.subtype,
            "content": node.content,
        });
        commands.push(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: id_to_key[&node.id],
                generation: 0,
                symbol: sym(&entity_symbol(&node.node_type, &node.subtype))?,
                content: Some(store.append_content(&content)?),
            },
        });
    }
    for (index, r) in kept.iter().enumerate() {
        commands.push(UniverseCommand::PutRelation {
            relation: RelationRecord {
                key: RelationKey(REL_BASE + index as u128),
                generation: 0,
                source: r.source,
                target: r.target,
                predicate: sym(&r.predicate)?,
                content: None,
            },
        });
    }

    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: "mutation:lumina-house-alarm:v0".to_string(),
        commands,
    };
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    transaction.commit(&store, &mut snapshot, boundary_tick)?;
    Ok(())
}

fn default_scratch_store() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    env::temp_dir().join(format!("construct-loop-{}-{nanos}", std::process::id()))
}

/// Read every file under `dir` into a path-keyed map of raw bytes, for a literal
/// byte-identity comparison of the committed store across the loop.
fn read_all_files(dir: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, Box<dyn Error>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.clone(), fs::read(&path)?);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serial wake-queue loop over a fresh tempdir scratch store: N dormant
    /// ticks run 0 waves with the store byte-identical; a woken tick runs 1 wave,
    /// fires the construct and surfaces 1 candidate; a second woken tick fires
    /// again; and the committed store is byte-identical throughout. The
    /// construct's physics is resolved from COMMITTED graph content.
    #[test]
    fn serial_wake_queue_drains_constructs_without_mutating_the_store() {
        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("store");
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let genesis = repo.join("fixtures/genesis/minimal-genesis.json");
        let fixture = repo.join("fixtures/ontology/lumina-prime-house-alarm-v0.json");

        let out = drive_loop(&store_dir, &genesis, &fixture).unwrap();

        // The shared expectation gate: dormant -> 0 waves; woken -> fired + 1
        // candidate + drained; store byte-identical throughout.
        assert_expectations(&out).unwrap();

        // Explicit, independent restatement of the load-bearing facts.
        let dormant_waves = out.ticks.iter().filter(|t| !t.woken && t.wave).count();
        assert_eq!(dormant_waves, 0, "a dormant tick must run no wave");

        let woken: Vec<&TickRecord> = out.ticks.iter().filter(|t| t.woken).collect();
        assert_eq!(woken.len(), 2, "the schedule wakes the construct twice");
        for tick in &woken {
            assert!(tick.wave, "a woken tick runs a wave");
            assert!(tick.fired_trigger, "a woken tick fires the trigger");
            assert!(tick.fired_emitter, "a woken tick fires the terminal emitter");
            assert_eq!(tick.candidates, 1, "a woken tick surfaces exactly one candidate");
            assert_eq!(tick.queue_after, 0, "the queue drains to empty");
        }

        assert!(out.store_byte_identical, "committed store byte-identical across the loop");
        // The construct came from committed content: it has a real code node id
        // and the resolver split it into a sensor half and a 2-atom construct half.
        assert!(out.code_node.0 >= 0xD000);
        assert_eq!(out.construct_atoms, 2);
        assert!(out.sensor_atoms >= 1);
        assert_eq!(out.deposits, 1);
    }
}
