//! Magic object blueprint + decorator.
//!
//! A magic object is a `space` (roleAxis: conteneur — "borne la diffusion",
//! canonical ontology l.2278-2282) that contains a cluster of role-typed nodes
//! and whose gradient-policy governs how an actor's injected energy diffuses.
//! The object *contains and governs a context*, which is exactly the ontology's
//! criterion for the `space` role (the institution override, l.5623-5626).
//!
//! The **decorator** ([`MagicObject::decorate`]) wraps a bare, role-typed
//! cluster into a bounded, validated magic object runnable on `AtomDynamics`
//! (the existing physics primitive — this module owns no new law). The `space`
//! node is the boundary: it bounds the diffusion but does not itself carry
//! energy, so it is excluded from the runnable cluster. Transformation is done
//! by the `thing` (routeur — "laisse passer en transformant") nodes *inside*.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use universe_core::{EntityKey, RelationKey};
use universe_physics::{AtomBond, AtomDynamics, AtomSpec, BondPolarity};

use crate::E2eError;

/// The single documented default step ceiling for a magic object's bounded
/// diffusion, applied **only** when the graph (the blueprint) declares no
/// `quiescence_budget`. This is the one outermost boundary default; the
/// mechanism ([`MagicObject::wield`]) must never carry a magic literal —
/// exactly as `execute_runtime_bond_artifact` sources its ceiling from
/// `plan.budgets` rather than a buried constant.
const DEFAULT_QUIESCENCE_BUDGET: usize = 64;

/// The physical roleAxis: the primary axis, orthogonal to the epistemic
/// nodeType, defined by what a node does with energy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// pompe — injects energy.
    Actor,
    /// passage — transmits and absorbs.
    Moment,
    /// attracteur — retains.
    Narrative,
    /// conteneur — bounds the diffusion.
    Space,
    /// routeur — lets energy pass while transforming it.
    Thing,
}

impl Role {
    pub fn is_container(self) -> bool {
        matches!(self, Role::Space)
    }
}

/// What the object energizes and inhibits: the policy that governs the local
/// deformation. It is graph data, never hidden logic.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GradientPolicy {
    #[serde(default)]
    pub energizes: Vec<String>,
    #[serde(default)]
    pub inhibits: Vec<String>,
    #[serde(default)]
    pub fork_predicate: Option<String>,
    #[serde(default)]
    pub answer_predicate: Option<String>,
}

/// The object's self-description of how to activate it, rendered as a sphere
/// floating above it. It `EXPLAINS` the object's own `space`. It is metadata,
/// **not** a physics participant: it never diffuses energy. Because activating a
/// magic object means `ChangeSet -> Commit -> Physics` (physics runs only after
/// the commit receipt), the sphere states exactly that runtime path.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ActivationSphere {
    pub role: Role,
    pub shape: String,
    /// The `space` key this sphere explains (the object itself).
    pub explains: u128,
    #[serde(default)]
    pub capability: Option<String>,
    #[serde(default)]
    pub how: Option<String>,
    #[serde(default)]
    pub gestures: Vec<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub promise: Option<String>,
    #[serde(default)]
    pub forbidden: Vec<String>,
}

/// One wield gesture: a measured injection into a bound node by the actor. The
/// magnitude is the gesture's force; it is not graph structure.
pub struct Gesture<'a> {
    pub binding: &'a str,
    pub energy: u64,
}

/// One node of a derived (non-fixture) cluster passed to [`MagicObject::from_parts`].
pub struct PartNode {
    pub key: u128,
    pub role: Role,
    pub function: String,
    pub binding: Option<String>,
    pub threshold: u64,
    pub seed_energy: u64,
    pub required_supports: Vec<u128>,
    pub inhibition_threshold: Option<u64>,
}

/// One bond of a derived cluster passed to [`MagicObject::from_parts`].
pub struct PartBond {
    pub key: u128,
    pub source: u128,
    pub target: u128,
    pub polarity: BondPolarity,
    pub energy: u64,
}

/// The settled result of wielding the object: which nodes fired, their support,
/// and the physical ledger. Each object *interprets* this in its own terms.
#[derive(Clone, Debug)]
pub struct Activation {
    pub fired: Vec<EntityKey>,
    pub support: BTreeMap<EntityKey, u64>,
    pub energy_conserved: bool,
    pub quiescent: bool,
}

