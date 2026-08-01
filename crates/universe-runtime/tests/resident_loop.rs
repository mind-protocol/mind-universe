//! The resident loop's own proofs, independent of any construct fixture.
//!
//! These run over a throwaway tempdir store booted from minimal genesis, with a
//! synthetic field. They prove the four properties the loop is responsible for:
//!
//!   1. a dormant city advances no turns and burns no measurable CPU — it PARKS
//!      on its queue rather than polling;
//!   2. a perturbation wakes it, exactly one turn runs, and the queue drains;
//!   3. the files can be taken away and the city keeps living, with the
//!      in-memory authority unchanged (and a disk readback correctly failing);
//!   4. a backup is a COPY: restoring from it with the event log deleted yields
//!      byte-for-byte the same snapshot.

use std::path::{Path, PathBuf};
use std::time::Duration;

use universe_core::{EntityKey, Epistemic, RelationKey, UniverseError};
use universe_physics::{
    AtomBond, AtomExecutionBudget, AtomSpec, BondPolarity, LocalAtomCluster, PhysicsEventDeposit,
};
use universe_runtime::{
    BackupPolicy, Field, FieldInjection, ResidentUniverse, RuntimeConfig, StopReason, Until,
};
use universe_store::UniverseSnapshot;
use universe_supervisor::{NoTriggers, PhysicsWaveInputs, PhysicsWaveSelector};

fn genesis() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/genesis/minimal-genesis.json")
}

/// A minimal field: one construct whose sensor is seeded above threshold, so a
/// wave that runs actually fires. It is dormant until woken, and a wake drains in
/// exactly one turn.
struct TestField {
    id: EntityKey,
    queue: usize,
    selected: u64,
}

impl TestField {
    fn new(id: EntityKey) -> Self {
        Self {
            id,
            queue: 0,
            selected: 0,
        }
    }

    fn inputs(&self) -> PhysicsWaveInputs {
        let sensor = EntityKey(0xA001);
        let trigger = EntityKey(0xA002);
        PhysicsWaveInputs {
            sensor_cluster: LocalAtomCluster {
                atoms: vec![AtomSpec {
                    key: sensor,
                    threshold: 100,
                    seed_energy: 100,
                    required_supports: Vec::new(),
                    inhibition_threshold: None,
                }],
                bonds: Vec::new(),
                injections: Vec::new(),
            },
            deposit_bindings: vec![PhysicsEventDeposit {
                trigger: sensor,
                target: trigger,
                weight: 100,
            }],
            construct_cluster: LocalAtomCluster {
                atoms: vec![
                    AtomSpec {
                        key: trigger,
                        threshold: 100,
                        seed_energy: 0,
                        required_supports: Vec::new(),
                        inhibition_threshold: None,
                    },
                    AtomSpec {
                        key: EntityKey(0xA003),
                        threshold: 100,
                        seed_energy: 0,
                        required_supports: Vec::new(),
                        inhibition_threshold: None,
                    },
                ],
                bonds: vec![AtomBond {
                    key: RelationKey(0xAA01),
                    source: trigger,
                    target: EntityKey(0xA003),
                    polarity: BondPolarity::Support,
                    energy: 100,
                }],
                injections: Vec::new(),
            },
            effect_bindings: Vec::new(),
            budget: AtomExecutionBudget {
                max_atoms: 8,
                max_bonds: 8,
                max_steps: 8,
                max_total_energy: 1_000,
            },
        }
    }
}

impl PhysicsWaveSelector for TestField {
    fn select(
        &mut self,
        _snapshot: &UniverseSnapshot,
    ) -> Result<Option<PhysicsWaveInputs>, UniverseError> {
        if self.queue == 0 {
            return Ok(None);
        }
        self.queue -= 1;
        self.selected += 1;
        Ok(Some(self.inputs()))
    }
}

impl Field for TestField {
    fn deposit(&mut self, injection: &FieldInjection) -> Result<(), UniverseError> {
        if injection.target != self.id {
            return Err(UniverseError::Validation("unknown construct".into()));
        }
        self.queue += 1;
        Ok(())
    }

    fn pending_wakes(&self) -> usize {
        self.queue
    }

    fn as_selector(&mut self) -> &mut dyn PhysicsWaveSelector {
        self
    }
}

fn boot(dir: &Path) -> ResidentUniverse {
    ResidentUniverse::boot(
        dir,
        genesis(),
        RuntimeConfig {
            backup: BackupPolicy::none(),
            dormant_slice: Duration::from_millis(25),
            ..RuntimeConfig::default()
        },
    )
    .expect("cold boot")
}

