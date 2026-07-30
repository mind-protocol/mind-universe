//! Deterministic generic fixtures shared by integration tests.

use std::path::Path;
use universe_core::UniverseId;
use universe_store::{load_genesis, UniverseSnapshot};

pub const MINIMAL_UNIVERSE_ID: UniverseId = UniverseId(1);

pub fn minimal_snapshot() -> UniverseSnapshot {
    load_genesis(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/genesis/minimal-genesis.json"),
    )
    .expect("repository Genesis fixture must remain valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_is_deterministic_and_valid() {
        let a = minimal_snapshot();
        let b = minimal_snapshot();
        assert_eq!(a.canonical_hash().unwrap(), b.canonical_hash().unwrap());
        a.validate().unwrap();
        assert_eq!(a.symbols[a.entities[0].symbol as usize], "Actor");
        assert!(a.relations.iter().any(|relation| {
            a.symbols[relation.predicate as usize] == "result_type"
                && a.symbols[a
                    .entities
                    .iter()
                    .find(|entity| entity.key == relation.target)
                    .unwrap()
                    .symbol as usize]
                    == "Moment"
        }));
    }
}
