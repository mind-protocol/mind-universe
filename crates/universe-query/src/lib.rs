//! Bounded local graph reads over an explicitly supplied adjacency frontier.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use universe_core::{EntityKey, RelationKey};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalRelation {
    pub key: RelationKey,
    pub source: EntityKey,
    pub target: EntityKey,
}

/// A truth-layer view must provide direct local adjacency, never a global iterator.
pub trait LocalGraph {
    fn contains(&self, entity: EntityKey) -> bool;
    fn adjacent(&self, entity: EntityKey) -> &[LocalRelation];
}

pub fn graph_read(
    graph: &impl LocalGraph,
    origin: QueryOrigin,
    budget: QueryBudget,
) -> LocalSituation {
    let QueryOrigin::Entity(origin) = origin;
    if !graph.contains(origin) {
        return LocalSituation {
            entities: Vec::new(),
            relations: Vec::new(),
            status: QueryStatus::UnknownOrigin,
            visited_entities: 0,
            inspected_relations: 0,
        };
    }
    let mut queue = VecDeque::from([(origin, 0usize)]);
    let mut visited = BTreeSet::new();
    let mut relations = BTreeSet::new();
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
            if inspected >= budget.max_relations {
                budget_hit = true;
                break;
            }
            inspected += 1;
            relations.insert(relation.key);
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
    LocalSituation {
        entities: visited.iter().copied().collect(),
        relations: relations.iter().copied().collect(),
        status,
        visited_entities: visited.len(),
        inspected_relations: inspected,
    }
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
    fn contains(&self, entity: EntityKey) -> bool {
        self.entities.contains(&entity)
    }

    fn adjacent(&self, entity: EntityKey) -> &[LocalRelation] {
        self.adjacency
            .get(&entity)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_testkit::minimal_snapshot;

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
}
