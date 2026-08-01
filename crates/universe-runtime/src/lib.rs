//! The resident Universe — the process the city lives in.
//!
//! `mind-universe` has never had one. Every path into the world so far has been
//! a visit: `Supervisor::boot`, replay the whole event log from revision zero, do
//! one thing, exit. Under that shape the FILES are the authority and every
//! process is a tourist — which is why two of them can prepare over the same base
//! revision and freeze the city, and why the log's length is paid again on every
//! single call.
//!
//! This crate is the inversion. One process boots the store once, holds the
//! authoritative state in memory, and advances the serial loop for as long as it
//! is alive:
//!
//! ```text
//! cold boot (the only file read)
//! → hold the snapshot in memory — THIS is the truth
//! → block on the wake queue                              (dormant costs nothing)
//! → a perturbation arrives → one turn: drain, commit boundary, physics wave
//! → back up the whole snapshot every so often            (a copy, not a chain)
//! ```
//!
//! # The floor and only the floor
//!
//! Per `CLAUDE.md`'s bootstrap exception this crate is the runtime shell, the
//! clock and the queue — nothing else. It holds no threshold, no metaphor, no
//! cluster selection, no cognitive policy. Which construct wakes is the
//! [`field::Field`]'s answer; which program runs is the driver's; which model
//! thinks is the [`seams::InferencePort`]'s; how often the city is copied is the
//! caller's [`resident::BackupPolicy`].
//!
//! # Modules
//!
//! * [`resident`] — the loop, the boot, the backup, the counters.
//! * [`wake`] — perturbing the field from outside, and the queue the loop parks on.
//! * [`field`] — the two facts the loop needs from a field beyond the supervisor's
//!   existing selector seam.
//! * [`seams`] — the inference seam.
//! * [`metrics`] — OS-measured CPU and I/O counters, so the claims in this crate
//!   are measurements rather than assertions.

pub mod field;
pub mod metrics;
pub mod resident;
pub mod seams;
pub mod wake;

pub use field::{DynDriver, Field};
pub use resident::{
    BackupOutcome, BackupPolicy, BootEvidence, Counters, PollingCost, ResidentUniverse, RunReport,
    RuntimeConfig, StopReason, Until,
};
pub use seams::{
    InferenceCall, InferenceError, InferencePort, InferenceReply, InferenceTrace,
    UnavailableInference,
};
pub use wake::{wake_channel, Delivery, FieldInjection, Injector, WaitOutcome, WakeInbox};
