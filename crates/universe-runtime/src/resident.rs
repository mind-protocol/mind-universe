//! The resident Universe: boot once, hold the state, live — and take a backup
//! now and then, just in case.
//!
//! # What is different here
//!
//! Every existing entry point into this world is a *visit*: open the store,
//! replay its whole history from zero, do one thing, die. That makes the files
//! the authority and the process a tourist — and it is why two visitors can both
//! commit over the same base revision and freeze the city.
//!
//! A resident Universe inverts it. The files are read exactly once, at cold
//! start, to reconstitute the world. From that moment the AUTHORITY IS THE
//! RUNNING PROCESS: the in-memory snapshot the supervisor holds. Persistence is
//! then a backup, not a chain: every so often the loop writes the whole current
//! snapshot through the store's existing [`UniverseStore::checkpoint`]. A backup
//! is a copy — it cannot fork, it cannot be poisoned by one bad line, and
//! restoring from it means starting from that point and accepting the loss of
//! whatever came after. Everyone already knows what a backup is.
//!
//! # The loop
//!
//! ```text
//! admit perturbations (non-blocking)
//! → is there work?  ── no ──→ BLOCK on the wake queue        (dormant: no cost)
//!        │ yes
//!        ▼
//! one turn = Supervisor::advance_draining_triggers
//!            (drain the wake queue → commit boundary → physics wave → ingest firings)
//!        ▼
//! is a backup due?  ── yes ──→ checkpoint the in-memory snapshot
//! ```
//!
//! "Work" is exactly three measured facts: a queued wake in the field, a wake
//! request on the supervisor's own scheduler, or a prepared transaction awaiting
//! the boundary. When all three are zero the city has nothing to do and the loop
//! parks. **It never polls.** There is no scan over constructs looking for one
//! that might be ready; a construct that is not woken is never visited at all.
//!
//! # What this module deliberately does NOT own
//!
//! Which construct wakes (the [`Field`]), which program runs (the driver), which
//! model thinks (the [`InferencePort`]), how often a backup is taken (the
//! caller's [`BackupPolicy`]). The loop is a clock and a queue.

use std::time::{Duration, Instant};

use universe_core::{Epistemic, Revision, Tick, UniverseError};
use universe_store::{UniverseSnapshot, UniverseStore};
use universe_supervisor::{PhaseHook, Supervisor, TickPhase, TriggerTickDriver};

use crate::field::{DynDriver, Field};
use crate::metrics::{self, IoCounts};
use crate::seams::{
    InferenceCall, InferencePort, InferenceTrace, UnavailableInference,
};
use crate::wake::{FieldInjection, Injector, WaitOutcome, WakeInbox};

/// A phase hook that does nothing. The resident loop asserts on measured
/// outcomes, not on phase callbacks; a caller that wants observation hooks
/// supplies its own in a later slice.
struct NoopHook;

impl PhaseHook for NoopHook {
    fn run(&mut self, _phase: TickPhase, _snapshot: &UniverseSnapshot) -> Result<(), UniverseError> {
        Ok(())
    }
}

/// How long a `run` should stay resident.
#[derive(Clone, Copy, Debug)]
pub enum Until {
    /// A real resident city: never returns on its own.
    Forever,
    /// Do all the work that is currently available, then return WITHOUT blocking.
    /// The bounded shape used to measure one wake cycle.
    Quiescent,
    /// Live, but return once the city has been dormant this long without a single
    /// perturbation.
    Idle(Duration),
}

/// When to take a backup. Both dimensions are caller-supplied: the runtime holds
/// no opinion about how much loss is acceptable, because that is not a property
/// of the trusted computing base.
#[derive(Clone, Copy, Debug, Default)]
pub struct BackupPolicy {
    /// Back up after this many turns of the loop.
    pub every_turns: Option<u64>,
    /// Back up after this much wall time.
    pub every: Option<Duration>,
}

impl BackupPolicy {
    /// No automatic backup. The city still lives; it just is not copied.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn every_turns(turns: u64) -> Self {
        Self {
            every_turns: Some(turns),
            every: None,
        }
    }

    pub fn every(interval: Duration) -> Self {
        Self {
            every_turns: None,
            every: Some(interval),
        }
    }

    fn is_armed(&self) -> bool {
        self.every_turns.is_some() || self.every.is_some()
    }
}

