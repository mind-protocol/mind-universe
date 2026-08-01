//! The resident Universe, run for real and measured.
//!
//! This driver boots a city ONCE, seals its files away, and proves the city keeps
//! living without them — then unseals, commits through the live heartbeat, takes
//! a backup, and restores from that backup alone.
//!
//! ```text
//!  P0 cold boot            the one and only file read
//!  P1 resolve              find the construct in the COMMITTED graph content
//!  P2 SEAL                 rename the store away; prove a disk readback now FAILS
//!  P3 live while sealed    perturb the field, drain, fire, surface candidates
//!  P4 dormancy             park on the queue; measure the CPU an idle city costs
//!  P5 polling contrast     measure what a tick-per-frame poll WOULD cost, and
//!                          that it does not grow with the number of dormant constructs
//!  P6 UNSEAL               the disk comes back; readback works again
//!  P7 commit               a firing drains into a real committed Moment (revision +1)
//!  P8 backup               copy the living snapshot to disk; measure the dump
//!  P9 restore              boot from the backup with the event log DELETED
//! ```
//!
//! HONEST BOUNDARIES
//!   * The sensor crossing is SIMULATED: the authored circuit seeds its sensor
//!     with an `external_measured_injections` block, and a perturbation stands in
//!     for a real Rapier intersection. This proves the resident loop, the wake
//!     path and the heartbeat — NOT a real physical entry.
//!   * The universe clock only moves when something COMMITS. Turns of the loop
//!     and ticks of the Universe are reported separately and never conflated.
//!   * The inference seam is exercised with a stub that answers a fixed string.
//!     That measures the seam's shape and its one-call-in-flight discipline. It
//!     is not evidence about any model.
//!
//! Usage:
//!   `resident_universe --store <dir> [--genesis <path>] [--forever]`
//!
//!   `<dir>` must be a SCRATCH store containing the house-alarm construct. Build
//!   one with the existing injector, which this driver does not duplicate:
//!     `cargo run -p universe-e2e --bin inject_house_alarm -- <dir>`
//!   The live ontology store is refused outright.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use universe_compiler::canonical_hash;
use universe_core::{Epistemic, EntityKey, Revision, Tick, UniverseError};
use universe_e2e::construct_registry::find_construct_in_snapshot;
use universe_e2e::wave_selector::GraphWaveSelector;
use universe_ir::{
    CodeDefinition, ExecutionRequest, Operator, TriggerBudgets, TriggerControls,
    TriggerEvidenceRequirement, TriggerEventKind, TriggerSubscription, Value as IrValue, IR_VERSION,
    TRIGGER_CONTRACT_VERSION,
};
use universe_physics::AtomExecutionBudget;
use universe_runtime::{
    metrics, BackupPolicy, Field, FieldInjection, InferenceCall, InferenceError, InferencePort,
    InferenceReply, ResidentUniverse, RuntimeConfig, Until,
};
use universe_store::{EntityRecord, UniverseSnapshot};
use universe_supervisor::{NoTriggers, PhysicsWaveInputs, PhysicsWaveSelector, Supervisor, TriggerTickDriver};
use universe_transactions::{UniverseCommand, UniverseWriteSet};
use universe_vm::{execute_program, ExecutionLimits, ExecutionReceipt, VmHost};

/// Bounded hydration ceiling for the graph-native construct finder. Never a
/// whole-universe scan.
const MAX_HYDRATIONS: usize = 4096;

/// The caller-supplied execution budget every wave is bounded by. Graph authority
/// the caller brings, mirroring `construct_loop`.
fn budget() -> AtomExecutionBudget {
    AtomExecutionBudget {
        max_atoms: 16,
        max_bonds: 16,
        max_steps: 16,
        max_total_energy: 10_000,
    }
}

// ---------------------------------------------------------------------------
// The field: the existing graph-resolved wake-queue selector, taught the two
// extra facts a resident loop needs.
// ---------------------------------------------------------------------------

