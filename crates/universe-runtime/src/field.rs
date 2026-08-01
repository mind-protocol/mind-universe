//! What the resident loop needs from a field, and nothing more.
//!
//! The supervisor already owns the generic seam that decides which construct
//! wakes on a tick ([`PhysicsWaveSelector`]). The resident loop needs two more
//! facts that the selector trait does not expose, because a continuously running
//! process must be able to answer "is there anything to do?" without doing it:
//!
//!   * deposit an outside perturbation onto a construct's trigger;
//!   * report how many wakes are still queued.
//!
//! That is the whole of [`Field`]. It carries no cluster policy, no thresholds
//! and no metaphor — those live in the graph-authored construct the selector
//! resolves.

use universe_core::UniverseError;
use universe_ir::{CodeDefinition, ExecutionRequest, Value};
use universe_store::UniverseSnapshot;
use universe_supervisor::{PhysicsWaveSelector, TriggerTickDriver};
use universe_vm::{ExecutionLimits, ExecutionReceipt};

use crate::wake::FieldInjection;

/// A physical field the resident loop can perturb and interrogate.
pub trait Field: PhysicsWaveSelector {
    /// Land an outside perturbation. Implementations resolve the injection's
    /// target to the construct's trigger; whether that crosses a threshold is the
    /// physics' answer, given later, in a wave.
    fn deposit(&mut self, injection: &FieldInjection) -> Result<(), UniverseError>;

    /// How many wakes are queued and not yet drained. `0` means every construct
    /// in this field is dormant — and a dormant construct is not visited, not
    /// scanned and not charged.
    fn pending_wakes(&self) -> usize;

    /// Hand the same object to the supervisor under the seam it already knows.
    /// Explicit rather than relying on trait upcasting, so the coercion is
    /// visible at the call site.
    fn as_selector(&mut self) -> &mut dyn PhysicsWaveSelector;
}

/// A `dyn`-compatible carrier for the supervisor's generic trigger driver.
///
/// [`universe_supervisor::Supervisor::advance_draining_triggers`] is generic over
/// its driver, so a trait object cannot be passed directly. This local newtype
/// forwards every call unchanged — it adds no behaviour, holds no policy, and
/// exists purely so the resident loop can be built once and handed any driver.
pub struct DynDriver<'a>(pub &'a mut dyn TriggerTickDriver);

impl TriggerTickDriver for DynDriver<'_> {
    fn resolve_code(&mut self, request: &ExecutionRequest) -> Result<CodeDefinition, UniverseError> {
        self.0.resolve_code(request)
    }

    fn execute(
        &mut self,
        code: &CodeDefinition,
        inputs: &std::collections::BTreeMap<String, Value>,
        limits: ExecutionLimits,
        revision: universe_core::Revision,
        tick: universe_core::Tick,
    ) -> Result<ExecutionReceipt, UniverseError> {
        self.0.execute(code, inputs, limits, revision, tick)
    }

    fn translate(
        &mut self,
        request: &ExecutionRequest,
        receipt: &ExecutionReceipt,
        snapshot: &UniverseSnapshot,
    ) -> Result<Option<universe_transactions::UniverseWriteSet>, UniverseError> {
        self.0.translate(request, receipt, snapshot)
    }
}
