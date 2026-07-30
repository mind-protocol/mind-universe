//! Versioned, bounded headless protocol contracts.

use serde::{Deserialize, Serialize};
use universe_core::{EntityKey, Epistemic, Revision, Tick, UniverseError};
use universe_store::EntityRecord;
use universe_supervisor::{RuntimeInventory, Supervisor};

pub const PROTOCOL_VERSION: u16 = 0;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrelationId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReadEntityRequest {
    pub protocol_version: u16,
    pub correlation: CorrelationId,
    pub key: EntityKey,
    pub max_entities: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReadEntityResponse {
    pub correlation: CorrelationId,
    pub revision: Revision,
    pub tick: Tick,
    pub result: Epistemic<EntityRecord>,
}

pub struct HeadlessProtocol<'a> {
    supervisor: &'a Supervisor,
}

impl<'a> HeadlessProtocol<'a> {
    pub fn new(supervisor: &'a Supervisor) -> Self {
        Self { supervisor }
    }

    /// Opens a fresh store replay through the supervisor; it never serves the
    /// commit receipt or the supervisor's in-memory projection as readback.
    pub fn read_entity(
        &self,
        request: ReadEntityRequest,
    ) -> Result<ReadEntityResponse, UniverseError> {
        if request.protocol_version != PROTOCOL_VERSION {
            return Err(UniverseError::UnsupportedVersion(request.protocol_version));
        }
        if request.max_entities == 0 {
            return Err(UniverseError::BudgetExhausted(
                "max_entities must be non-zero".into(),
            ));
        }
        let snapshot = self.supervisor.independent_readback()?;
        let result = snapshot
            .entities
            .iter()
            .find(|entity| entity.key == request.key)
            .cloned()
            .map(Epistemic::Observed)
            .unwrap_or(Epistemic::KnownAbsent);
        Ok(ReadEntityResponse {
            correlation: request.correlation,
            revision: snapshot.revision,
            tick: snapshot.tick,
            result,
        })
    }

    pub fn runtime_inventory(&self) -> RuntimeInventory {
        self.supervisor.runtime_inventory()
    }
}