/// One backup, measured.
#[derive(Clone, Debug)]
pub struct BackupOutcome {
    /// The revision that was copied.
    pub revision: Revision,
    pub tick: Tick,
    pub wall: Duration,
    /// Size of the written checkpoint, when it could be stat'ed.
    pub bytes: Epistemic<u64>,
    /// `None` on success; the failure reason otherwise. A failed backup is
    /// counted and reported — it never stops the city.
    pub failure: Option<String>,
}

/// Everything the cold start measured. Kept for the lifetime of the process so a
/// later claim ("the files were never read again") has a baseline to stand on.
#[derive(Clone, Debug)]
pub struct BootEvidence {
    pub store_root: String,
    pub revision: Revision,
    pub tick: Tick,
    pub entities: usize,
    pub relations: usize,
    pub symbols: usize,
    /// Canonical hash of the snapshot the process booted with. The in-memory
    /// authority's fingerprint.
    pub snapshot_hash: String,
    pub wall: Duration,
    /// Process I/O counters the instant boot finished. Every later reading is
    /// compared against this.
    pub io_at_boot: Epistemic<IoCounts>,
    pub cpu_at_boot: Epistemic<Duration>,
}

/// Running totals the loop owns. Each one counts something that actually
/// happened; none of them is a derived score.
#[derive(Clone, Debug, Default)]
pub struct Counters {
    /// Tick-advance calls made by the loop. This is the loop's own clock; it is
    /// NOT the universe clock, which only moves when something commits.
    pub turns: u64,
    /// Turns in which a physics wave actually ran.
    pub waves: u64,
    /// Construct atoms that crossed their threshold, summed over all waves.
    pub fired_atoms: u64,
    /// Candidate effects surfaced. Proposals — never committed by the loop.
    pub candidate_effects: u64,
    /// Commit receipts returned at a tick boundary.
    pub commits: u64,
    /// Perturbations taken off the wake queue and landed in the field.
    pub injections_admitted: u64,
    /// Times the loop actually parked because there was nothing to do.
    pub dormant_waits: u64,
    /// Dormant waits that expired without any perturbation arriving.
    pub dormant_timeouts: u64,
    pub inferences_ok: u64,
    pub inferences_failed: u64,
    /// Times a second inference was found in flight. The loop is serial, so this
    /// must stay 0; it is counted rather than assumed.
    pub inference_reentrancy: u64,
    pub backups_ok: u64,
    pub backups_failed: u64,
    pub total_backup_nanos: u64,
    pub max_backup_nanos: u64,
    /// The worst single turn.
    pub max_turn_nanos: u64,
    /// Every turn, summed.
    pub total_turn_nanos: u64,
}

/// What one `run` measured.
#[derive(Clone, Debug)]
pub struct RunReport {
    pub turns: u64,
    pub wall: Duration,
    pub cpu: Epistemic<Duration>,
    pub io: Epistemic<IoCounts>,
    pub dormant_waits: u64,
    pub dormant_timeouts: u64,
    pub backups: u64,
    /// Why the run returned.
    pub stopped_because: StopReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopReason {
    /// No work left and the caller asked for [`Until::Quiescent`].
    Quiescent,
    /// Dormant for the whole idle budget.
    IdleElapsed,
    /// Every [`Injector`] was dropped: nothing can ever perturb this city again.
    NoInjectorsLeft,
}

/// The cost of polling, measured rather than argued.
#[derive(Clone, Debug)]
pub struct PollingCost {
    pub turns: u64,
    pub wall: Duration,
    pub cpu: Epistemic<Duration>,
    pub io: Epistemic<IoCounts>,
    pub nanos_per_turn: u64,
}

/// Caller-supplied wiring. Cognition defaults to something that refuses rather
/// than pretends, and backups default to off — a city that is not being copied
/// should say so, not assume it is safe.
pub struct RuntimeConfig {
    pub inference: Box<dyn InferencePort>,
    /// Carried verbatim to the inference port. The runtime does not enforce it —
    /// the provider does, and the loop waits for the provider.
    pub inference_deadline: Duration,
    pub backup: BackupPolicy,
    /// How long a dormant wait parks before it re-checks its stop condition.
    /// A bound on the clock, not a policy: it changes only how promptly a
    /// `Forever` run notices a dropped injector, never what the city does.
    pub dormant_slice: Duration,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            inference: Box::new(UnavailableInference::default()),
            inference_deadline: Duration::from_secs(30),
            backup: BackupPolicy::none(),
            dormant_slice: Duration::from_millis(250),
        }
    }
}

