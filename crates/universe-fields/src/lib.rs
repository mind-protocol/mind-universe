//! Generic bounded ReadField and TopologicalFold lifecycle.

use serde::{Deserialize, Serialize};
use universe_core::{EntityKey, Epistemic};
use universe_physics::{
    PhysicalState, PhysicsBudget, PhysicsCommand, PhysicsEvent, UniversePhysics,
};
use universe_query::{graph_read, LocalGraph, LocalSituation, QueryBudget, QueryOrigin};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Materialization {
    pub entity: EntityKey,
    pub generation: u32,
    pub state: PhysicalState,
}

/// A graph-provided resonance measurement transported without native scoring.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpaceResonance {
    pub space: EntityKey,
    pub score: f64,
    pub metric: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadField {
    pub actor: EntityKey,
    pub origin: QueryOrigin,
    pub budget: QueryBudget,
    /// States and selection are supplied by graph data; this host adds no semantic policy.
    pub materialization: Vec<Materialization>,
    /// Measurements supplied by graph data in canonical order.
    pub resonances: Vec<SpaceResonance>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ReadEvent {
    ReadStarted {
        actor: EntityKey,
    },
    FoldAppeared {
        actor: EntityKey,
        origin: EntityKey,
    },
    SpaceResonanceMeasured {
        origin: EntityKey,
        space: EntityKey,
        score: f64,
        metric: String,
    },
    EntityMaterialized {
        entity: EntityKey,
    },
    ReadStabilized {
        origin: EntityKey,
        state: Epistemic<universe_query::QueryStatus>,
    },
    ReadCompleted {
        situation: LocalSituation,
    },
    ReadReleased {
        actor: EntityKey,
        released: usize,
    },
}

pub struct TopologicalFold {
    actor: EntityKey,
    materialized: Vec<EntityKey>,
    released: bool,
}

impl TopologicalFold {
    pub fn open(
        field: ReadField,
        graph: &impl LocalGraph,
        physics: &mut UniversePhysics,
    ) -> (Self, LocalSituation, Vec<ReadEvent>) {
        let QueryOrigin::Entity(origin) = field.origin;
        let mut events = vec![
            ReadEvent::ReadStarted { actor: field.actor },
            ReadEvent::FoldAppeared {
                actor: field.actor,
                origin,
            },
        ];
        events.extend(field.resonances.iter().map(|measurement| {
            ReadEvent::SpaceResonanceMeasured {
                origin,
                space: measurement.space,
                score: measurement.score,
                metric: measurement.metric.clone(),
            }
        }));
        let commands = field
            .materialization
            .iter()
            .map(|item| PhysicsCommand::Materialize {
                entity: item.entity,
                generation: item.generation,
                state: item.state,
            })
            .chain(std::iter::once(PhysicsCommand::Step))
            .collect();
        let delta = physics.apply(commands);
        let materialized: Vec<_> = delta
            .events
            .iter()
            .filter_map(|event| match event {
                PhysicsEvent::Materialized { entity } => Some(*entity),
                _ => None,
            })
            .collect();
        events.extend(
            materialized
                .iter()
                .map(|entity| ReadEvent::EntityMaterialized { entity: *entity }),
        );
        let situation = graph_read(graph, field.origin, field.budget);
        events.push(ReadEvent::ReadStabilized {
            origin,
            state: Epistemic::Measured(situation.status),
        });
        events.push(ReadEvent::ReadCompleted {
            situation: situation.clone(),
        });
        (
            Self {
                actor: field.actor,
                materialized,
                released: false,
            },
            situation,
            events,
        )
    }

    pub fn release(&mut self, physics: &mut UniversePhysics) -> Vec<ReadEvent> {
        if self.released {
            return Vec::new();
        }
        let delta = physics.apply(
            self.materialized
                .iter()
                .map(|entity| PhysicsCommand::Release { entity: *entity })
                .chain(std::iter::once(PhysicsCommand::Step))
                .collect(),
        );
        let released = delta
            .events
            .iter()
            .filter(|event| matches!(event, PhysicsEvent::Released { .. }))
            .count();
        self.released = true;
        vec![ReadEvent::ReadReleased {
            actor: self.actor,
            released,
        }]
    }
}

pub fn default_test_host(max_active_bodies: usize) -> UniversePhysics {
    UniversePhysics::new(1.0 / 60.0, PhysicsBudget { max_active_bodies })
        .expect("fixed bootstrap timestep is valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_core::{EntityKey, RelationKey};
    use universe_query::{AdjacencyIndex, LocalRelation, QueryStatus};

    #[test]
    fn real_materialize_read_and_release_has_independent_physics_readback() {
        let graph = AdjacencyIndex::from_parts(
            [EntityKey(1), EntityKey(2)],
            [LocalRelation {
                key: RelationKey(1),
                source: EntityKey(1),
                target: EntityKey(2),
            }],
        );
        let field = ReadField {
            actor: EntityKey(1),
            origin: QueryOrigin::Entity(EntityKey(1)),
            budget: QueryBudget {
                max_entities: 2,
                max_relations: 2,
                max_depth: 1,
            },
            materialization: vec![
                Materialization {
                    entity: EntityKey(1),
                    generation: 0,
                    state: PhysicalState {
                        position: [0.0; 3],
                        velocity: [0.0; 3],
                    },
                },
                Materialization {
                    entity: EntityKey(2),
                    generation: 0,
                    state: PhysicalState {
                        position: [1.0, 0.0, 0.0],
                        velocity: [0.0; 3],
                    },
                },
            ],
            resonances: vec![SpaceResonance {
                space: EntityKey(2),
                score: 0.75,
                metric: "fixture_metric".into(),
            }],
        };
        let mut physics = default_test_host(2);
        let (mut fold, situation, open_events) = TopologicalFold::open(field, &graph, &mut physics);
        assert_eq!(situation.status, QueryStatus::Complete);
        assert_eq!(physics.active_count(), 2);
        assert!(open_events
            .iter()
            .any(|event| matches!(event, ReadEvent::ReadCompleted { .. })));
        assert!(open_events.iter().any(|event| matches!(
            event,
            ReadEvent::SpaceResonanceMeasured {
                origin: EntityKey(1),
                space: EntityKey(2),
                score,
                metric,
            } if *score == 0.75 && metric == "fixture_metric"
        )));
        assert!(open_events.iter().any(|event| matches!(
            event,
            ReadEvent::ReadStabilized {
                origin: EntityKey(1),
                state: Epistemic::Measured(QueryStatus::Complete),
            }
        )));
        let release_events = fold.release(&mut physics);
        assert_eq!(physics.active_count(), 0);
        assert_eq!(
            release_events[0],
            ReadEvent::ReadReleased {
                actor: EntityKey(1),
                released: 2,
            }
        );
        assert!(fold.release(&mut physics).is_empty());
    }
}
