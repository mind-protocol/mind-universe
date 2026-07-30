//! Deterministic, bounded physical residency for Universe entities.

use rapier3d::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use universe_core::{EntityKey, HandleKind, PackedHandle, Tick, UniverseError};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Residency {
    Dormant,
    Hot,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhysicalState {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
}

impl PhysicalState {
    fn validate(self) -> Result<(), UniverseError> {
        if self
            .position
            .into_iter()
            .chain(self.velocity)
            .all(f32::is_finite)
        {
            Ok(())
        } else {
            Err(UniverseError::Validation(
                "physical state contains NaN or infinity".into(),
            ))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PhysicsCommand {
    Materialize {
        entity: EntityKey,
        generation: u32,
        state: PhysicalState,
    },
    Release {
        entity: EntityKey,
    },
    Step,
}

impl PhysicsCommand {
    fn sort_key(&self) -> (u8, EntityKey) {
        match self {
            Self::Materialize { entity, .. } => (0, *entity),
            Self::Release { entity } => (1, *entity),
            Self::Step => (2, EntityKey(0)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PhysicsEvent {
    Materialized {
        entity: EntityKey,
    },
    Released {
        entity: EntityKey,
        state: PhysicalState,
    },
    Stepped {
        tick: Tick,
        active_bodies: usize,
    },
    Rejected {
        entity: Option<EntityKey>,
        reason: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PhysicsDelta {
    pub tick: Tick,
    pub events: Vec<PhysicsEvent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsBudget {
    pub max_active_bodies: usize,
}

#[derive(Clone, Copy, Debug)]
struct BodyBinding {
    handle: RigidBodyHandle,
    generation: u32,
}

/// Rapier remains a bounded numerical projection; entity keys remain authoritative.
pub struct UniversePhysics {
    pipeline: PhysicsPipeline,
    gravity: Vector<Real>,
    integration: IntegrationParameters,
    islands: IslandManager,
    broad_phase: BroadPhaseMultiSap,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd: CCDSolver,
    bindings: BTreeMap<EntityKey, BodyBinding>,
    dormant: BTreeMap<EntityKey, PhysicalState>,
    tick: Tick,
    budget: PhysicsBudget,
}

impl UniversePhysics {
    pub fn new(fixed_dt: f32, budget: PhysicsBudget) -> Result<Self, UniverseError> {
        if !fixed_dt.is_finite() || fixed_dt <= 0.0 || budget.max_active_bodies == 0 {
            return Err(UniverseError::Validation(
                "fixed_dt and active-body budget must be positive".into(),
            ));
        }
        let integration = IntegrationParameters {
            dt: fixed_dt,
            ..IntegrationParameters::default()
        };
        Ok(Self {
            pipeline: PhysicsPipeline::new(),
            gravity: vector![0.0, 0.0, 0.0],
            integration,
            islands: IslandManager::new(),
            broad_phase: BroadPhaseMultiSap::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd: CCDSolver::new(),
            bindings: BTreeMap::new(),
            dormant: BTreeMap::new(),
            tick: Tick(0),
            budget,
        })
    }

    pub fn apply(&mut self, mut commands: Vec<PhysicsCommand>) -> PhysicsDelta {
        commands.sort_by_key(PhysicsCommand::sort_key);
        let mut events = Vec::new();
        for command in commands {
            match command {
                PhysicsCommand::Materialize {
                    entity,
                    generation,
                    state,
                } => match self.materialize(entity, generation, state) {
                    Ok(true) => events.push(PhysicsEvent::Materialized { entity }),
                    Ok(false) => {}
                    Err(error) => events.push(PhysicsEvent::Rejected {
                        entity: Some(entity),
                        reason: error.to_string(),
                    }),
                },
                PhysicsCommand::Release { entity } => match self.release(entity) {
                    Ok(Some(state)) => events.push(PhysicsEvent::Released { entity, state }),
                    Ok(None) => {}
                    Err(error) => events.push(PhysicsEvent::Rejected {
                        entity: Some(entity),
                        reason: error.to_string(),
                    }),
                },
                PhysicsCommand::Step => {
                    self.pipeline.step(
                        &self.gravity,
                        &self.integration,
                        &mut self.islands,
                        &mut self.broad_phase,
                        &mut self.narrow_phase,
                        &mut self.bodies,
                        &mut self.colliders,
                        &mut self.impulse_joints,
                        &mut self.multibody_joints,
                        &mut self.ccd,
                        None,
                        &(),
                        &(),
                    );
                    self.tick.0 += 1;
                    events.push(PhysicsEvent::Stepped {
                        tick: self.tick,
                        active_bodies: self.bindings.len(),
                    });
                }
            }
        }
        PhysicsDelta {
            tick: self.tick,
            events,
        }
    }

    fn materialize(
        &mut self,
        entity: EntityKey,
        generation: u32,
        state: PhysicalState,
    ) -> Result<bool, UniverseError> {
        state.validate()?;
        if self.bindings.contains_key(&entity) {
            return Ok(false);
        }
        if self.bindings.len() >= self.budget.max_active_bodies {
            return Err(UniverseError::BudgetExhausted("active body budget".into()));
        }
        let slot = u64::try_from(self.bodies.len())
            .map_err(|_| UniverseError::InvalidHandle("body slot overflow".into()))?;
        let user_data = PackedHandle {
            kind: HandleKind::Entity,
            generation,
            slot,
        }
        .pack()?;
        let body = RigidBodyBuilder::dynamic()
            .translation(vector![
                state.position[0],
                state.position[1],
                state.position[2]
            ])
            .linvel(vector![
                state.velocity[0],
                state.velocity[1],
                state.velocity[2]
            ])
            .user_data(user_data)
            .build();
        let handle = self.bodies.insert(body);
        self.bindings
            .insert(entity, BodyBinding { handle, generation });
        self.dormant.remove(&entity);
        Ok(true)
    }

    fn release(&mut self, entity: EntityKey) -> Result<Option<PhysicalState>, UniverseError> {
        let Some(binding) = self.bindings.remove(&entity) else {
            return Ok(None);
        };
        let body = self
            .bodies
            .get(binding.handle)
            .ok_or(UniverseError::StaleHandle)?;
        let position = body.translation();
        let velocity = body.linvel();
        let state = PhysicalState {
            position: [position.x, position.y, position.z],
            velocity: [velocity.x, velocity.y, velocity.z],
        };
        state.validate()?;
        self.bodies.remove(
            binding.handle,
            &mut self.islands,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            true,
        );
        self.dormant.insert(entity, state);
        Ok(Some(state))
    }

    pub fn residency(&self, entity: EntityKey) -> Residency {
        if self.bindings.contains_key(&entity) {
            Residency::Hot
        } else {
            Residency::Dormant
        }
    }

    pub fn active_entities(&self) -> Vec<EntityKey> {
        self.bindings.keys().copied().collect()
    }

    pub fn active_count(&self) -> usize {
        self.bindings.len()
    }

    pub fn generation(&self, entity: EntityKey) -> Option<u32> {
        self.bindings.get(&entity).map(|binding| binding.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(x: f32) -> PhysicalState {
        PhysicalState {
            position: [x, 0.0, 0.0],
            velocity: [0.0; 3],
        }
    }

    #[test]
    fn commands_are_deterministic_bounded_and_release_to_dormant() {
        let mut physics = UniversePhysics::new(
            1.0 / 60.0,
            PhysicsBudget {
                max_active_bodies: 2,
            },
        )
        .unwrap();
        let delta = physics.apply(vec![
            PhysicsCommand::Materialize {
                entity: EntityKey(2),
                generation: 0,
                state: state(2.0),
            },
            PhysicsCommand::Materialize {
                entity: EntityKey(1),
                generation: 0,
                state: state(1.0),
            },
            PhysicsCommand::Materialize {
                entity: EntityKey(3),
                generation: 0,
                state: state(3.0),
            },
            PhysicsCommand::Step,
        ]);
        assert_eq!(physics.active_entities(), vec![EntityKey(1), EntityKey(2)]);
        assert!(matches!(
            delta.events[2],
            PhysicsEvent::Rejected {
                entity: Some(EntityKey(3)),
                ..
            }
        ));
        physics.apply(vec![PhysicsCommand::Release {
            entity: EntityKey(1),
        }]);
        assert_eq!(physics.residency(EntityKey(1)), Residency::Dormant);
        assert_eq!(physics.active_count(), 1);
    }

    #[test]
    fn invalid_state_is_local_rejection() {
        let mut physics = UniversePhysics::new(
            1.0 / 60.0,
            PhysicsBudget {
                max_active_bodies: 1,
            },
        )
        .unwrap();
        let delta = physics.apply(vec![PhysicsCommand::Materialize {
            entity: EntityKey(1),
            generation: 0,
            state: state(f32::NAN),
        }]);
        assert!(matches!(delta.events[0], PhysicsEvent::Rejected { .. }));
        assert_eq!(physics.active_count(), 0);
    }
}
