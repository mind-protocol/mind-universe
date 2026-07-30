//! The Lantern: a STATIC magic object that reveals epistemic status without
//! moving. It is the tightest proof that the [`crate::magic_object`] blueprint is
//! interpretation-agnostic — it uses the SAME MagicObject/decorate/wield as the
//! board, but the readout PARTITIONS nodes (observed / derived / fog) instead of
//! computing a descent attractor. There is no argmax, no carve, no next step.
//!
//! The status is EARNED by the energy ledger, never declared:
//! - observed = fires under zero gesture (self-evidence, independent of the tool);
//! - derived  = fires only under the lantern's halo (evidence via the instrument);
//! - fog      = never fires, even under the halo (not_measured, kept as Fog).
//!
//! `mislabeled` is the epistemic-discipline guard rendered as a physics test: a
//! node authored "observed" that does not self-light, or "derived" that lights
//! with no lantern, is surfaced rather than trusted.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};
use universe_core::EntityKey;

use crate::magic_object::{Gesture, MagicObject};
use crate::E2eError;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LanternReveal {
    pub observed: Vec<EntityKey>,
    pub derived: Vec<EntityKey>,
    pub fog: Vec<EntityKey>,
    /// Nodes whose authored status the physics contradicts.
    pub mislabeled: Vec<EntityKey>,
    pub energy_conserved: bool,
    pub quiescent: bool,
    pub epistemic_status: String,
}

struct Lantern {
    object: MagicObject,
}

impl Lantern {
    fn load(fixture_path: &Path) -> Result<Self, E2eError> {
        Ok(Self {
            object: MagicObject::load(fixture_path)?,
        })
    }

    fn reveal(&self) -> Result<LanternReveal, E2eError> {
        // Two fixed passes — never a moving cursor. DARK: nothing but self-seeded
        // facts light. HALO: the lantern floods support along the evidence bonds.
        let dark = self.object.wield(&[])?;
        let halo = self.object.wield(&[Gesture {
            binding: "illuminate",
            energy: 300,
        }])?;
        let dark_fired: BTreeSet<EntityKey> = dark.fired.iter().copied().collect();
        let halo_fired: BTreeSet<EntityKey> = halo.fired.iter().copied().collect();

        let observed_nodes = self.object.function_nodes("observed");
        let derived_nodes = self.object.function_nodes("derived");
        let fog_nodes = self.object.function_nodes("fog");

        let observed = observed_nodes
            .iter()
            .copied()
            .filter(|node| dark_fired.contains(node))
            .collect();
        let derived = derived_nodes
            .iter()
            .copied()
            .filter(|node| halo_fired.contains(node) && !dark_fired.contains(node))
            .collect();
        let fog = fog_nodes
            .iter()
            .copied()
            .filter(|node| !halo_fired.contains(node))
            .collect();

        // Honesty guard: a claimed-observed node that does not self-light, or a
        // claimed-derived node that lights with no lantern, is mislabeled.
        let mut mislabeled = Vec::new();
        for node in &observed_nodes {
            if !dark_fired.contains(node) {
                mislabeled.push(*node);
            }
        }
        for node in &derived_nodes {
            if dark_fired.contains(node) {
                mislabeled.push(*node);
            }
        }
        mislabeled.sort();

        Ok(LanternReveal {
            observed,
            derived,
            fog,
            mislabeled,
            energy_conserved: dark.energy_conserved && halo.energy_conserved,
            quiescent: dark.quiescent && halo.quiescent,
            epistemic_status: "observed|derived earned by the energy ledger; fog = not_measured"
                .into(),
        })
    }
}

/// Load the lantern magic object and read back the epistemic partition of its
/// neighborhood. It moves nothing and chooses no next step.
pub fn reveal(fixture_path: &Path) -> Result<LanternReveal, E2eError> {
    Lantern::load(fixture_path)?.reveal()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/atoms/lantern-reveal-v0.json")
    }

    #[test]
    fn lantern_reveals_epistemic_status_without_moving() {
        let reveal = reveal(&fixture()).unwrap();

        // observed = self-lit under DARK; derived = lit only under the halo;
        // fog = never lit.
        assert_eq!(reveal.observed, vec![EntityKey(1), EntityKey(2)]);
        assert_eq!(reveal.derived, vec![EntityKey(3)]);
        assert_eq!(reveal.fog, vec![EntityKey(4)]);

        // No authored status is contradicted by the physics.
        assert!(reveal.mislabeled.is_empty());
        assert!(reveal.energy_conserved && reveal.quiescent);

        // Static: revealing again is byte-identical — nothing moved.
        let again = super::reveal(&fixture()).unwrap();
        assert_eq!(reveal, again);
    }
}
