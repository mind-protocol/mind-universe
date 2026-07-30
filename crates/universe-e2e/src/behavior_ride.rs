//! The arc's first rung: the board rides a REAL executed BehaviorBond, not
//! `AtomDynamics` over derived `PartBond`s.
//!
//! It reuses the full green pipeline ([`crate::behavior_runtime::run`]): a
//! graph-authored BehaviorBond over the canonical store is projected,
//! materialized, compiled, and EXECUTED via `execute_runtime_bond_artifact` —
//! the same primitive the supervisor's Physics phase runs — with content hashes
//! verified and graph-owned loop health closed. This module owns no physics; it
//! reads the executed `RuntimeBondExecutionReceipt` and reports the glide step
//! (did the bond's target fire?) with a `measured / executed` tag.
//!
//! This is ONE bond (the authored authority bond, tied to a canonical predicate
//! with an authored integer energy = measured provenance). Riding a whole
//! canonical neighborhood needs a BehaviorBond per relation — the remaining L
//! work. But unlike the derived ride, THIS energy is measured and executed:
//! `universe-protocol` would accept it as an energy transfer.

use serde::Serialize;
use std::path::Path;
use universe_core::{EntityKey, Epistemic};
use universe_ir::BehaviorLoopClosure;
use universe_physics::AtomConvergence;

use crate::behavior_runtime::{default_genesis_path, run, BehaviorRuntimeConfig};
use crate::E2eError;

#[derive(Clone, Debug, Serialize)]
pub struct MeasuredGlide {
    /// The whole point: this energy is measured and executed, not derived.
    pub epistemic_status: String,
    pub bond: EntityKey,
    pub source: EntityKey,
    pub target: EntityKey,
    /// Did the bond's target atom fire under the EXECUTED physics? (the glide step)
    pub target_fired: bool,
    pub transfer_energy: u64,
    pub converged: bool,
    pub energy_conserved: bool,
    pub loop_health_closed: bool,
    pub artifact_hash: String,
    pub execution_receipt_hash: String,
}

/// Run the graph-authored BehaviorBond through the real pipeline and read its
/// executed physics back as a single measured glide step.
pub fn ride_executed_bond(artifact_root: &Path) -> Result<MeasuredGlide, E2eError> {
    let manifest = run(&BehaviorRuntimeConfig {
        artifact_root: artifact_root.to_path_buf(),
        genesis_path: default_genesis_path(),
    })?;
    let plan = &manifest.execution.plan;
    let target = plan.target.atom;
    let target_fired = manifest
        .execution
        .physical
        .run
        .steps
        .iter()
        .any(|step| step.fired.contains(&target));
    let execution_receipt_hash = manifest
        .receipt_content_hashes
        .get("execution")
        .cloned()
        .unwrap_or_default();
    Ok(MeasuredGlide {
        epistemic_status: "measured / executed / hash-verified".into(),
        bond: plan.behavior_bond,
        source: plan.source.atom,
        target,
        target_fired,
        transfer_energy: plan.transfer_energy,
        converged: manifest.execution.physical.convergence == AtomConvergence::Quiescent,
        energy_conserved: manifest.execution.physical.energy.conserved,
        loop_health_closed: manifest.loop_health.health
            == Epistemic::Measured(BehaviorLoopClosure::Closed),
        artifact_hash: manifest.execution.artifact_hash.clone(),
        execution_receipt_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_content_hash(hash: &str) -> bool {
        hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    }

    #[test]
    fn board_rides_a_real_executed_behavior_bond() {
        let temp = tempfile::tempdir().unwrap();
        let glide = ride_executed_bond(temp.path()).unwrap();
        println!("{glide:#?}");

        // The glide step is confirmed by EXECUTED physics: the target fired.
        assert!(
            glide.target_fired,
            "the bond's target must fire under executed physics"
        );
        assert!(glide.converged && glide.energy_conserved);

        // Measured, hash-verified, and the graph-owned loop health closed.
        assert!(glide.loop_health_closed);
        assert!(is_content_hash(&glide.artifact_hash));
        assert!(is_content_hash(&glide.execution_receipt_hash));
        assert_eq!(glide.epistemic_status, "measured / executed / hash-verified");
        assert!(glide.transfer_energy > 0);
    }
}