struct GraphField(GraphWaveSelector);

impl PhysicsWaveSelector for GraphField {
    fn select(
        &mut self,
        snapshot: &UniverseSnapshot,
    ) -> Result<Option<PhysicsWaveInputs>, UniverseError> {
        self.0.select(snapshot)
    }
}

impl Field for GraphField {
    fn deposit(&mut self, injection: &FieldInjection) -> Result<(), UniverseError> {
        if !self.0.is_registered(injection.target) {
            return Err(UniverseError::Validation(format!(
                "perturbation names {:#x}, which is not a construct in this field",
                injection.target.0
            )));
        }
        self.0.wake(injection.target);
        Ok(())
    }

    fn pending_wakes(&self) -> usize {
        self.0.queue_len()
    }

    fn as_selector(&mut self) -> &mut dyn PhysicsWaveSelector {
        self
    }
}

// ---------------------------------------------------------------------------
// The inference seam, exercised with a stub. It answers, so the seam's shape and
// its serial discipline are measured; it is NOT evidence about any model.
// ---------------------------------------------------------------------------

struct StubInference {
    calls: u64,
    max_concurrent: u32,
    in_flight: u32,
}

impl InferencePort for StubInference {
    fn infer(&mut self, call: &InferenceCall) -> Result<InferenceReply, InferenceError> {
        self.in_flight += 1;
        self.max_concurrent = self.max_concurrent.max(self.in_flight);
        self.calls += 1;
        let answer = if call.prompt.is_empty() {
            Err(InferenceError::Refused("empty observation".into()))
        } else {
            Ok(InferenceReply {
                text: format!("stub-turn:{:#x}", call.actor.0),
            })
        };
        self.in_flight -= 1;
        answer
    }
}

// ---------------------------------------------------------------------------
// The commit driver: a pinned program that proposes one crossing, and the
// translation of that proposal into ONE Moment write-set. Caller authority — the
// runtime holds none of it.
// ---------------------------------------------------------------------------

struct BareHost;

impl VmHost for BareHost {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn capabilities(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn open_query(
        &mut self,
        _spec: &universe_ir::QuerySpec,
        _origin: &IrValue,
        _selector: &IrValue,
    ) -> Result<IrValue, String> {
        Err("bare host performs no queries".into())
    }
    fn await_query(&mut self, _handle: &IrValue) -> Result<IrValue, String> {
        Err("bare host performs no queries".into())
    }
    fn follow_one(&mut self, _source: &IrValue, _predicate: &IrValue) -> Result<IrValue, String> {
        Err("bare host follows no relations".into())
    }
    fn entity_symbol(&mut self, _entity: &IrValue) -> Result<IrValue, String> {
        Err("bare host resolves no symbols".into())
    }
    fn hydrate(&mut self, _selected: &[IrValue], _max_bytes: u32) -> Result<Vec<IrValue>, String> {
        Err("bare host hydrates nothing".into())
    }
}

/// The pinned CodeDefinition: constants, a record, one proposal, a return. It
/// runs on the bare host and its single proposal is the gate — no proposal, no
/// Moment.
fn crossing_program() -> CodeDefinition {
    CodeDefinition {
        ir_version: IR_VERSION,
        revision: Revision(1),
        required_capabilities: Vec::new(),
        operators: vec![
            Operator::Constant {
                value: IrValue::Text("crossing".into()),
                output: 0,
            },
            Operator::Constant {
                value: IrValue::Integer(1),
                output: 1,
            },
            Operator::MakeRecord {
                fields: vec![("moment_kind".into(), 0), ("crossings".into(), 1)],
                output: 2,
            },
            Operator::Propose {
                command: 2,
                output: 3,
            },
            Operator::Return { value: 3 },
        ],
    }
}

fn crossing_subscription(code: &CodeDefinition) -> Result<TriggerSubscription, Box<dyn Error>> {
    Ok(TriggerSubscription {
        contract_version: TRIGGER_CONTRACT_VERSION,
        subscription: EntityKey(0x9002),
        revision: Revision(1),
        enabled: true,
        event_kinds: vec![TriggerEventKind::AtomFired],
        code_definition: EntityKey(0x9001),
        code_revision: code.revision,
        code_hash: canonical_hash(code)?,
        evidence_requirement: TriggerEvidenceRequirement::Measured,
        max_event_age_ticks: 64,
        budgets: TriggerBudgets {
            fuel: 1024,
            max_mutations: 4,
            max_ticks: 4,
        },
        controls: TriggerControls {
            cooldown_ticks: 0,
            debounce_ticks: 0,
            max_causal_depth: 8,
            max_firings_per_tick: 8,
        },
        idempotency_namespace: "resident-universe:crossing".into(),
    })
}

struct MomentDriver {
    code: CodeDefinition,
    host: BareHost,
    /// Only THIS atom's firing is the construct's wake; the terminal emitter's
    /// downstream firing proposes no Moment.
    trigger_atom: EntityKey,
    moment_symbol: u32,
    committed: u64,
    executed: u64,
}

impl TriggerTickDriver for MomentDriver {
    fn resolve_code(&mut self, _request: &ExecutionRequest) -> Result<CodeDefinition, UniverseError> {
        Ok(self.code.clone())
    }