#[derive(Debug, Deserialize)]
struct Blueprint {
    magic_object: String,
    space: SpaceDecl,
    #[serde(default)]
    gradient_policy: GradientPolicy,
    /// Graph-authored ceiling on physics steps for the bounded diffusion. When
    /// absent, the object falls back to [`DEFAULT_QUIESCENCE_BUDGET`] — the sole
    /// documented default. The space bounds the diffusion; this bounds its time.
    #[serde(default)]
    quiescence_budget: Option<usize>,
    contents: Vec<ContentDecl>,
    bonds: Vec<BondDecl>,
    #[serde(default)]
    activation: Option<ActivationSphere>,
}

#[derive(Debug, Deserialize)]
struct SpaceDecl {
    key: u128,
    role: Role,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContentDecl {
    key: u128,
    role: Role,
    function: String,
    #[serde(default)]
    measurement_binding: Option<String>,
    physics: PhysicsDecl,
}

#[derive(Debug, Deserialize)]
struct PhysicsDecl {
    threshold: u64,
    #[serde(default)]
    seed_energy: u64,
    #[serde(default)]
    required_supports: Vec<u128>,
    #[serde(default)]
    inhibition_threshold: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct BondDecl {
    key: u128,
    source: u128,
    target: u128,
    polarity: BondPolarity,
    energy: u64,
}

/// A decorated, bounded magic object.
pub struct MagicObject {
    pub name: String,
    pub space: EntityKey,
    pub space_name: Option<String>,
    pub policy: GradientPolicy,
    /// The sphere that explains how to activate this object, if declared.
    pub activation: Option<ActivationSphere>,
    /// The resolved step ceiling for the bounded diffusion, sourced from graph
    /// authority (the blueprint) or the single documented default. `wield` reads
    /// this instead of a buried literal.
    quiescence_budget: usize,
    atoms: Vec<AtomSpec>,
    bonds: Vec<AtomBond>,
    roles: BTreeMap<EntityKey, Role>,
    functions: BTreeMap<EntityKey, String>,
    bindings: BTreeMap<String, EntityKey>,
}

impl MagicObject {
    /// Load a blueprint fixture and decorate it.
    pub fn load(path: &Path) -> Result<Self, E2eError> {
        let bytes = fs::read(path).map_err(|error| E2eError::Io(error.to_string()))?;
        let blueprint: Blueprint = serde_json::from_slice(&bytes)
            .map_err(|error| E2eError::Contract(error.to_string()))?;
        Self::decorate(blueprint)
    }

    /// Decorate a derived, in-memory cluster (not a fixture) into a magic object,
    /// reusing the exact same blueprint validation as [`Self::load`]. This is the
    /// single path shared by fixtures and by derived rides over the real store.
    pub fn from_parts(
        name: String,
        space: u128,
        space_name: Option<String>,
        policy: GradientPolicy,
        nodes: Vec<PartNode>,
        bonds: Vec<PartBond>,
    ) -> Result<Self, E2eError> {
        let blueprint = Blueprint {
            magic_object: name,
            space: SpaceDecl {
                key: space,
                role: Role::Space,
                name: space_name,
            },
            gradient_policy: policy,
            // A derived (non-fixture) cluster carries no graph-declared budget,
            // so it resolves to the single documented default in `decorate`.
            quiescence_budget: None,
            contents: nodes
                .into_iter()
                .map(|node| ContentDecl {
                    key: node.key,
                    role: node.role,
                    function: node.function,
                    measurement_binding: node.binding,
                    physics: PhysicsDecl {
                        threshold: node.threshold,
                        seed_energy: node.seed_energy,
                        required_supports: node.required_supports,
                        inhibition_threshold: node.inhibition_threshold,
                    },
                })
                .collect(),
            bonds: bonds
                .into_iter()
                .map(|bond| BondDecl {
                    key: bond.key,
                    source: bond.source,
                    target: bond.target,
                    polarity: bond.polarity,
                    energy: bond.energy,
                })
                .collect(),
            activation: None,
        };
        Self::decorate(blueprint)
    }

