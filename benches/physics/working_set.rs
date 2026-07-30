//! Standalone benchmark seed for later p50/p95/p99 harness integration.
//! Run the crate tests for the currently evidenced deterministic working-set proof.

use universe_core::EntityKey;
use universe_physics::{PhysicalState, PhysicsBudget, PhysicsCommand, UniversePhysics};

fn main() {
    let mut physics =
        UniversePhysics::new(1.0 / 60.0, PhysicsBudget { max_active_bodies: 10_000 }).unwrap();
    let commands = (0..10_000)
        .map(|slot| PhysicsCommand::Materialize {
            entity: EntityKey(slot + 1),
            generation: 0,
            state: PhysicalState {
                position: [slot as f32, 0.0, 0.0],
                velocity: [0.0; 3],
            },
        })
        .collect();
    let started = std::time::Instant::now();
    physics.apply(commands);
    println!("active={} elapsed={:?}", physics.active_count(), started.elapsed());
}
