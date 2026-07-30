//! Versioned primitive contracts shared by every bootstrap component.

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

pub const CONTRACT_VERSION: u16 = 0;

macro_rules! id128 {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            Eq,
            Hash,
            Ord,
            PartialEq,
            PartialOrd,
            Serialize,
            Deserialize,
        )]
        pub struct $name(#[serde(with = "u128_hex")] pub u128);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{:032x}", self.0)
            }
        }
    };
}

mod u128_hex {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{value:032x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.len() != 32 {
            return Err(D::Error::custom(
                "u128 ID must contain exactly 32 hex digits",
            ));
        }
        u128::from_str_radix(&value, 16).map_err(D::Error::custom)
    }
}

id128!(UniverseId);
id128!(EntityKey);
id128!(RelationKey);
id128!(ChunkId);

#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Tick(pub u64);

#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Revision(pub u64);

/// Stable packed handle used in solver `user_data`.
///
/// Layout, high to low: kind:8, generation:32, slot:56, reserved:32.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct PackedHandle {
    pub kind: HandleKind,
    pub generation: u32,
    pub slot: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum HandleKind {
    Entity = 1,
    Relation = 2,
    Runtime = 3,
}

impl PackedHandle {
    pub const MAX_SLOT: u64 = (1u64 << 56) - 1;

    pub fn pack(self) -> Result<u128, UniverseError> {
        if self.slot > Self::MAX_SLOT {
            return Err(UniverseError::InvalidHandle("slot exceeds 56 bits".into()));
        }
        Ok(((self.kind as u128) << 120)
            | ((self.generation as u128) << 88)
            | ((self.slot as u128) << 32))
    }

    pub fn unpack(value: u128) -> Result<Self, UniverseError> {
        if value & 0xffff_ffff != 0 {
            return Err(UniverseError::InvalidHandle(
                "reserved handle bits are non-zero".into(),
            ));
        }
        let kind = match (value >> 120) as u8 {
            1 => HandleKind::Entity,
            2 => HandleKind::Relation,
            3 => HandleKind::Runtime,
            _ => return Err(UniverseError::InvalidHandle("unknown handle kind".into())),
        };
        Ok(Self {
            kind,
            generation: ((value >> 88) & 0xffff_ffff) as u32,
            slot: ((value >> 32) & (Self::MAX_SLOT as u128)) as u64,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ContentPtr {
    pub segment: u64,
    pub offset: u64,
    pub length: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum Epistemic<T> {
    Observed(T),
    Measured(T),
    KnownAbsent,
    Unknown,
    NotMeasured,
    MeasurementFailed { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VersionEnvelope<T> {
    pub contract_version: u16,
    pub universe: UniverseId,
    pub revision: Revision,
    pub tick: Tick,
    pub payload: T,
}

impl<T> VersionEnvelope<T> {
    pub fn v0(universe: UniverseId, revision: Revision, tick: Tick, payload: T) -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            universe,
            revision,
            tick,
            payload,
        }
    }

    pub fn validate_version(&self) -> Result<(), UniverseError> {
        if self.contract_version == CONTRACT_VERSION {
            Ok(())
        } else {
            Err(UniverseError::UnsupportedVersion(self.contract_version))
        }
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq, Serialize, Deserialize)]
pub enum UniverseError {
    #[error("unsupported contract version {0}")]
    UnsupportedVersion(u16),
    #[error("invalid stable handle: {0}")]
    InvalidHandle(String),
    #[error("stale generational handle")]
    StaleHandle,
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("revision conflict: expected {expected:?}, actual {actual:?}")]
    RevisionConflict {
        expected: Revision,
        actual: Revision,
    },
    #[error("content is corrupt: {0}")]
    CorruptContent(String),
    #[error("event log is corrupt: {0}")]
    CorruptLog(String),
    #[error("I/O failed: {0}")]
    Io(String),
    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),
    #[error("operation was cancelled")]
    Cancelled,
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_handle_round_trip_is_stable() {
        let handle = PackedHandle {
            kind: HandleKind::Entity,
            generation: 42,
            slot: PackedHandle::MAX_SLOT,
        };
        assert_eq!(
            PackedHandle::unpack(handle.pack().unwrap()).unwrap(),
            handle
        );
    }

    #[test]
    fn reserved_bits_are_rejected() {
        assert!(PackedHandle::unpack(1).is_err());
    }
}