    /// The decorator: wrap a role-typed cluster into a bounded magic object.
    ///
    /// It enforces the blueprint: the container has the `space` role, contents
    /// are non-container role-typed nodes, bindings are unique, and every bond
    /// stays strictly inside the space boundary (the space bounds the
    /// diffusion). The `space` node is not a physics participant.
    fn decorate(blueprint: Blueprint) -> Result<Self, E2eError> {
        if !blueprint.space.role.is_container() {
            return Err(E2eError::Contract(
                "magic object space must have role `space` (conteneur)".into(),
            ));
        }
        let space = EntityKey(blueprint.space.key);

        let mut atoms = Vec::new();
        let mut roles = BTreeMap::new();
        let mut functions = BTreeMap::new();
        let mut bindings = BTreeMap::new();
        let mut content_keys = BTreeSet::new();
        for content in &blueprint.contents {
            let key = EntityKey(content.key);
            if content.role.is_container() {
                return Err(E2eError::Contract(
                    "a content node may not itself be a space in blueprint v0".into(),
                ));
            }
            if key == space {
                return Err(E2eError::Contract(
                    "a content node reuses the space key".into(),
                ));
            }
            if !content_keys.insert(key) {
                return Err(E2eError::Contract(format!("duplicate content key {key}")));
            }
            roles.insert(key, content.role);
            functions.insert(key, content.function.clone());
            if let Some(binding) = &content.measurement_binding {
                if bindings.insert(binding.clone(), key).is_some() {
                    return Err(E2eError::Contract(format!(
                        "duplicate measurement binding {binding}"
                    )));
                }
            }
            atoms.push(AtomSpec {
                key,
                threshold: content.physics.threshold,
                seed_energy: content.physics.seed_energy,
                required_supports: content
                    .physics
                    .required_supports
                    .iter()
                    .map(|support| RelationKey(*support))
                    .collect(),
                inhibition_threshold: content.physics.inhibition_threshold,
            });
        }
        if atoms.is_empty() {
            return Err(E2eError::Contract("magic object contains no nodes".into()));
        }

        let mut bonds = Vec::new();
        for bond in &blueprint.bonds {
            let source = EntityKey(bond.source);
            let target = EntityKey(bond.target);
            if !content_keys.contains(&source) || !content_keys.contains(&target) {
                return Err(E2eError::Contract(
                    "a bond endpoint escapes the object's space boundary".into(),
                ));
            }
            bonds.push(AtomBond {
                key: RelationKey(bond.key),
                source,
                target,
                polarity: bond.polarity,
                energy: bond.energy,
            });
        }

        if let Some(activation) = &blueprint.activation {
            if activation.role.is_container() {
                return Err(E2eError::Contract(
                    "the activation sphere may not itself be a space".into(),
                ));
            }
            if EntityKey(activation.explains) != space {
                return Err(E2eError::Contract(
                    "the activation sphere must explain this object's own space".into(),
                ));
            }
            if activation.shape.trim().is_empty() {
                return Err(E2eError::Contract(
                    "the activation sphere must declare a shape".into(),
                ));
            }
        }

        // Source the diffusion's step ceiling from graph authority (the
        // blueprint); fall back to the one documented default only when the
        // graph declares none. No magic literal lives in the mechanism.
        let quiescence_budget = blueprint
            .quiescence_budget
            .unwrap_or(DEFAULT_QUIESCENCE_BUDGET);

        Ok(Self {
            name: blueprint.magic_object,
            space,
            space_name: blueprint.space.name,
            policy: blueprint.gradient_policy,
            activation: blueprint.activation,
            quiescence_budget,
            atoms,
            bonds,
            roles,
            functions,
            bindings,
        })
    }