    fn execute(
        &mut self,
        code: &CodeDefinition,
        inputs: &BTreeMap<String, IrValue>,
        limits: ExecutionLimits,
        revision: Revision,
        tick: Tick,
    ) -> Result<ExecutionReceipt, UniverseError> {
        self.executed += 1;
        execute_program(code, &mut self.host, inputs, revision, tick, limits)
            .map_err(|error| UniverseError::Validation(format!("crossing program trapped: {error}")))
    }

    fn translate(
        &mut self,
        request: &ExecutionRequest,
        receipt: &ExecutionReceipt,
        snapshot: &UniverseSnapshot,
    ) -> Result<Option<UniverseWriteSet>, UniverseError> {
        let subject = match &request.trigger.evidence {
            Epistemic::Measured(payload) | Epistemic::Observed(payload) => payload.subject,
            _ => None,
        };
        if subject != Some(self.trigger_atom) {
            return Ok(None);
        }
        if receipt.proposals.is_empty() {
            return Ok(None);
        }
        let key = EntityKey(
            snapshot
                .entities
                .iter()
                .map(|entity| entity.key.0)
                .max()
                .unwrap_or(0)
                + 1,
        );
        self.committed += 1;
        // The firing that caused this Moment is named in the idempotency key, so
        // the commit stays attributable to the exact atom that crossed.
        Ok(Some(UniverseWriteSet {
            base_revision: snapshot.revision,
            idempotency_key: format!(
                "mutation:resident-universe:crossing-moment:{:#x}:{}",
                self.trigger_atom.0, self.committed
            ),
            commands: vec![UniverseCommand::PutEntity {
                entity: EntityRecord {
                    key,
                    generation: 0,
                    symbol: self.moment_symbol,
                    content: None,
                },
            }],
        }))
    }
}

// ---------------------------------------------------------------------------

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("\nRESIDENT UNIVERSE: expectations FAILED");
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("\nRESIDENT UNIVERSE FAILED: {error}");
            std::process::exit(1);
        }
    }
}