/// A dormant city parks. It advances zero turns across a real idle interval and
/// spends no measurable CPU doing it — the loop is waiting on its queue, not
/// scanning for work.
#[test]
fn a_dormant_city_advances_no_turns_and_costs_no_cpu() {
    let temp = tempfile::tempdir().unwrap();
    let mut city = boot(temp.path());
    let id = EntityKey(0xD001);
    let mut field = TestField::new(id);

    let report = city
        .run(&mut field, &mut NoTriggers, Until::Idle(Duration::from_millis(600)))
        .unwrap();

    assert_eq!(report.turns, 0, "a dormant city advances no turns");
    assert_eq!(report.stopped_because, StopReason::IdleElapsed);
    assert!(
        report.dormant_waits > 0,
        "the loop really parked on the queue"
    );
    assert!(
        report.wall >= Duration::from_millis(500),
        "the idle interval was actually spent idling: {:?}",
        report.wall
    );
    if let Epistemic::Measured(cpu) = report.cpu {
        assert!(
            cpu < Duration::from_millis(50),
            "a parked loop must not burn CPU, spent {cpu:?}"
        );
    }
    assert_eq!(field.selected, 0, "no wave ran while dormant");
}

/// One perturbation, one turn, one wave. The queue drains and the city returns to
/// dormancy on its own.
#[test]
fn one_perturbation_runs_exactly_one_turn() {
    let temp = tempfile::tempdir().unwrap();
    let mut city = boot(temp.path());
    let id = EntityKey(0xD001);
    let mut field = TestField::new(id);

    city.injector()
        .inject(FieldInjection::perturb(id, "test"));
    let report = city
        .run(&mut field, &mut NoTriggers, Until::Quiescent)
        .unwrap();

    assert_eq!(report.turns, 1, "exactly one turn per queued wake");
    assert_eq!(report.stopped_because, StopReason::Quiescent);
    assert_eq!(field.pending_wakes(), 0, "the queue drained");
    let counters = city.counters();
    assert_eq!(counters.waves, 1);
    assert!(counters.fired_atoms >= 1, "the woken construct fired");
    assert_eq!(counters.commits, 0, "a physics wave commits nothing");
}

/// Take the files away and the city keeps living. A disk readback fails; the
/// in-memory authority is unchanged; a backup attempted with no disk is reported
/// and is not fatal.
#[test]
fn the_city_lives_with_its_files_taken_away() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let mut city = boot(&store);
    let hash_at_boot = city.boot_evidence().snapshot_hash.clone();
    let id = EntityKey(0xD001);
    let mut field = TestField::new(id);

    let sealed = temp.path().join("store.sealed");
    std::fs::rename(&store, &sealed).unwrap();
    assert!(
        city.readback_from_disk().is_err(),
        "the files must really be unreachable"
    );

    for _ in 0..5 {
        city.injector()
            .inject(FieldInjection::perturb(id, "test"));
        city.run(&mut field, &mut NoTriggers, Until::Quiescent)
            .unwrap();
    }
    assert_eq!(city.counters().waves, 5, "five waves ran with no files");
    assert_eq!(
        city.snapshot_hash().unwrap(),
        hash_at_boot,
        "the in-memory authority is intact"
    );

    let failed = city.backup_now();
    assert!(
        failed.failure.is_some(),
        "a backup with no disk must report failure"
    );
    city.injector()
        .inject(FieldInjection::perturb(id, "test"));
    let after = city
        .run(&mut field, &mut NoTriggers, Until::Quiescent)
        .unwrap();
    assert_eq!(after.turns, 1, "a failed backup does not stop the city");

    std::fs::rename(&sealed, &store).unwrap();
    assert!(city.readback_from_disk().is_ok(), "the disk comes back");
}

/// A backup is a copy, not a chain: restore it with the event log deleted and the
/// snapshot is byte-for-byte identical.
#[test]
fn a_backup_restores_without_the_event_log() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let mut city = boot(&store);

    let backup = city.backup_now();
    assert!(backup.failure.is_none(), "{backup:?}");
    assert_eq!(backup.revision, city.supervisor().revision());

    let restore = temp.path().join("restored");
    std::fs::create_dir_all(&restore).unwrap();
    for entry in std::fs::read_dir(&store).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), restore.join(entry.file_name())).unwrap();
        }
    }
    let log = restore.join("events.jsonl");
    if log.exists() {
        std::fs::remove_file(&log).unwrap();
    }

    let restored = ResidentUniverse::boot(&restore, genesis(), RuntimeConfig::default()).unwrap();
    assert_eq!(
        restored.boot_evidence().snapshot_hash,
        city.snapshot_hash().unwrap(),
        "a restored city is the backed-up city"
    );
    assert_eq!(restored.supervisor().revision(), city.supervisor().revision());
}