    /// Wield the object: inject the actor's gestures and let the activation
    /// diffuse, bounded to the object's space. Returns the settled facts.
    pub fn wield(&self, gestures: &[Gesture<'_>]) -> Result<Activation, E2eError> {
        let mut dynamics = AtomDynamics::new(self.atoms.clone(), self.bonds.clone())?;
        for gesture in gestures {
            let atom = self.bindings.get(gesture.binding).copied().ok_or_else(|| {
                E2eError::Contract(format!("object has no binding {}", gesture.binding))
            })?;
            dynamics.inject(atom, gesture.energy, format!("wield:{}", gesture.binding))?;
        }
        let run = dynamics.run_until_quiescent(self.quiescence_budget)?;
        let mut support = BTreeMap::new();
        let mut fired = Vec::new();
        for atom in &self.atoms {
            if let Some(state) = dynamics.state(atom.key) {
                support.insert(atom.key, state.support_energy);
            }
            if dynamics.fired(atom.key) {
                fired.push(atom.key);
            }
        }
        Ok(Activation {
            fired,
            support,
            energy_conserved: run.energy_conserved,
            quiescent: run.quiescent,
        })
    }

    /// Content nodes carrying a given functional label, sorted.
    pub fn function_nodes(&self, function: &str) -> Vec<EntityKey> {
        let mut nodes: Vec<_> = self
            .functions
            .iter()
            .filter(|(_, label)| label.as_str() == function)
            .map(|(key, _)| *key)
            .collect();
        nodes.sort();
        nodes
    }

    /// Content nodes carrying a given roleAxis role, sorted.
    pub fn nodes_with_role(&self, role: Role) -> Vec<EntityKey> {
        let mut nodes: Vec<_> = self
            .roles
            .iter()
            .filter(|(_, node_role)| **node_role == role)
            .map(|(key, _)| *key)
            .collect();
        nodes.sort();
        nodes
    }

    pub fn role_of(&self, node: EntityKey) -> Option<Role> {
        self.roles.get(&node).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board_fixture() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/atoms/board-descent-v0.json")
    }

    #[test]
    fn a_magic_object_explains_its_own_activation_in_a_sphere() {
        let object = MagicObject::load(&board_fixture()).unwrap();
        let sphere = object
            .activation
            .expect("the board must explain how to activate it");

        // The sphere explains the object itself, and is shaped as a sphere.
        assert_eq!(EntityKey(sphere.explains), object.space);
        assert_eq!(sphere.shape, "sphere");
        assert!(!sphere.gestures.is_empty());

        // It states the real runtime path: activation goes through Commit before
        // any physics transform runs.
        let runtime = sphere.runtime.expect("the sphere must state the runtime");
        assert!(runtime.contains("Commit"));
        assert!(runtime.contains("Physics"));
    }

    #[test]
    fn an_activation_sphere_must_explain_its_own_object() {
        let object = MagicObject::load(&board_fixture()).unwrap();
        // A sphere is metadata, never a physics participant: the space it
        // explains is excluded from the diffusion cluster.
        assert!(object.role_of(object.space).is_none());
    }

    /// A 3-node linear support chain: node 1 seeds and fires, delivering energy
    /// downstream one step at a time, so the diffusion needs several physics
    /// steps to quiesce. The graph-declared `quiescence_budget` is the ceiling.
    fn chain_blueprint(budget: Option<usize>) -> Blueprint {
        let content = |key: u128, threshold: u64, seed: u64| ContentDecl {
            key,
            role: Role::Moment,
            function: format!("n{key}"),
            measurement_binding: None,
            physics: PhysicsDecl {
                threshold,
                seed_energy: seed,
                required_supports: Vec::new(),
                inhibition_threshold: None,
            },
        };
        let bond = |key: u128, source: u128, target: u128, energy: u64| BondDecl {
            key,
            source,
            target,
            polarity: BondPolarity::Support,
            energy,
        };
        Blueprint {
            magic_object: "budget-chain".into(),
            space: SpaceDecl {
                key: 0,
                role: Role::Space,
                name: None,
            },
            gradient_policy: GradientPolicy::default(),
            quiescence_budget: budget,
            contents: vec![
                content(1, 1, 300),
                content(2, 150, 0),
                content(3, 100, 0),
            ],
            bonds: vec![bond(501, 1, 2, 200), bond(502, 2, 3, 100)],
            activation: None,
        }
    }

    #[test]
    fn a_graph_declared_budget_is_the_diffusion_step_ceiling() {
        // The chain needs four loop iterations to reach a terminal empty step.
        // A budget below that must exhaust before quiescence; a budget above it
        // must settle. Proves `wield` honours the graph value, not a literal.
        let starved = MagicObject::decorate(chain_blueprint(Some(2)))
            .unwrap()
            .wield(&[])
            .unwrap();
        assert!(
            !starved.quiescent,
            "a budget of 2 steps must exhaust before the chain settles"
        );

        let settled = MagicObject::decorate(chain_blueprint(Some(16)))
            .unwrap()
            .wield(&[])
            .unwrap();
        assert!(
            settled.quiescent,
            "an ample graph-declared budget must let the chain settle"
        );
    }

    #[test]
    fn an_absent_budget_falls_back_to_the_single_documented_default() {
        // No buried literal in the mechanism: when the graph declares nothing,
        // the object resolves to exactly the one documented boundary default.
        let object = MagicObject::decorate(chain_blueprint(None)).unwrap();
        assert_eq!(object.quiescence_budget, DEFAULT_QUIESCENCE_BUDGET);

        // The fixture path resolves the same way when it omits a budget.
        let board = MagicObject::load(&board_fixture()).unwrap();
        assert_eq!(board.quiescence_budget, DEFAULT_QUIESCENCE_BUDGET);
    }
}
