//! The snowboard: a magic object interpreted as a descent.
//!
//! The board is one instance of the [`crate::magic_object`] blueprint. This
//! module owns no physics and no boundary logic — it loads the decorated object
//! and reads its settled activation as a glide: the fired downhill `moment`
//! carrying the most support is the attractor (the next step); an `open_question`
//! (`narrative`) that never fires is the untaken fork, still Fog.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};
use universe_core::EntityKey;

use crate::magic_object::{Gesture, MagicObject};
use crate::E2eError;

/// One glide step read back from a settled activation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Glide {
    pub attractor: Option<EntityKey>,
    pub attractor_support: u64,
    pub fork_materialized: bool,
    pub fired: Vec<EntityKey>,
    pub energy_conserved: bool,
    pub quiescent: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BoardManifest {
    /// No gesture: energy flows down the steepest bond.
    pub glide: Glide,
    /// The same glide, re-run: proves the descent is deterministic.
    pub glide_repeat: Glide,
    /// Carve toward B: re-injection re-chooses the attractor.
    pub carve: Glide,
    /// Active contradiction: the slope toward A is impassable, descent stalls.
    pub contradiction: Glide,
    /// Contradiction plus a spoken answer: the untaken branch materializes.
    pub counterfactual: Glide,
    pub deterministic: bool,
    pub carve_redirects: bool,
    pub contradiction_stalls: bool,
    pub counterfactual_opens_fork: bool,
    pub all_energy_conserved: bool,
    pub status: String,
}

struct Descent {
    object: MagicObject,
    candidates: Vec<EntityKey>,
    fork: EntityKey,
}

impl Descent {
    fn load(fixture_path: &Path) -> Result<Self, E2eError> {
        let object = MagicObject::load(fixture_path)?;
        let candidates = object.function_nodes("candidate");
        if candidates.is_empty() {
            return Err(E2eError::Contract(
                "descent object declares no candidate moments".into(),
            ));
        }
        let fork = object
            .function_nodes("fork")
            .first()
            .copied()
            .ok_or_else(|| E2eError::Contract("descent object declares no fork".into()))?;
        Ok(Self {
            object,
            candidates,
            fork,
        })
    }

    fn glide(&self, gestures: &[Gesture<'_>]) -> Result<Glide, E2eError> {
        let activation = self.object.wield(gestures)?;
        let fired: BTreeSet<EntityKey> = activation.fired.iter().copied().collect();

        // The attractor is the fired downhill candidate carrying the most
        // support: the steepest place the deformation sends us. A tie resolves
        // to the lower key so the glide stays deterministic.
        let attractor = self
            .candidates
            .iter()
            .filter(|candidate| fired.contains(candidate))
            .filter_map(|candidate| {
                activation
                    .support
                    .get(candidate)
                    .map(|support| (*candidate, *support))
            })
            .max_by(|left, right| {
                left.1
                    .cmp(&right.1)
                    .then_with(|| right.0 .0.cmp(&left.0 .0))
            });
        Ok(Glide {
            attractor: attractor.map(|(key, _)| key),
            attractor_support: attractor.map(|(_, support)| support).unwrap_or(0),
            fork_materialized: fired.contains(&self.fork),
            fired: activation.fired,
            energy_conserved: activation.energy_conserved,
            quiescent: activation.quiescent,
        })
    }
}

/// Load the board magic object and read back every descent scenario as observed
/// activation facts. This proves nothing beyond this object.
pub fn prove(fixture_path: &Path) -> Result<BoardManifest, E2eError> {
    let descent = Descent::load(fixture_path)?;

    let glide = descent.glide(&[])?;
    let glide_repeat = descent.glide(&[])?;
    let carve = descent.glide(&[Gesture {
        binding: "carve_b",
        energy: 150,
    }])?;
    let contradiction = descent.glide(&[Gesture {
        binding: "contradiction",
        energy: 1,
    }])?;
    let counterfactual = descent.glide(&[
        Gesture {
            binding: "contradiction",
            energy: 1,
        },
        Gesture {
            binding: "answer_q",
            energy: 100,
        },
    ])?;

    let deterministic = glide == glide_repeat;
    let carve_redirects = glide.attractor.is_some()
        && carve.attractor.is_some()
        && glide.attractor != carve.attractor;
    let contradiction_stalls = contradiction.attractor.is_none();
    let counterfactual_opens_fork = !glide.fork_materialized && counterfactual.fork_materialized;
    let all_energy_conserved = [
        &glide,
        &glide_repeat,
        &carve,
        &contradiction,
        &counterfactual,
    ]
    .into_iter()
    .all(|glide| glide.energy_conserved && glide.quiescent);

    let passed = deterministic
        && carve_redirects
        && contradiction_stalls
        && counterfactual_opens_fork
        && all_energy_conserved;
    Ok(BoardManifest {
        glide,
        glide_repeat,
        carve,
        contradiction,
        counterfactual,
        deterministic,
        carve_redirects,
        contradiction_stalls,
        counterfactual_opens_fork,
        all_energy_conserved,
        status: if passed {
            "validated_for_fixture"
        } else {
            "not_validated"
        }
        .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/atoms/board-descent-v0.json")
    }

    #[test]
    fn board_deforms_topology_into_a_descent_over_atoms() {
        let manifest = prove(&fixture()).unwrap();

        // Default glide: the steepest bond wins, the fork stays Fog.
        assert_eq!(manifest.glide.attractor, Some(EntityKey(2)));
        assert!(!manifest.glide.fork_materialized);

        // The descent is deterministic given the same object and gestures.
        assert!(manifest.deterministic);

        // Carving re-chooses "down": B instead of A.
        assert_eq!(manifest.carve.attractor, Some(EntityKey(3)));
        assert!(manifest.carve_redirects);

        // An active contradiction makes the slope impassable — descent stalls.
        assert!(manifest.contradiction_stalls);

        // Speaking the counterfactual materializes the untaken branch.
        assert!(manifest.counterfactual_opens_fork);

        // The energy ledger balances in every scenario.
        assert!(manifest.all_energy_conserved);
        assert_eq!(manifest.status, "validated_for_fixture");
    }
}
