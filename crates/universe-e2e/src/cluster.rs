//! A bounded cluster-from-space builder.
//!
//! `cluster_from_space` selects the bounded physical working set of a
//! `space`-role node and assembles it into the [`LocalAtomCluster`] shape that
//! [`universe_physics::execute_local_atom_cluster`] consumes. It mirrors the
//! `magic_object` decorator (space excluded so it never diffuses into itself)
//! but sources its members from a live `UniverseSnapshot` instead of an
//! authored blueprint.
//!
//! This is a SELECTION primitive, not a physics author. It walks the space's
//! `PART_OF` / `APPLIES_IN` membership frontier with the existing bounded query
//! primitives (`universe-query`) and returns which atoms and intra-member bonds
//! make up the cluster. The generic snapshot carries membership and atom
//! identity but no per-atom firing physics (threshold, polarity, and energy
//! live only in authored circuit blocks, dropped at materialization), so the
//! selected atoms are emitted PHYSICS-INERT: an unauthored threshold of
//! `u64::MAX` (an atom the graph did not authorise to fire cannot fire) and
//! neutral zero-energy bonds. It never invents precision the snapshot lacks.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use universe_core::{EntityKey, RelationKey};
use universe_physics::{AtomBond, AtomSpec, BondPolarity, LocalAtomCluster};
use universe_query::{
    read_local_binding_subgraph, AdjacencyIndex, LocalGraph, LocalRelation, QueryBudget,
    QueryOrigin, QueryStatus,
};
use universe_store::UniverseSnapshot;

use crate::E2eError;

/// Predicate names whose edges attach a member to a space. Membership is walked
/// through these and only these; every other edge is treated as a candidate
/// bond or ignored.
pub const MEMBERSHIP_PREDICATES: [&str; 2] = ["PART_OF", "APPLIES_IN"];

/// The graph-authored ceiling on how large a selected cluster may grow. Both
/// bounds are hard: exceeding either truncates deterministically and the
/// selection reports [`ClusterStatus::BudgetExhausted`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClusterSelectionBudget {
    pub max_atoms: usize,
    pub max_bonds: usize,
}

/// Whether the selection captured the whole membership frontier within budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClusterStatus {
    /// Every member and intra-member bond fit within the budget and the bounded
    /// walk drained its frontier.
    Complete,
    /// The budget (atoms, bonds, or the bounded walk itself) was reached before
    /// the frontier was exhausted; the cluster is a bounded truncation.
    BudgetExhausted,
}

/// The bounded working set selected for one space node.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpaceCluster {
    /// The space the cluster was selected for. It is deliberately NOT one of
    /// the atoms — it bounds diffusion, it does not participate in it.
    pub space: EntityKey,
    pub cluster: LocalAtomCluster,
    pub status: ClusterStatus,
    /// Count of selected atoms (mirrors `cluster.atoms.len()`, kept for readback
    /// without borrowing the cluster).
    pub member_count: usize,
    /// Count of selected intra-member bonds.
    pub bond_count: usize,
}