/// A Universe that lives in this process.
pub struct ResidentUniverse {
    supervisor: Supervisor,
    /// A second handle on the same store root, used ONLY to write backups and to
    /// read back independently. The loop never reads through it.
    store: UniverseStore,
    inbox: WakeInbox,
    injector: Injector,
    config: RuntimeConfig,
    boot: BootEvidence,
    counters: Counters,
    inference_in_flight: bool,
    /// The inference this process ran most recently.
    last_inference: Option<InferenceTrace>,
    last_backup_turn: u64,
    last_backup_at: Instant,
    last_backup: Option<BackupOutcome>,
}

impl ResidentUniverse {
    /// Cold start: the ONE moment this process reads the store.
    ///
    /// Everything after this — every wave, every drain, every commit boundary —
    /// runs against the snapshot held in memory.
    pub fn boot(
        store_root: impl AsRef<std::path::Path>,
        genesis: impl AsRef<std::path::Path>,
        config: RuntimeConfig,
    ) -> Result<Self, UniverseError> {
        let root = store_root.as_ref().to_path_buf();
        let started = Instant::now();
        let supervisor = Supervisor::boot(&root, genesis)?;
        let wall = started.elapsed();
        let store = UniverseStore::open(&root)?;
        let snapshot = supervisor.snapshot();
        let boot = BootEvidence {
            store_root: root.to_string_lossy().into_owned(),
            revision: snapshot.revision,
            tick: snapshot.tick,
            entities: snapshot.entities.len(),
            relations: snapshot.relations.len(),
            symbols: snapshot.symbols.len(),
            snapshot_hash: snapshot.canonical_hash()?,
            wall,
            io_at_boot: metrics::process_io(),
            cpu_at_boot: metrics::process_cpu_time(),
        };
        let (injector, inbox) = crate::wake::wake_channel();
        Ok(Self {
            supervisor,
            store,
            inbox,
            injector,
            config,
            boot,
            counters: Counters::default(),
            inference_in_flight: false,
            last_inference: None,
            last_backup_turn: 0,
            last_backup_at: Instant::now(),
            last_backup: None,
        })
    }

    /// A handle for perturbing this city from outside. Clone it freely: an
    /// observer port, a transport, another thread. It can add energy and nothing
    /// else.
    pub fn injector(&self) -> Injector {
        self.injector.clone()
    }

    pub fn boot_evidence(&self) -> &BootEvidence {
        &self.boot
    }

    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    pub fn supervisor(&self) -> &Supervisor {
        &self.supervisor
    }

    /// Mutable access for callers that must register graph-authored subscriptions
    /// before the loop starts. The loop itself never reaches for this.
    pub fn supervisor_mut(&mut self) -> &mut Supervisor {
        &mut self.supervisor
    }

    pub fn last_inference(&self) -> Option<&InferenceTrace> {
        self.last_inference.as_ref()
    }

    pub fn last_backup(&self) -> Option<&BackupOutcome> {
        self.last_backup.as_ref()
    }

    /// The fingerprint of the CURRENT in-memory authority. Pure computation over
    /// resident state: it touches no file, which is exactly why it can be
    /// compared against [`BootEvidence::snapshot_hash`] while the store is
    /// unreachable.
    pub fn snapshot_hash(&self) -> Result<String, UniverseError> {
        self.supervisor.snapshot().canonical_hash()
    }

    /// Is there anything to do right now? Three measured facts, no scan.
    pub fn has_work(&self, field: &dyn Field) -> bool {
        field.pending_wakes() > 0
            || self.supervisor.trigger_backlog() > 0
            || self.supervisor.pending_commit_backlog() > 0
    }