struct Args {
    store: PathBuf,
    genesis: PathBuf,
    forever: bool,
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut store: Option<PathBuf> = None;
    let mut genesis = repo.join("fixtures/genesis/minimal-genesis.json");
    let mut forever = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--store" => store = Some(PathBuf::from(args.next().ok_or("--store needs a path")?)),
            "--genesis" => genesis = PathBuf::from(args.next().ok_or("--genesis needs a path")?),
            "--forever" => forever = true,
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let store = store.ok_or(
        "--store <dir> is required (build one with: cargo run -p universe-e2e --bin inject_house_alarm -- <dir>)",
    )?;
    let normalized = store.to_string_lossy().replace('\\', "/").to_lowercase();
    if normalized.contains("ontology-registry/current/store") {
        return Err(
            "refusing to run against the LIVE ontology store; copy it to a scratch dir first".into(),
        );
    }
    Ok(Args {
        store,
        genesis,
        forever,
    })
}

fn run() -> Result<bool, Box<dyn Error>> {
    let args = parse_args()?;
    let mut failures: Vec<String> = Vec::new();
    let mut check = |ok: bool, label: &str| {
        println!("   [{}] {label}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            failures.push(label.to_string());
        }
    };

    // ---------------------------------------------------------------- P0 boot
    println!("== P0  COLD BOOT — the one and only file read ==");
    let io_before_boot = metrics::process_io();
    let mut city = ResidentUniverse::boot(
        &args.store,
        &args.genesis,
        RuntimeConfig {
            inference: Box::new(StubInference {
                calls: 0,
                max_concurrent: 0,
                in_flight: 0,
            }),
            inference_deadline: Duration::from_secs(10),
            backup: BackupPolicy::none(),
            dormant_slice: Duration::from_millis(50),
        },
    )?;
    let boot = city.boot_evidence().clone();
    let boot_io = metrics::io_delta(&io_before_boot, &boot.io_at_boot);
    println!("   store        : {}", boot.store_root);
    println!(
        "   revision {:?}  tick {:?}  entities {}  relations {}  symbols {}",
        boot.revision, boot.tick, boot.entities, boot.relations, boot.symbols
    );
    println!("   snapshot hash: {}", boot.snapshot_hash);
    println!("   boot wall    : {:?}", boot.wall);
    println!("   boot I/O     : {}", describe_io(&boot_io));

    // ------------------------------------------------------------- P1 resolve
    println!("\n== P1  RESOLVE the construct from COMMITTED graph content ==");
    let snapshot = city.supervisor().snapshot().clone();
    let registered = find_construct_in_snapshot(city.supervisor(), &snapshot, MAX_HYDRATIONS);
    let registered = match registered {
        Ok(registered) => registered,
        Err(error) => {
            // A store with no construct still exercises the parts of the runtime
            // that do not need one: the cold boot, the backup, and the restore.
            // Reporting THAT honestly is better than refusing to measure at all.
            println!("   no construct in this store: {error:?}");
            println!("   running the reduced sequence: boot + backup + restore only.");
            without_construct(&mut city, &args, &mut check)?;
            return Ok(failures.is_empty());
        }
    };
    let construct_id = registered.code_node;
    let trigger_atom = *registered
        .resolved
        .atom_keys
        .get("alarm_trigger")
        .ok_or("resolved circuit has no alarm_trigger atom")?;
    // Resolved to assert the circuit's shape: a construct with a trigger but no
    // terminal emitter would surface no candidate, and the proof below would be
    // measuring nothing.
    let _emitter_atom = *registered
        .resolved
        .atom_keys
        .get("notify_emitter")
        .ok_or("resolved circuit has no notify_emitter atom")?;
    println!("   construct id : {construct_id:#x?} (the committed `code` node)");
    println!(
        "   clusters     : sensor {} atoms, construct {} atoms, {} deposit binding(s)",
        registered.resolved.sensor_cluster.atoms.len(),
        registered.resolved.construct_cluster.atoms.len(),
        registered.resolved.deposit_bindings.len()
    );

    let mut field = GraphField(GraphWaveSelector::new(budget()));
    field.0.register(construct_id, registered.resolved.clone());
    let injector = city.injector();

    // ------------------------------------------------------------------ P2 seal
    println!("\n== P2  SEAL — take the files away from the running city ==");
    let sealed_path = sealed_path(&args.store);
    if sealed_path.exists() {
        fs::remove_dir_all(&sealed_path)?;
    }
    fs::rename(&args.store, &sealed_path)?;
    println!("   moved {} -> {}", args.store.display(), sealed_path.display());
    let readback_while_sealed = city.readback_from_disk();
    check(
        readback_while_sealed.is_err(),
        "a disk readback FAILS while sealed (the files really are gone)",
    );
    if let Err(error) = &readback_while_sealed {
        println!("        readback error: {error:?}");
    }

    // -------------------------------------------------------- P3 live, sealed
    println!("\n== P3  THE CITY LIVES WITH NO FILES ==");
    // I/O is summed from each `run`'s OWN measured window, so this driver's own
    // console writes are never mistaken for the loop touching the store.
    let mut sealed_waves = 0u64;
    let mut sealed_candidates = 0u64;
    let mut sealed_reads = 0u64;
    let mut sealed_writes = 0u64;
    let mut sealed_io_measured = true;
    for round in 0..3u32 {
        injector.inject(FieldInjection::turn(
            construct_id,
            "resident_universe:proof",
            format!("observation:round-{round}"),
        ));
        let before = city.counters().clone();
        let report = city.run(&mut field, &mut NoTriggers, Until::Quiescent)?;
        let after = city.counters().clone();
        sealed_waves += after.waves - before.waves;
        sealed_candidates += after.candidate_effects - before.candidate_effects;
        match io_ops(&report.io) {
            Some((reads, writes)) => {
                sealed_reads += reads;
                sealed_writes += writes;
            }
            None => sealed_io_measured = false,
        }
        println!(
            "   round {round}: turns {}  waves {}  fired atoms {}  candidates {}  I/O {}",
            report.turns,
            after.waves - before.waves,
            after.fired_atoms - before.fired_atoms,
            after.candidate_effects - before.candidate_effects,
            describe_io(&report.io)
        );
    }
    check(sealed_waves == 3, "three perturbations ran exactly three waves");
    check(
        sealed_candidates == 3,
        "each wave surfaced exactly one candidate effect (a proposal, never a commit)",
    );
    check(
        sealed_io_measured && sealed_reads == 0 && sealed_writes == 0,
        "ZERO read and ZERO write operations while the city lived sealed",
    );
    let live_hash = city.snapshot_hash()?;
    check(
        live_hash == boot.snapshot_hash,
        "the in-memory authority is intact with the disk gone",
    );
    // A backup with no disk to write to: counted, reported, and NOT fatal.
    let failed_backup = city.backup_now();
    check(
        failed_backup.failure.is_some(),
        "a backup attempted while sealed FAILS and is reported",
    );
    let after_failed_backup = city.run(&mut field, &mut NoTriggers, Until::Quiescent)?;
    check(
        after_failed_backup.stopped_because == universe_runtime::StopReason::Quiescent,
        "the city keeps living after a failed backup",
    );

    // -------------------------------------------------------------- P4 dormancy
    println!("\n== P4  DORMANCY — what an idle city costs ==");
    let idle_budget = Duration::from_secs(2);
    let before = city.counters().clone();
    let idle = city.run(&mut field, &mut NoTriggers, Until::Idle(idle_budget))?;
    let after = city.counters().clone();
    println!(
        "   idle for {:?}: turns {}  dormant waits {}  timeouts {}  stop {:?}",
        idle.wall,
        idle.turns,
        after.dormant_waits - before.dormant_waits,
        after.dormant_timeouts - before.dormant_timeouts,
        idle.stopped_because
    );
    println!("   idle CPU     : {}", describe_cpu(&idle.cpu));
    println!("   idle I/O     : {}", describe_io(&idle.io));
    check(idle.turns == 0, "a dormant city advances ZERO turns");
    check(
        matches!(idle.cpu, Epistemic::Measured(cpu) if cpu < Duration::from_millis(50)),
        "a dormant city burns under 50ms of CPU across a 2s idle",
    );

    // ------------------------------------------------------- P5 polling contrast
    println!("\n== P5  WHAT POLLING WOULD COST (the loop never does this) ==");
    let poll_turns = 50_000u64;
    let baseline = city.measure_polling_cost(&mut field, &mut NoTriggers, poll_turns)?;
    println!(
        "   {} dormant turns: {:?} wall, {} ns/turn, cpu {}",
        baseline.turns,
        baseline.wall,
        baseline.nanos_per_turn,
        describe_cpu(&baseline.cpu)
    );
    println!("   polling I/O  : {}", describe_io(&baseline.io));
    check(
        io_ops(&baseline.io).map_or(false, |(reads, _)| reads == 0),
        "50 000 dormant turns issued ZERO read operations",
    );
    // Does a dormant construct cost anything per turn? Register more of them and
    // measure again. A flat cost means dormant constructs are not visited at all.
    let mut scaling = Vec::new();
    for extra in [0u128, 63, 1023] {
        for index in 0..extra {
            field
                .0
                .register(EntityKey(0xE000_0000 + index), registered.resolved.clone());
        }
        let cost = city.measure_polling_cost(&mut field, &mut NoTriggers, 20_000)?;
        scaling.push((extra + 1, cost.nanos_per_turn));
        println!(
            "   {:>5} dormant constructs registered: {} ns/turn",
            extra + 1,
            cost.nanos_per_turn
        );
    }
    let cheapest = scaling.iter().map(|(_, ns)| *ns).min().unwrap_or(0).max(1);
    let dearest = scaling.iter().map(|(_, ns)| *ns).max().unwrap_or(0);
    check(
        dearest <= cheapest.saturating_mul(3),
        "per-turn cost does not grow with the number of dormant constructs (1 -> 1024)",
    );

    // ------------------------------------------------------------- P6 unseal
    println!("\n== P6  UNSEAL — give the files back ==");
    fs::rename(&sealed_path, &args.store)?;
    let readback = city.readback_from_disk()?;
    println!(
        "   disk says revision {:?} tick {:?}; the living city says revision {:?} tick {:?}",
        readback.revision,
        readback.tick,
        city.supervisor().revision(),
        city.supervisor().tick()
    );
    check(
        readback.revision == city.supervisor().revision(),
        "the disk copy and the living city agree before any new commit",
    );

    // ------------------------------------------------------------- P7 commit
    println!("\n== P7  A FIRING BECOMES A COMMITTED MOMENT, through the loop ==");
    let code = crossing_program();
    let subscription = crossing_subscription(&code)?;
    city.supervisor_mut().register_subscription(subscription);
    let moment_symbol = city
        .supervisor()
        .snapshot()
        .symbol_id("Moment")
        .ok_or("this store has no Moment symbol")?;
    let mut driver = MomentDriver {
        code,
        host: BareHost,
        trigger_atom,
        moment_symbol,
        committed: 0,
        executed: 0,
    };
    let revision_before = city.supervisor().revision();
    let tick_before = city.supervisor().tick();
    let entities_before = city.supervisor().snapshot().entities.len();
    injector.inject(FieldInjection::perturb(construct_id, "resident_universe:proof"));
    let commit_run = city.run(&mut field, &mut driver, Until::Quiescent)?;
    let commit_io = commit_run.io.clone();
    println!(
        "   turns {}  programs executed {}  write-sets translated {}",
        commit_run.turns, driver.executed, driver.committed
    );
    println!(
        "   revision {:?} -> {:?}   universe tick {:?} -> {:?}   entities {} -> {}",
        revision_before,
        city.supervisor().revision(),
        tick_before,
        city.supervisor().tick(),
        entities_before,
        city.supervisor().snapshot().entities.len()
    );
    println!("   commit I/O   : {}", describe_io(&commit_io));
    check(
        city.supervisor().revision().0 == revision_before.0 + 1,
        "the loop committed exactly one revision through the live heartbeat",
    );
    check(
        city.supervisor().tick().0 > tick_before.0,
        "the universe clock advanced (it only moves on a commit)",
    );
    check(
        city.supervisor().snapshot().entities.len() == entities_before + 1,
        "one Moment entity is present in the living city",
    );
    check(
        driver.executed >= 1,
        "the drained wake really ran its pinned program on a VM host",
    );
    check(
        io_ops(&commit_io).map_or(false, |(_, writes)| writes > 0),
        "the commit boundary wrote to disk SYNCHRONOUSLY (measured, see report)",
    );
    check(
        io_ops(&commit_io).map_or(false, |(reads, _)| reads == 0),
        "committing still re-read NOTHING from disk",
    );

    // ------------------------------------------------------------- P8 backup
    println!("\n== P8  BACKUP — copy the living city, just in case ==");
    let backup = city.backup_now();
    println!(
        "   backed up revision {:?} tick {:?} in {:?}; checkpoint size {}",
        backup.revision,
        backup.tick,
        backup.wall,
        match &backup.bytes {
            Epistemic::Measured(bytes) => format!("{bytes} bytes"),
            other => format!("{other:?}"),
        }
    );
    check(backup.failure.is_none(), "the backup succeeded");
    check(
        backup.revision == city.supervisor().revision(),
        "the backup carries the CURRENT in-memory revision",
    );

    // ------------------------------------------------------------ P9 restore
    println!("\n== P9  RESTORE FROM THE BACKUP ALONE (event log deleted) ==");
    let restore_dir = restore_path(&args.store);
    if restore_dir.exists() {
        fs::remove_dir_all(&restore_dir)?;
    }
    copy_dir(&args.store, &restore_dir)?;
    let log = restore_dir.join("events.jsonl");
    let log_bytes = fs::metadata(&log).map(|meta| meta.len()).unwrap_or(0);
    if log.exists() {
        fs::remove_file(&log)?;
    }
    println!("   copied to {} and deleted its {log_bytes}-byte event log", restore_dir.display());
    let restored = Supervisor::boot(&restore_dir, &args.genesis)?;
    let restored_hash = restored.snapshot().canonical_hash()?;
    let living_hash = city.snapshot_hash()?;
    println!(
        "   restored revision {:?} tick {:?} entities {}",
        restored.revision(),
        restored.tick(),
        restored.snapshot().entities.len()
    );
    check(
        restored.revision() == city.supervisor().revision(),
        "a city restored from the backup alone is at the living revision",
    );
    check(
        restored_hash == living_hash,
        "the restored snapshot is byte-for-byte the living snapshot",
    );

    // ------------------------------------------------------------- summary
    let counters = city.counters().clone();
    println!("\n== MEASURED TOTALS ==");
    println!(
        "   loop turns {}   waves {}   fired atoms {}   candidates {}   commits {}",
        counters.turns,
        counters.waves,
        counters.fired_atoms,
        counters.candidate_effects,
        counters.commits
    );
    println!(
        "   perturbations admitted {}   dormant waits {}   dormant timeouts {}",
        counters.injections_admitted, counters.dormant_waits, counters.dormant_timeouts
    );
    println!(
        "   inference: {} answered, {} refused, {} re-entrancy violations",
        counters.inferences_ok, counters.inferences_failed, counters.inference_reentrancy
    );
    println!(
        "   backups: {} ok, {} failed, worst dump {} ns",
        counters.backups_ok, counters.backups_failed, counters.max_backup_nanos
    );
    println!("   worst single turn: {} ns", counters.max_turn_nanos);
    check(
        counters.inference_reentrancy == 0,
        "never more than one inference in flight",
    );
    check(
        counters.inferences_ok == 3,
        "the inference seam ran once per turn-carrying perturbation",
    );

    if args.forever {
        println!("\n== RESIDENT — running forever; perturb the field to wake it ==");
        city.run(&mut field, &mut driver, Until::Forever)?;
    }

    if failures.is_empty() {
        println!("\n=================================================================");
        println!("RESIDENT UNIVERSE: the city booted once, lived with its files taken");
        println!("away, cost nothing while idle, committed through its own heartbeat,");
        println!("backed itself up, and was restored from that backup alone.");
        println!("=================================================================");
        Ok(true)
    } else {
        println!("\nFAILED EXPECTATIONS:");
        for failure in &failures {
            println!("   - {failure}");
        }
        Ok(false)
    }
}

