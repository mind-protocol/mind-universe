//! Bounded local graph reads over an explicitly supplied adjacency frontier.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use universe_core::{EntityKey, RelationKey, UniverseError};
use universe_store::{
    AdjacentRelations, IndexedUniverseSnapshot, OverlayAdjacentRelations,
    OverlayIndexedUniverseSnapshot, UniverseStore,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum QueryOrigin {
    Entity(EntityKey),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueryBudget {
    pub max_entities: usize,
    pub max_relations: usize,
    pub max_depth: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryStatus {
    Complete,
    FrontierExhausted,
    BudgetExhausted,
    UnknownOrigin,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalSituation {
    pub entities: Vec<EntityKey>,
    pub relations: Vec<RelationKey>,
    pub status: QueryStatus,
    pub visited_entities: usize,
    pub inspected_relations: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalRelation {
    pub key: RelationKey,
    pub source: EntityKey,
    pub target: EntityKey,
}

/// A bounded local subgraph rooted at one graph-supplied binding entity.
///
/// Relation identities and endpoints are returned exactly as stored. This
/// query never interprets predicate names or decides which bindings are
/// semantically required.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalBindingSubgraph {
    pub origin: QueryOrigin,
    pub situation: LocalSituation,
    pub relations: Vec<LocalRelation>,
    /// Endpoints reached by an inspected relation but not visited within the
    /// entity/depth budget.
    pub frontier_entities: Vec<EntityKey>,
}

/// A truth-layer view must provide direct local adjacency, never a global iterator.
pub trait LocalGraph {
    type Adjacent<'a>: Iterator<Item = LocalRelation>
    where
        Self: 'a;

    fn contains(&self, entity: EntityKey) -> bool;
    fn adjacent(&self, entity: EntityKey) -> Self::Adjacent<'_>;
}

/// Read a complete or explicitly partial local binding subgraph without a
/// whole-Universe iterator.
pub fn read_local_binding_subgraph(
    graph: &impl LocalGraph,
    origin: QueryOrigin,
    budget: QueryBudget,
) -> LocalBindingSubgraph {
    let QueryOrigin::Entity(origin_entity) = origin;
    if !graph.contains(origin_entity) {
        return LocalBindingSubgraph {
            origin,
            situation: LocalSituation {
                entities: Vec::new(),
                relations: Vec::new(),
                status: QueryStatus::UnknownOrigin,
                visited_entities: 0,
                inspected_relations: 0,
            },
            relations: Vec::new(),
            frontier_entities: Vec::new(),
        };
    }
    let mut queue = VecDeque::from([(origin_entity, 0usize)]);
    let mut visited = BTreeSet::new();
    let mut relations = BTreeMap::new();
    let mut inspected = 0usize;
    let mut budget_hit = false;
    while let Some((entity, depth)) = queue.pop_front() {
        if visited.contains(&entity) {
            continue;
        }
        if visited.len() >= budget.max_entities {
            budget_hit = true;
            break;
        }
        visited.insert(entity);
        if depth >= budget.max_depth {
            continue;
        }
        for relation in graph.adjacent(entity) {
            if relations.contains_key(&relation.key) {
                // Reusable visit stamp: the collected-relations map doubles as
                // the per-traversal relation stamp. A relation shared by two
                // visited endpoints is inspected at most once, so it consumes
                // relation budget at most once and cannot be double-counted.
                // The endpoint it would reach was already discovered when the
                // relation was first inspected, so skipping loses no frontier.
                continue;
            }
            if inspected >= budget.max_relations {
                budget_hit = true;
                break;
            }
            inspected += 1;
            relations.insert(relation.key, relation);
            let next = if relation.source == entity {
                relation.target
            } else {
                relation.source
            };
            if !visited.contains(&next) {
                queue.push_back((next, depth + 1));
            }
        }
        if budget_hit {
            break;
        }
    }
    let status = if budget_hit || !queue.is_empty() {
        QueryStatus::BudgetExhausted
    } else if visited.len() == 1 && inspected == 0 {
        QueryStatus::FrontierExhausted
    } else {
        QueryStatus::Complete
    };
    let frontier_entities = relations
        .values()
        .flat_map(|relation| [relation.source, relation.target])
        .filter(|entity| !visited.contains(entity))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let relation_values: Vec<_> = relations.values().copied().collect();
    LocalBindingSubgraph {
        origin,
        situation: LocalSituation {
            entities: visited.iter().copied().collect(),
            relations: relations.keys().copied().collect(),
            status,
            visited_entities: visited.len(),
            inspected_relations: inspected,
        },
        relations: relation_values,
        frontier_entities,
    }
}

pub fn graph_read(
    graph: &impl LocalGraph,
    origin: QueryOrigin,
    budget: QueryBudget,
) -> LocalSituation {
    read_local_binding_subgraph(graph, origin, budget).situation
}

#[derive(Default)]
pub struct AdjacencyIndex {
    entities: BTreeSet<EntityKey>,
    adjacency: BTreeMap<EntityKey, Vec<LocalRelation>>,
}

impl AdjacencyIndex {
    pub fn from_parts(
        entities: impl IntoIterator<Item = EntityKey>,
        relations: impl IntoIterator<Item = LocalRelation>,
    ) -> Self {
        let entities = entities.into_iter().collect();
        let mut adjacency: BTreeMap<EntityKey, Vec<LocalRelation>> = BTreeMap::new();
        for relation in relations {
            adjacency.entry(relation.source).or_default().push(relation);
            adjacency.entry(relation.target).or_default().push(relation);
        }
        for values in adjacency.values_mut() {
            values.sort_by_key(|relation| relation.key);
        }
        Self {
            entities,
            adjacency,
        }
    }
}

impl LocalGraph for AdjacencyIndex {
    type Adjacent<'a> = std::iter::Copied<std::slice::Iter<'a, LocalRelation>>;

    fn contains(&self, entity: EntityKey) -> bool {
        self.entities.contains(&entity)
    }

    fn adjacent(&self, entity: EntityKey) -> Self::Adjacent<'_> {
        self.adjacency
            .get(&entity)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .copied()
    }
}

pub struct IndexedAdjacentRelations<'a> {
    inner: AdjacentRelations<'a>,
}

impl Iterator for IndexedAdjacentRelations<'_> {
    type Item = LocalRelation;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|relation| LocalRelation {
            key: relation.key,
            source: relation.source,
            target: relation.target,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for IndexedAdjacentRelations<'_> {}

impl LocalGraph for IndexedUniverseSnapshot {
    type Adjacent<'a> = IndexedAdjacentRelations<'a>;

    fn contains(&self, entity: EntityKey) -> bool {
        self.adjacency().contains(entity)
    }

    fn adjacent(&self, entity: EntityKey) -> Self::Adjacent<'_> {
        IndexedAdjacentRelations {
            inner: self.adjacent_relations(entity),
        }
    }
}

pub struct OverlayIndexedAdjacentRelations<'a> {
    inner: OverlayAdjacentRelations<'a>,
}

impl Iterator for OverlayIndexedAdjacentRelations<'_> {
    type Item = LocalRelation;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|relation| LocalRelation {
            key: relation.key,
            source: relation.source,
            target: relation.target,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for OverlayIndexedAdjacentRelations<'_> {}

impl LocalGraph for OverlayIndexedUniverseSnapshot {
    type Adjacent<'a> = OverlayIndexedAdjacentRelations<'a>;

    fn contains(&self, entity: EntityKey) -> bool {
        self.contains(entity)
    }

    fn adjacent(&self, entity: EntityKey) -> Self::Adjacent<'_> {
        OverlayIndexedAdjacentRelations {
            inner: self.adjacent_relations(entity),
        }
    }
}

/// The capability set one actor holds, materialized from the graph by a bounded
/// local read. It is exactly the set the actor's outgoing grant edges name — no
/// more. This structure carries no admission decision: whether any capability
/// admits a mutate is decided by the caller (e.g. a sealed capability port)
/// against `capabilities`, never here.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeldCapabilitySet {
    pub actor: EntityKey,
    pub capabilities: BTreeSet<String>,
    /// The grant relation keys that contributed a capability, for attribution.
    pub grant_relations: Vec<RelationKey>,
    pub status: QueryStatus,
    pub inspected_relations: usize,
}