    /// Live.
    pub fn run(
        &mut self,
        field: &mut dyn Field,
        driver: &mut dyn TriggerTickDriver,
        until: Until,
    ) -> Result<RunReport, UniverseError> {
        let started = Instant::now();
        let turns_at_entry = self.counters.turns;
        let backups_at_entry = self.counters.backups_ok + self.counters.backups_failed;
        let cpu_at_entry = metrics::process_cpu_time();
        let io_at_entry = metrics::process_io();
        let mut dormant_since: Option<Instant> = None;

        let stopped_because = loop {
            // Admit whatever the outside left for us. Non-blocking, so a busy
            // city never stalls on the injector port.
            while let Some(injection) = self.inbox.try_recv() {
                self.admit(injection, field)?;
                dormant_since = None;
            }

            if !self.has_work(field) {
                match until {
                    Until::Quiescent => break StopReason::Quiescent,
                    Until::Idle(budget) => {
                        let since = *dormant_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= budget {
                            break StopReason::IdleElapsed;
                        }
                    }
                    Until::Forever => {}
                }
                // DORMANT. The thread parks on the queue. No tick is advanced, no
                // construct is visited, no CPU is spent.
                self.counters.dormant_waits += 1;
                match self.inbox.recv_timeout(self.config.dormant_slice) {
                    WaitOutcome::Woken(injection) => {
                        self.admit(injection, field)?;
                        dormant_since = None;
                    }
                    WaitOutcome::TimedOut => {
                        self.counters.dormant_timeouts += 1;
                    }
                    WaitOutcome::NoInjectorsLeft => break StopReason::NoInjectorsLeft,
                }
                continue;
            }

            dormant_since = None;
            self.turn(field, driver)?;
            self.backup_if_due();
        };

        Ok(RunReport {
            turns: self.counters.turns - turns_at_entry,
            wall: started.elapsed(),
            cpu: metrics::cpu_delta(&cpu_at_entry, &metrics::process_cpu_time()),
            io: metrics::io_delta(&io_at_entry, &metrics::process_io()),
            dormant_waits: self.counters.dormant_waits,
            dormant_timeouts: self.counters.dormant_timeouts,
            backups: (self.counters.backups_ok + self.counters.backups_failed) - backups_at_entry,
            stopped_because,
        })
    }

    /// One turn of the serial loop, callable on its own so a caller can drive a
    /// bounded, inspectable sequence.
    pub fn turn(
        &mut self,
        field: &mut dyn Field,
        driver: &mut dyn TriggerTickDriver,
    ) -> Result<(), UniverseError> {
        let started = Instant::now();
        let mut hook = NoopHook;
        let mut carrier = DynDriver(driver);
        let outcome =
            self.supervisor
                .advance_draining_triggers(&mut hook, field.as_selector(), &mut carrier)?;
        let turn_nanos = started.elapsed().as_nanos() as u64;

        self.counters.turns += 1;
        self.counters.total_turn_nanos = self.counters.total_turn_nanos.saturating_add(turn_nanos);
        self.counters.max_turn_nanos = self.counters.max_turn_nanos.max(turn_nanos);
        self.counters.commits += outcome.commits.len() as u64;
        if let Some(wave) = outcome.physics_wave.as_ref() {
            self.counters.waves += 1;
            self.counters.fired_atoms += wave.fired_construct_atoms.len() as u64;
            self.counters.candidate_effects += wave.candidate_effects.len() as u64;
        }
        Ok(())
    }

    /// Copy the living city to disk. A backup, not a commit: it writes the WHOLE
    /// current snapshot through the store's existing checkpoint path, which is
    /// atomic (temp file + rename) and depends on nothing that came before it.
    ///
    /// A failure here is counted and returned, never propagated into the loop: a
    /// city that cannot reach its disk keeps living, it just stops being copied.
    pub fn backup_now(&mut self) -> BackupOutcome {
        let snapshot = self.supervisor.snapshot();
        let revision = snapshot.revision;
        let tick = snapshot.tick;
        let started = Instant::now();
        let result = self.store.checkpoint(snapshot);
        let wall = started.elapsed();
        let failure = result.err().map(|error| format!("{error:?}"));
        let bytes = if failure.is_none() {
            match std::fs::metadata(std::path::Path::new(&self.boot.store_root).join("snapshot.json"))
            {
                Ok(meta) => Epistemic::Measured(meta.len()),
                Err(error) => Epistemic::MeasurementFailed {
                    reason: error.to_string(),
                },
            }
        } else {
            Epistemic::KnownAbsent
        };

        let nanos = wall.as_nanos() as u64;
        self.counters.total_backup_nanos = self.counters.total_backup_nanos.saturating_add(nanos);
        self.counters.max_backup_nanos = self.counters.max_backup_nanos.max(nanos);
        if failure.is_none() {
            self.counters.backups_ok += 1;
        } else {
            self.counters.backups_failed += 1;
        }
        self.last_backup_turn = self.counters.turns;
        self.last_backup_at = Instant::now();

        let outcome = BackupOutcome {
            revision,
            tick,
            wall,
            bytes,
            failure,
        };
        self.last_backup = Some(outcome.clone());
        outcome
    }