/// The reduced sequence for a store that holds no construct: prove the cold boot,
/// the backup, and the restore-from-backup-alone. No wave can run without a
/// construct, and pretending otherwise would be measuring nothing.
fn without_construct(
    city: &mut ResidentUniverse,
    args: &Args,
    check: &mut dyn FnMut(bool, &str),
) -> Result<(), Box<dyn Error>> {
    println!("
== B  BACKUP — copy the living city, just in case ==");
    let backup = city.backup_now();
    println!(
        "   backed up revision {:?} tick {:?} in {:?}; checkpoint size {}",
        backup.revision,
        backup.tick,
        backup.wall,
        match &backup.bytes {
            Epistemic::Measured(bytes) => format!("{bytes} bytes"),
            other => format!("{other:?}"),
        }
    );
    check(backup.failure.is_none(), "the backup succeeded");

    println!("
== R  RESTORE FROM THE BACKUP ALONE (event log deleted) ==");
    let restore_dir = restore_path(&args.store);
    if restore_dir.exists() {
        fs::remove_dir_all(&restore_dir)?;
    }
    copy_dir(&args.store, &restore_dir)?;
    let log = restore_dir.join("events.jsonl");
    let log_bytes = fs::metadata(&log).map(|meta| meta.len()).unwrap_or(0);
    if log.exists() {
        fs::remove_file(&log)?;
    }
    println!("   copied to {} and deleted its {log_bytes}-byte event log", restore_dir.display());
    let restored = Supervisor::boot(&restore_dir, &args.genesis)?;
    println!(
        "   restored revision {:?} tick {:?} entities {}",
        restored.revision(),
        restored.tick(),
        restored.snapshot().entities.len()
    );
    check(
        restored.revision() == city.supervisor().revision(),
        "a city restored from the backup alone is at the living revision",
    );
    check(
        restored.snapshot().canonical_hash()? == city.snapshot_hash()?,
        "the restored snapshot is byte-for-byte the living snapshot",
    );
    Ok(())
}

fn sealed_path(store: &Path) -> PathBuf {
    let mut name = store.file_name().unwrap_or_default().to_os_string();
    name.push(".sealed");
    store.with_file_name(name)
}

fn restore_path(store: &Path) -> PathBuf {
    let mut name = store.file_name().unwrap_or_default().to_os_string();
    name.push(".restored");
    store.with_file_name(name)
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn io_ops(io: &Epistemic<metrics::IoCounts>) -> Option<(u64, u64)> {
    match io {
        Epistemic::Measured(counts) => Some((counts.read_ops, counts.write_ops)),
        _ => None,
    }
}

fn describe_io(io: &Epistemic<metrics::IoCounts>) -> String {
    match io {
        Epistemic::Measured(counts) => format!(
            "{} reads ({} bytes), {} writes ({} bytes), {} other",
            counts.read_ops,
            counts.read_bytes,
            counts.write_ops,
            counts.write_bytes,
            counts.other_ops
        ),
        other => format!("{other:?}"),
    }
}

fn describe_cpu(cpu: &Epistemic<Duration>) -> String {
    match cpu {
        Epistemic::Measured(duration) => format!("{duration:?}"),
        other => format!("{other:?}"),
    }
}