/// Materialize an actor's held capability set from the graph with a bounded,
/// local read — never a whole-store scan.
///
/// Only relations OUTGOING from `actor` whose predicate is `grant_predicate` are
/// followed; each target entity's content `capability_field` string is the held
/// capability. Work is bounded by the actor's own incident-relation count and by
/// `max_relations`; each capability entity is resolved by a binary search over
/// the key-sorted entity column (no linear scan). Reaching the relation budget
/// before the actor's adjacency is exhausted is reported as `BudgetExhausted`,
/// never silently truncated.
///
/// Epistemic honesty: the returned set is read verbatim. Holding an observe
/// capability never implies holding an authority/mutate capability; this reader
/// interprets no predicate meaning beyond "is this the grant predicate", and
/// makes no admission decision.
pub fn read_actor_capability_set(
    indexed: &IndexedUniverseSnapshot,
    store: &UniverseStore,
    actor: EntityKey,
    grant_predicate: u32,
    capability_field: &str,
    max_relations: usize,
) -> Result<HeldCapabilitySet, UniverseError> {
    if !indexed.adjacency().contains(actor) {
        return Ok(HeldCapabilitySet {
            actor,
            capabilities: BTreeSet::new(),
            grant_relations: Vec::new(),
            status: QueryStatus::UnknownOrigin,
            inspected_relations: 0,
        });
    }
    let entities = &indexed.snapshot().entities;
    let mut capabilities = BTreeSet::new();
    let mut grant_relations = Vec::new();
    let mut inspected = 0usize;
    let mut budget_hit = false;
    for relation in indexed.adjacent_relations(actor) {
        if inspected >= max_relations {
            budget_hit = true;
            break;
        }
        inspected += 1;
        // Held-by is directional: only edges the actor is the SOURCE of grant a
        // capability to it. An incoming grant predicate (someone else's grant)
        // never widens this actor's set.
        if relation.predicate != grant_predicate || relation.source != actor {
            continue;
        }
        // Bounded key lookup: the entity column is key-sorted, so this is a
        // binary search, not a whole-store scan.
        let target = entities
            .binary_search_by_key(&relation.target, |entity| entity.key)
            .ok()
            .map(|index| &entities[index])
            .ok_or_else(|| {
                UniverseError::Validation(format!(
                    "capability grant target {} is not an entity in this snapshot",
                    relation.target
                ))
            })?;
        let content_ref = target.content.as_ref().ok_or_else(|| {
            UniverseError::Validation(format!(
                "capability entity {} has no content to resolve",
                relation.target
            ))
        })?;
        let content = store.read_content(content_ref)?;
        let capability = content
            .get(capability_field)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                UniverseError::Validation(format!(
                    "capability entity {} has no string field '{capability_field}'",
                    relation.target
                ))
            })?;
        capabilities.insert(capability.to_string());
        grant_relations.push(relation.key);
    }
    let status = if budget_hit {
        QueryStatus::BudgetExhausted
    } else {
        QueryStatus::Complete
    };
    Ok(HeldCapabilitySet {
        actor,
        capabilities,
        grant_relations,
        status,
        inspected_relations: inspected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_testkit::{
        create_behavior_bond_authority_store, minimal_snapshot, BEHAVIOR_BOND_AUTHORITY_KEYS,
    };

    #[test]
    fn genesis_read_is_deterministic_and_local() {
        let snapshot = minimal_snapshot();
        let index = AdjacencyIndex::from_parts(
            snapshot.entities.iter().map(|entity| entity.key),
            snapshot.relations.iter().map(|relation| LocalRelation {
                key: relation.key,
                source: relation.source,
                target: relation.target,
            }),
        );
        let budget = QueryBudget {
            max_entities: 2,
            max_relations: 2,
            max_depth: 1,
        };
        let first = graph_read(&index, QueryOrigin::Entity(EntityKey(1)), budget);
        let second = graph_read(&index, QueryOrigin::Entity(EntityKey(1)), budget);
        assert_eq!(first, second);
        assert_eq!(first.entities, vec![EntityKey(1), EntityKey(2)]);
        assert_eq!(first.visited_entities, 2);
    }

    #[test]
    fn counters_prove_work_is_bounded_independently_of_total_entities() {
        let index = AdjacencyIndex::from_parts(
            (1..=10_000).map(EntityKey),
            [LocalRelation {
                key: RelationKey(1),
                source: EntityKey(1),
                target: EntityKey(2),
            }],
        );
        let result = graph_read(
            &index,
            QueryOrigin::Entity(EntityKey(1)),
            QueryBudget {
                max_entities: 2,
                max_relations: 1,
                max_depth: 1,
            },
        );
        assert_eq!(result.visited_entities, 2);
        assert_eq!(result.inspected_relations, 1);
        assert!(result.visited_entities < 10_000);
    }

    #[test]
    fn binding_subgraph_returns_exact_endpoints_without_predicate_interpretation() {
        let relations = [
            LocalRelation {
                key: RelationKey(11),
                source: EntityKey(1),
                target: EntityKey(2),
            },
            LocalRelation {
                key: RelationKey(12),
                source: EntityKey(1),
                target: EntityKey(3),
            },
        ];
        let index =
            AdjacencyIndex::from_parts([EntityKey(1), EntityKey(2), EntityKey(3)], relations);

        let subgraph = read_local_binding_subgraph(
            &index,
            QueryOrigin::Entity(EntityKey(1)),
            QueryBudget {
                max_entities: 3,
                max_relations: 4,
                max_depth: 1,
            },
        );

        assert_eq!(subgraph.situation.status, QueryStatus::Complete);
        assert_eq!(subgraph.relations, relations);
        assert!(subgraph.frontier_entities.is_empty());
        assert_eq!(
            subgraph.situation.relations,
            vec![RelationKey(11), RelationKey(12)]
        );
    }

    #[test]
    fn partial_binding_subgraph_preserves_budget_and_frontier_evidence() {
        let index = AdjacencyIndex::from_parts(
            (1..=10_000).map(EntityKey),
            [
                LocalRelation {
                    key: RelationKey(11),
                    source: EntityKey(1),
                    target: EntityKey(2),
                },
                LocalRelation {
                    key: RelationKey(12),
                    source: EntityKey(1),
                    target: EntityKey(3),
                },
                LocalRelation {
                    key: RelationKey(13),
                    source: EntityKey(1),
                    target: EntityKey(4),
                },
            ],
        );

        let subgraph = read_local_binding_subgraph(
            &index,
            QueryOrigin::Entity(EntityKey(1)),
            QueryBudget {
                max_entities: 2,
                max_relations: 2,
                max_depth: 1,
            },
        );

        assert_eq!(subgraph.situation.status, QueryStatus::BudgetExhausted);
        assert_eq!(subgraph.situation.visited_entities, 1);
        assert_eq!(subgraph.situation.inspected_relations, 2);
        assert_eq!(subgraph.relations.len(), 2);
        assert_eq!(subgraph.frontier_entities, vec![EntityKey(2), EntityKey(3)]);
        assert!(subgraph.situation.visited_entities < 10_000);
    }

    #[test]
    fn shared_relation_is_inspected_once_and_budget_is_not_double_counted() {
        // A path 1 -- R11 -- 2 -- R12 -- 3. With a depth budget that visits
        // node 2, node 2's adjacency lists R11 again (it is stored on both
        // endpoints). The visit stamp must skip that second sighting so the
        // relation is neither re-inspected nor charged to the budget twice.
        let index = AdjacencyIndex::from_parts(
            [EntityKey(1), EntityKey(2), EntityKey(3)],
            [
                LocalRelation {
                    key: RelationKey(11),
                    source: EntityKey(1),
                    target: EntityKey(2),
                },
                LocalRelation {
                    key: RelationKey(12),
                    source: EntityKey(2),
                    target: EntityKey(3),
                },
            ],
        );

        let subgraph = read_local_binding_subgraph(
            &index,
            QueryOrigin::Entity(EntityKey(1)),
            QueryBudget {
                max_entities: 8,
                max_relations: 8,
                max_depth: 2,
            },
        );

        // Node 2 was visited (depth 1) so its adjacency, which re-lists R11,
        // was scanned. Without the stamp this would report 3 inspections.
        assert_eq!(subgraph.situation.visited_entities, 3);
        assert_eq!(subgraph.situation.inspected_relations, 2);
        // Invariant: every counted inspection produced exactly one distinct
        // relation. No relation is double-counted and none is silently dropped.
        assert_eq!(
            subgraph.situation.inspected_relations,
            subgraph.relations.len()
        );
        let distinct_keys = subgraph
            .relations
            .iter()
            .map(|relation| relation.key)
            .collect::<BTreeSet<_>>();
        assert_eq!(distinct_keys.len(), subgraph.relations.len());
        // The whole neighbourhood fit in budget, so this is honestly Complete
        // with no frontier — the dedup did not hide any remaining work.
        assert_eq!(subgraph.situation.status, QueryStatus::Complete);
        assert!(subgraph.frontier_entities.is_empty());
    }

    /// End-to-end underground enforcement on REAL read data, short of the
    /// write-path call-site: an authority-grant fixture is installed into a real
    /// store; each actor's held capability set is materialized by the bounded
    /// read; and both sets are adjudicated by the graph-authored sealed hatch.
    /// The maintenance authority is Admitted; the plain citizen fails closed.
    #[test]
    fn held_capability_set_drives_sealed_hatch_admission_end_to_end() {
        use std::path::Path;
        use universe_capabilities::{MutateAdmission, SealedCapabilityPort};
        use universe_store::IndexedUniverseSnapshot;

        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let temp = tempfile::tempdir().unwrap();

        // Install the grant fixture into a real store (canonical seed + grant).
        universe_testkit::install_authority_fixture(
            repo.join("fixtures/ontology/underground-maintenance-grant.json"),
            temp.path(),
        )
        .unwrap();

        // INDEPENDENT reopen from disk, then index for bounded adjacency.
        let store = universe_store::UniverseStore::open(temp.path()).unwrap();
        let snapshot = store.replay(store.load_snapshot().unwrap()).unwrap();
        let grant_predicate = snapshot
            .symbol_id("USED")
            .expect("grant predicate symbol is interned");
        let indexed = IndexedUniverseSnapshot::new(snapshot).unwrap();

        let maintenance = EntityKey(0x5a20);
        let citizen = EntityKey(0x5a21);

        let maintenance_set =
            read_actor_capability_set(&indexed, &store, maintenance, grant_predicate, "capability", 32)
                .unwrap();
        let citizen_set =
            read_actor_capability_set(&indexed, &store, citizen, grant_predicate, "capability", 32)
                .unwrap();

        // The read is bounded, complete, and materializes exactly the held set.
        assert_eq!(maintenance_set.status, QueryStatus::Complete);
        assert_eq!(citizen_set.status, QueryStatus::Complete);
        assert!(maintenance_set
            .capabilities
            .contains("authority:underground-maintenance"));
        assert_eq!(maintenance_set.grant_relations.len(), 1);
        // The citizen holds a non-empty set (observe) but NOT the authority cap:
        // fail-closed will be a real decision, not an empty-set artifact.
        assert!(citizen_set.capabilities.contains("observe"));
        assert!(!citizen_set
            .capabilities
            .contains("authority:underground-maintenance"));

        // The sealed hatch is materialized from the Underground construct's
        // graph-declared capability_gate — data, not native policy.
        let underground: serde_json::Value = serde_json::from_slice(
            &std::fs::read(repo.join("fixtures/ontology/underground-toolkit-v0.json")).unwrap(),
        )
        .unwrap();
        let gate = underground
            .get("members")
            .and_then(|m| m.as_array())
            .unwrap()
            .iter()
            .find(|member| member.get("subtype").and_then(|s| s.as_str()) == Some("code"))
            .and_then(|code| code.pointer("/content/capability_gate"))
            .cloned()
            .expect("underground code member carries a capability_gate");
        let port: SealedCapabilityPort = serde_json::from_value(gate).unwrap();
        assert_eq!(
            port.required_mutate_capability,
            "authority:underground-maintenance"
        );

        // The resolver + real read data, end-to-end (still short of the write
        // path): authority is Admitted; the citizen fails closed, attributably.
        let maintenance_admission = port.admit_mutate_or_deny(&maintenance_set.capabilities);
        assert!(matches!(
            maintenance_admission,
            Ok(MutateAdmission::Admitted { .. })
        ));
        let citizen_admission = port.admit_mutate_or_deny(&citizen_set.capabilities);
        assert!(matches!(
            citizen_admission,
            Err(UniverseError::CapabilityDenied(message))
                if message.contains("authority:underground-maintenance")
        ));
    }

    #[test]
    fn stored_behavior_bond_binding_is_read_as_one_bounded_local_subgraph() {
        let temp = tempfile::tempdir().unwrap();
        let install = create_behavior_bond_authority_store(temp.path()).unwrap();
        let keys = BEHAVIOR_BOND_AUTHORITY_KEYS;
        assert_eq!(install.readback.keys, keys);

        let expected_relation_keys = [
            keys.binding_relations.source_atom,
            keys.binding_relations.target_atom,
            keys.binding_relations.uses_predicate,
            keys.binding_relations.uses_profile,
            keys.binding_relations.has_logic_role,
            keys.binding_relations.gated_by[0],
            keys.binding_relations.gated_by[1],
            keys.binding_relations.serves_objective,
            keys.binding_relations.justified_by,
            keys.binding_relations.applies_in,
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();

        let indexed = universe_store::UniverseStore::open(temp.path())
            .unwrap()
            .load_current_overlay_indexed(universe_store::AdjacencyOverlayBudget::default())
            .unwrap();
        assert_eq!(
            indexed.snapshot().canonical_hash().unwrap(),
            install.readback.snapshot.canonical_hash().unwrap()
        );
        assert_eq!(
            indexed.overlay().base_revision(),
            universe_core::Revision(0)
        );
        assert_eq!(
            indexed.overlay().current_revision(),
            universe_core::Revision(1)
        );
        assert_eq!(indexed.overlay().events_applied(), 1);
        assert_eq!(indexed.overlay().relation_addition_count(), 43);

        let subgraph = read_local_binding_subgraph(
            &indexed,
            QueryOrigin::Entity(keys.behavior_bond),
            QueryBudget {
                max_entities: 16,
                max_relations: 16,
                max_depth: 1,
            },
        );

        assert_eq!(subgraph.situation.status, QueryStatus::Complete);
        assert_eq!(subgraph.situation.visited_entities, 12);
        assert_eq!(subgraph.situation.inspected_relations, 11);
        let actual_relation_keys = subgraph
            .relations
            .iter()
            .map(|relation| relation.key)
            .collect::<BTreeSet<_>>();
        assert!(expected_relation_keys.is_subset(&actual_relation_keys));
        assert!(subgraph.frontier_entities.is_empty());
    }
}
