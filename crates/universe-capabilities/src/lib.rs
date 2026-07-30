//! Generic effect authorization and measured transport receipts.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use universe_core::{Tick, UniverseError};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EffectIntent {
    pub capability: String,
    pub idempotency_key: String,
    pub payload: Vec<u8>,
    pub deadline_tick: Tick,
    pub causal_ancestry: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EffectReceipt {
    TransportSucceeded { response: Vec<u8> },
    TransportFailed { reason: String },
}

pub trait EffectAdapter {
    fn transport(&mut self, payload: &[u8]) -> Result<Vec<u8>, String>;
}

#[derive(Default)]
pub struct CapabilityHost {
    adapters: BTreeMap<String, Box<dyn EffectAdapter>>,
    completed: BTreeSet<String>,
}

impl CapabilityHost {
    pub fn register(&mut self, name: impl Into<String>, adapter: Box<dyn EffectAdapter>) {
        self.adapters.insert(name.into(), adapter);
    }

    pub fn execute(
        &mut self,
        now: Tick,
        intent: &EffectIntent,
    ) -> Result<EffectReceipt, UniverseError> {
        if now > intent.deadline_tick {
            return Ok(EffectReceipt::TransportFailed {
                reason: "deadline exceeded before transport".into(),
            });
        }
        if self.completed.contains(&intent.idempotency_key) {
            return Err(UniverseError::Validation(
                "effect idempotency key already completed".into(),
            ));
        }
        let adapter = self
            .adapters
            .get_mut(&intent.capability)
            .ok_or_else(|| UniverseError::CapabilityDenied(intent.capability.clone()))?;
        let receipt = match adapter.transport(&intent.payload) {
            Ok(response) => EffectReceipt::TransportSucceeded { response },
            Err(reason) => EffectReceipt::TransportFailed { reason },
        };
        self.completed.insert(intent.idempotency_key.clone());
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    impl EffectAdapter for Echo {
        fn transport(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
            Ok(payload.to_vec())
        }
    }

    #[test]
    fn receipt_contains_actual_adapter_result() {
        let mut host = CapabilityHost::default();
        host.register("safe.echo", Box::new(Echo));
        let receipt = host
            .execute(
                Tick(1),
                &EffectIntent {
                    capability: "safe.echo".into(),
                    idempotency_key: "effect-1".into(),
                    payload: b"observed".to_vec(),
                    deadline_tick: Tick(2),
                    causal_ancestry: vec![],
                },
            )
            .unwrap();
        assert_eq!(
            receipt,
            EffectReceipt::TransportSucceeded {
                response: b"observed".to_vec()
            }
        );
    }
}