/// Select the bounded atom cluster of a space node.
///
/// The space node itself is excluded from the returned atoms so the cluster can
/// never diffuse energy into its own container. The returned
/// [`LocalAtomCluster`] is a valid input to
/// [`universe_physics::execute_local_atom_cluster`].
pub fn cluster_from_space(
    snapshot: &UniverseSnapshot,
    space: EntityKey,
    budget: ClusterSelectionBudget,
) -> Result<SpaceCluster, E2eError> {
    // Build a bounded local graph over the provided snapshot. This mirrors the
    // index construction in `RealReadHost::new`: an index built once from the
    // supplied snapshot, then walked only through bounded local primitives.
    let graph = AdjacencyIndex::from_parts(
        snapshot.entities.iter().map(|entity| entity.key),
        snapshot.relations.iter().map(|relation| LocalRelation {
            key: relation.key,
            source: relation.source,
            target: relation.target,
        }),
    );
    if !graph.contains(space) {
        return Err(E2eError::Contract(format!(
            "space node is absent from snapshot: {space}"
        )));
    }

    // A relation-key -> predicate lookup (LocalRelation drops the predicate).
    let predicate_of: BTreeMap<RelationKey, u32> = snapshot
        .relations
        .iter()
        .map(|relation| (relation.key, relation.predicate))
        .collect();
    // Membership predicate symbol ids that actually exist in this snapshot.
    let membership_ids: BTreeSet<u32> = MEMBERSHIP_PREDICATES
        .iter()
        .filter_map(|name| {
            snapshot
                .symbols
                .iter()
                .position(|symbol| symbol == name)
                .map(|index| index as u32)
        })
        .collect();
    let is_membership = |key: &RelationKey| {
        predicate_of
            .get(key)
            .map(|predicate| membership_ids.contains(predicate))
            .unwrap_or(false)
    };

    // Walk the space's membership frontier with the bounded query primitive.
    // Depth 1 inspects exactly the space's own edges; the budget caps how much
    // of that frontier is inspected so the walk is bounded, never global.
    let query_budget = QueryBudget {
        max_entities: budget.max_atoms.saturating_add(1),
        max_relations: budget.max_atoms.saturating_mul(2).saturating_add(1),
        max_depth: 1,
    };
    let frontier = read_local_binding_subgraph(&graph, QueryOrigin::Entity(space), query_budget);

    // Members: the non-space endpoint of every membership edge touching space.
    let mut members_all: BTreeSet<EntityKey> = BTreeSet::new();
    for relation in &frontier.relations {
        if !is_membership(&relation.key) {
            continue;
        }
        let member = if relation.source == space {
            relation.target
        } else if relation.target == space {
            relation.source
        } else {
            continue;
        };
        // Exclude the space node itself (guards a PART_OF self-loop).
        if member == space {
            continue;
        }
        members_all.insert(member);
    }

    // Truncate deterministically (BTreeSet iterates in key order) to the atom
    // budget. Truncation is a bounded status, not an error.
    let members_truncated = members_all.len() > budget.max_atoms;
    let members: Vec<EntityKey> = members_all.iter().copied().take(budget.max_atoms).collect();
    let member_set: BTreeSet<EntityKey> = members.iter().copied().collect();
    if member_set.contains(&space) {
        return Err(E2eError::Contract(
            "space node leaked into its own cluster".into(),
        ));
    }

    // Bonds: relations whose BOTH endpoints are selected members. Enumerated
    // through each member's local adjacency (a bounded per-entity primitive),
    // never a whole-snapshot scan. Membership edges (which touch the space, not
    // two members) and self-loops are excluded.
    let mut seen_bonds: BTreeSet<RelationKey> = BTreeSet::new();
    let mut bond_candidates: Vec<LocalRelation> = Vec::new();
    for member in &members {
        for relation in graph.adjacent(*member) {
            if relation.source == relation.target {
                continue;
            }
            if !member_set.contains(&relation.source) || !member_set.contains(&relation.target) {
                continue;
            }
            if is_membership(&relation.key) {
                continue;
            }
            if seen_bonds.insert(relation.key) {
                bond_candidates.push(relation);
            }
        }
    }
    bond_candidates.sort_by_key(|relation| relation.key);
    let bonds_truncated = bond_candidates.len() > budget.max_bonds;
    bond_candidates.truncate(budget.max_bonds);

    let atoms: Vec<AtomSpec> = members
        .iter()
        .map(|key| AtomSpec {
            key: *key,
            // Inert: the generic snapshot authors no firing threshold, so an
            // unauthorised atom cannot fire. u64::MAX is honest "unknown".
            threshold: u64::MAX,
            seed_energy: 0,
            required_supports: Vec::new(),
            inhibition_threshold: None,
        })
        .collect();
    let bonds: Vec<AtomBond> = bond_candidates
        .iter()
        .map(|relation| AtomBond {
            key: relation.key,
            source: relation.source,
            target: relation.target,
            // Unknown polarity -> neutral, zero energy (never invented as a
            // measured transfer).
            polarity: BondPolarity::Neutral,
            energy: 0,
        })
        .collect();

    let status = if members_truncated
        || bonds_truncated
        || frontier.situation.status == QueryStatus::BudgetExhausted
    {
        ClusterStatus::BudgetExhausted
    } else {
        ClusterStatus::Complete
    };

    let member_count = atoms.len();
    let bond_count = bonds.len();
    Ok(SpaceCluster {
        space,
        cluster: LocalAtomCluster {
            atoms,
            bonds,
            injections: Vec::new(),
        },
        status,
        member_count,
        bond_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_core::UniverseId;
    use universe_physics::{execute_local_atom_cluster, AtomConvergence, AtomExecutionBudget};
    use universe_store::{EntityRecord, RelationRecord};

    // A tiny hand-made snapshot:
    //   space(1) <-PART_OF- m2(2), m3(3), m4(4)
    //   m2 -BOND-> m3                (intra-member bond)
    //   m2 -BOND-> outsider(5)       (endpoint not a member -> not a bond)
    //   space -BOND-> outsider(5)    (touches space, never selected)
    // Symbol layout: 0 instance_of, 1 PART_OF, 2 Space, 3 Atom, 4 BOND.
    const SPACE: EntityKey = EntityKey(1);
    const M2: EntityKey = EntityKey(2);
    const M3: EntityKey = EntityKey(3);
    const M4: EntityKey = EntityKey(4);
    const OUTSIDER: EntityKey = EntityKey(5);

    fn entity(key: EntityKey, symbol: u32) -> EntityRecord {
        EntityRecord {
            key,
            generation: 0,
            symbol,
            content: None,
        }
    }

    fn relation(key: u128, source: EntityKey, target: EntityKey, predicate: u32) -> RelationRecord {
        RelationRecord {
            key: RelationKey(key),
            generation: 0,
            source,
            target,
            predicate,
            content: None,
        }
    }

    fn fixture() -> UniverseSnapshot {
        let mut snapshot = UniverseSnapshot::empty(UniverseId(1));
        snapshot.symbols = vec![
            "instance_of".into(),
            "PART_OF".into(),
            "Space".into(),
            "Atom".into(),
            "BOND".into(),
        ];
        snapshot.entities = vec![
            entity(SPACE, 2),
            entity(M2, 3),
            entity(M3, 3),
            entity(M4, 3),
            entity(OUTSIDER, 3),
        ];
        snapshot.relations = vec![
            relation(1, M2, SPACE, 1),      // PART_OF
            relation(2, M3, SPACE, 1),      // PART_OF
            relation(3, M4, SPACE, 1),      // PART_OF
            relation(4, M2, M3, 4),         // BOND (intra-member)
            relation(5, M2, OUTSIDER, 4),   // BOND but outsider is not a member
            relation(6, SPACE, OUTSIDER, 4), // touches space
        ];
        snapshot
    }

    #[test]
    fn selects_members_excludes_space_and_stays_consumable() {
        let snapshot = fixture();
        let selection = cluster_from_space(
            &snapshot,
            SPACE,
            ClusterSelectionBudget {
                max_atoms: 10,
                max_bonds: 10,
            },
        )
        .unwrap();

        assert_eq!(selection.status, ClusterStatus::Complete);
        let atom_keys: BTreeSet<EntityKey> =
            selection.cluster.atoms.iter().map(|atom| atom.key).collect();
        // The three members are selected; the space and the outsider are not.
        assert_eq!(atom_keys, BTreeSet::from([M2, M3, M4]));
        // The space node is excluded from its own cluster — the load-bearing
        // invariant (mirrors magic_object: the space bounds diffusion).
        assert!(!atom_keys.contains(&SPACE));
        assert_eq!(selection.space, SPACE);

        // Only the intra-member bond survives; edges touching the outsider or
        // the space are not bonds.
        let bond_keys: BTreeSet<RelationKey> =
            selection.cluster.bonds.iter().map(|bond| bond.key).collect();
        assert_eq!(bond_keys, BTreeSet::from([RelationKey(4)]));

        // The assembled shape is a valid input to the physics primitive and
        // settles trivially (inert atoms never fire).
        let receipt = execute_local_atom_cluster(
            selection.cluster.clone(),
            AtomExecutionBudget {
                max_atoms: 16,
                max_bonds: 16,
                max_steps: 8,
                max_total_energy: 1_000,
            },
        )
        .unwrap();
        assert_eq!(receipt.convergence, AtomConvergence::Quiescent);
        assert!(receipt.containment.within_budget);
    }

    #[test]
    fn budget_bounds_the_size() {
        let snapshot = fixture();
        let selection = cluster_from_space(
            &snapshot,
            SPACE,
            ClusterSelectionBudget {
                max_atoms: 2,
                max_bonds: 10,
            },
        )
        .unwrap();

        // Three members exist but the atom budget is two -> truncated + flagged.
        assert_eq!(selection.member_count, 2);
        assert_eq!(selection.cluster.atoms.len(), 2);
        assert_eq!(selection.status, ClusterStatus::BudgetExhausted);

        // The space node is still excluded under a tight budget.
        assert!(selection
            .cluster
            .atoms
            .iter()
            .all(|atom| atom.key != SPACE));

        // A bond is kept only if BOTH its endpoints survived truncation.
        for bond in &selection.cluster.bonds {
            assert!(selection
                .cluster
                .atoms
                .iter()
                .any(|atom| atom.key == bond.source));
            assert!(selection
                .cluster
                .atoms
                .iter()
                .any(|atom| atom.key == bond.target));
        }
    }

    #[test]
    fn separate_bond_budget_bounds_bonds() {
        // space(10) with two members fully bonded to each other twice.
        let mut snapshot = UniverseSnapshot::empty(UniverseId(2));
        snapshot.symbols = vec!["PART_OF".into(), "Space".into(), "BOND".into()];
        let space = EntityKey(10);
        let a = EntityKey(11);
        let b = EntityKey(12);
        snapshot.entities = vec![entity(space, 1), entity(a, 1), entity(b, 1)];
        snapshot.relations = vec![
            relation(1, a, space, 0),
            relation(2, b, space, 0),
            relation(3, a, b, 2),
            relation(4, b, a, 2),
        ];

        let selection = cluster_from_space(
            &snapshot,
            space,
            ClusterSelectionBudget {
                max_atoms: 10,
                max_bonds: 1,
            },
        )
        .unwrap();
        assert_eq!(selection.member_count, 2);
        assert_eq!(selection.bond_count, 1);
        assert_eq!(selection.status, ClusterStatus::BudgetExhausted);
    }

    #[test]
    fn absent_space_is_a_contract_error() {
        let snapshot = fixture();
        let error = cluster_from_space(
            &snapshot,
            EntityKey(999),
            ClusterSelectionBudget {
                max_atoms: 4,
                max_bonds: 4,
            },
        );
        assert!(matches!(error, Err(E2eError::Contract(_))));
    }
}