    fn backup_if_due(&mut self) {
        if !self.config.backup.is_armed() {
            return;
        }
        let by_turns = self
            .config
            .backup
            .every_turns
            .is_some_and(|every| self.counters.turns.saturating_sub(self.last_backup_turn) >= every);
        let by_time = self
            .config
            .backup
            .every
            .is_some_and(|interval| self.last_backup_at.elapsed() >= interval);
        if by_turns || by_time {
            let _ = self.backup_now();
        }
    }

    /// Land one perturbation, running its turn's inference first if it carries
    /// one.
    fn admit(
        &mut self,
        injection: FieldInjection,
        field: &mut dyn Field,
    ) -> Result<(), UniverseError> {
        self.counters.injections_admitted += 1;
        if let Some(prompt) = injection.turn.clone() {
            self.run_inference(&injection, prompt);
        }
        field.deposit(&injection)
    }

    /// --- THE INFERENCE SEAM --------------------------------------------------
    /// One local inference, serial, one in flight. The loop is single-threaded,
    /// so a second concurrent call is structurally impossible; the guard counts
    /// rather than assumes it.
    fn run_inference(&mut self, injection: &FieldInjection, prompt: String) {
        if self.inference_in_flight {
            self.counters.inference_reentrancy += 1;
            return;
        }
        self.inference_in_flight = true;
        let call = InferenceCall {
            actor: injection.target,
            prompt,
            deadline: self.config.inference_deadline,
        };
        let started = Instant::now();
        let result = self.config.inference.infer(&call);
        let latency_nanos = started.elapsed().as_nanos() as u64;
        self.inference_in_flight = false;

        let trace = match result {
            Ok(reply) => {
                self.counters.inferences_ok += 1;
                InferenceTrace {
                    actor: call.actor,
                    prompt_bytes: call.prompt.len(),
                    reply: Some(reply.text),
                    error: None,
                    latency_nanos,
                }
            }
            Err(error) => {
                self.counters.inferences_failed += 1;
                InferenceTrace {
                    actor: call.actor,
                    prompt_bytes: call.prompt.len(),
                    reply: None,
                    error: Some(error.to_string()),
                    latency_nanos,
                }
            }
        };
        self.last_inference = Some(trace);
    }

    /// Measure what polling WOULD cost: force `turns` tick advances against a
    /// field with nothing woken.
    ///
    /// The resident loop never does this — that is the point. It exists so the
    /// claim "a dormant construct costs nothing per tick" can be a measured
    /// contrast instead of an assertion.
    pub fn measure_polling_cost(
        &mut self,
        field: &mut dyn Field,
        driver: &mut dyn TriggerTickDriver,
        turns: u64,
    ) -> Result<PollingCost, UniverseError> {
        let cpu_before = metrics::process_cpu_time();
        let io_before = metrics::process_io();
        let started = Instant::now();
        for _ in 0..turns {
            let mut hook = NoopHook;
            let mut carrier = DynDriver(&mut *driver);
            self.supervisor
                .advance_draining_triggers(&mut hook, field.as_selector(), &mut carrier)?;
        }
        let wall = started.elapsed();
        // Deliberately NOT added to `counters.turns`: these are measurement
        // turns, not the city living. Conflating them would inflate the loop's
        // own history with work nobody asked for.
        Ok(PollingCost {
            turns,
            wall,
            cpu: metrics::cpu_delta(&cpu_before, &metrics::process_cpu_time()),
            io: metrics::io_delta(&io_before, &metrics::process_io()),
            nanos_per_turn: if turns == 0 {
                0
            } else {
                (wall.as_nanos() / u128::from(turns)) as u64
            },
        })
    }

    /// An INDEPENDENT observation of the durable copy: reopen the store from disk
    /// and replay whatever log sits beside it.
    ///
    /// This is the only read path in this module, and it is not part of the loop —
    /// it is how an observer checks the backup against the living city. While the
    /// store is sealed away it fails, which is precisely what makes the loop's
    /// continued running evidence rather than a claim.
    pub fn readback_from_disk(&self) -> Result<UniverseSnapshot, UniverseError> {
        // `UniverseStore::open` calls `create_dir_all`, so opening a store root
        // that has been taken away would silently RECREATE it as an empty
        // directory — turning a decisive "the files are gone" into a misleading
        // "the files are empty", and letting a later backup succeed into a
        // directory nobody restored. Check first, and refuse without side effects.
        let root = std::path::Path::new(&self.boot.store_root);
        if !root.is_dir() {
            return Err(UniverseError::Io(format!(
                "store root {} does not exist",
                self.boot.store_root
            )));
        }
        let store = UniverseStore::open(root)?;
        store.replay(store.load_snapshot()?)
    }
}
