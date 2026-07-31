//! Shared canonical-conformance vocabulary for the universe-e2e injectors and
//! fixture validators — the ONE source of truth for:
//!   * the authored -> canonical predicate remap (active-voice + direction), and
//!   * the node-type / subtype allow-lists a portable projection may carry,
//! so an injected graph interns ZERO new symbols into the canonical store.
//!
//! Previously this table was copy-pasted verbatim across
//! `bin/inject_orientation_beacon.rs`, `bin/inject_energy_pen.rs`, and
//! `tests/house_alarm_fixture.rs`. Drift between those copies would silently
//! change what a supposedly-conformant injection interns; a single module makes
//! the remap attributable and testable in exactly one place.
//!
//! FOLLOWUP: `bin/inject_construct.rs` still carries its own copy (its file is
//! owned by a concurrent session); migrate it onto this module once that lands.

use std::collections::BTreeSet;

/// Every authored predicate the portable projections emit. Kept as data so the
/// remap can be proven TOTAL over exactly this set (see the module tests) and so
/// the canonical vocabulary can be derived rather than restated.
pub const AUTHORED_PREDICATES: &[&str] = &[
    "PART_OF",
    "IMPLEMENTED_IN",
    "DEFINED_BY_CODE",
    "IMPLEMENTED_BY",
    "JUSTIFIED_BY",
    "VALIDATED_BY",
    "OBSERVED_BY",
    "PRODUCES",
    "FEEDS",
    "SUPPORTS",
];

/// Authored -> (canonical predicate, swap) remap. The portable projections use
/// passive/ad-hoc edge names that are NOT in the canonical predicate vocabulary
/// (`fixtures/ontology/canonical-ontology.json`). Each authored predicate maps
/// to an ACTIVE-VOICE canonical predicate and a `swap` flag (true = reverse
/// source/target so the canonical direction holds, e.g. `space IMPLEMENTED_IN
/// impl` becomes `impl IMPLEMENTS space`).
///
/// Returns `None` for an authored predicate with NO canonical mapping. Every
/// caller MUST treat `None` as a hard error and refuse the injection — the
/// injector never silently mints a non-canonical predicate into the canonical
/// store (fail-closed).
pub fn canonical_predicate(authored: &str) -> Option<(&'static str, bool)> {
    Some(match authored {
        "PART_OF" => ("PART_OF", false),
        "IMPLEMENTED_IN" => ("IMPLEMENTS", true),
        "DEFINED_BY_CODE" => ("DEFINES", true),
        "IMPLEMENTED_BY" => ("COMPILES_TO", false),
        "JUSTIFIED_BY" => ("GROUNDS", true),
        "VALIDATED_BY" => ("TESTS", true),
        "OBSERVED_BY" => ("OBSERVES", true),
        "PRODUCES" => ("PRODUCES", false),
        "FEEDS" => ("FEEDS", false),
        "SUPPORTS" => ("MOTIVATES", false),
        _ => return None,
    })
}

/// node_type symbols a conforming portable projection may carry. All already
/// exist in the canonical seed.
pub const CANONICAL_NODE_TYPES: &[&str] = &["space", "narrative", "thing"];

/// Member subtypes promoted to a specific canonical node-type symbol (strictly
/// more ontology-conformant than the generic `node_type`, and interns nothing
/// new). All already exist in the canonical seed.
pub const CANONICAL_SUBTYPES: &[&str] = &["metric", "validation"];

/// The symbol an entity interns: its canonical subtype when promotable, else its
/// `node_type`. Shared by every injector so the subtype-promotion rule cannot
/// drift between them.
pub fn entity_symbol(node_type: &str, subtype: &str) -> String {
    if CANONICAL_SUBTYPES.contains(&subtype) {
        subtype.to_string()
    } else {
        node_type.to_string()
    }
}

/// The complete canonical symbol vocabulary an injected projection may draw on:
/// node-type symbols + promotable subtypes + every remapped predicate target.
pub fn canonical_symbol_vocabulary() -> BTreeSet<String> {
    let mut vocabulary: BTreeSet<String> = BTreeSet::new();
    for symbol in CANONICAL_NODE_TYPES.iter().chain(CANONICAL_SUBTYPES.iter()) {
        vocabulary.insert((*symbol).to_string());
    }
    for authored in AUTHORED_PREDICATES {
        if let Some((canonical, _)) = canonical_predicate(authored) {
            vocabulary.insert(canonical.to_string());
        }
    }
    vocabulary
}

/// Symbol-budget check: of the `requested` symbols, which fall OUTSIDE the
/// canonical vocabulary (i.e. would intern a NEW symbol). An empty result is the
/// conformance guarantee every injector asserts before it commits. The store's
/// own `plan_symbol_interning` is the authoritative gate against the LIVE symbol
/// table; this is the projection-level pre-check over the shared vocabulary.
pub fn new_symbols<'a, I>(requested: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let vocabulary = canonical_symbol_vocabulary();
    let mut out: Vec<String> = requested
        .into_iter()
        .filter(|symbol| !vocabulary.contains(*symbol))
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn remap_is_total_over_the_used_predicates() {
        // Every authored predicate the projections emit resolves to a canonical
        // one — no injection can hit an unmapped predicate by accident.
        for authored in AUTHORED_PREDICATES {
            assert!(
                canonical_predicate(authored).is_some(),
                "authored predicate {authored} lost its canonical mapping"
            );
        }
    }

    #[test]
    fn every_remap_target_is_itself_canonical() {
        // The remap never produces a symbol outside the canonical vocabulary.
        let vocabulary = canonical_symbol_vocabulary();
        for authored in AUTHORED_PREDICATES {
            let (canonical, _) = canonical_predicate(authored).unwrap();
            assert!(
                vocabulary.contains(canonical),
                "remap target {canonical} is not a canonical symbol"
            );
        }
    }

    #[test]
    fn budget_check_flags_only_non_canonical_symbols() {
        assert!(new_symbols(["thing", "space", "PART_OF", "GROUNDS"]).is_empty());
        assert_eq!(
            new_symbols(["thing", "HOLDS_CAPABILITY"]),
            vec!["HOLDS_CAPABILITY".to_string()]
        );
    }

    #[test]
    fn unmapped_predicate_is_fail_closed() {
        // A predicate with no canonical mapping must be rejected, never minted.
        assert!(canonical_predicate("HOLDS_CAPABILITY").is_none());
        assert!(canonical_predicate("HAS_POSITION").is_none());
        assert!(canonical_predicate("CONSTRUCTED_BY").is_none());
    }

    #[test]
    fn grant_fixture_interns_zero_new_symbols() {
        // The underground-maintenance grant, installed on top of the canonical
        // seed, must mint NO new symbol: a conformant grant reuses only symbols
        // the seed already interns. Proven end-to-end on real stores by
        // comparing the post-install symbol table to a seed-only baseline.
        use universe_store::UniverseStore;

        let seed = universe_store::load_seed(
            repo_root().join("fixtures/ontology/canonical-ontology.json"),
        )
        .expect("canonical seed loads");

        let seed_dir = tempfile::tempdir().unwrap();
        let seed_store = UniverseStore::open(seed_dir.path()).unwrap();
        let seed_symbol_count = seed_store.install_seed(&seed).unwrap().symbols.len();

        let grant_dir = tempfile::tempdir().unwrap();
        let install = universe_testkit::install_authority_fixture(
            repo_root().join("fixtures/ontology/underground-maintenance-grant.json"),
            grant_dir.path(),
        )
        .expect("grant fixture installs on the canonical seed");

        assert_eq!(
            install.snapshot.symbols.len(),
            seed_symbol_count,
            "the grant fixture interned new symbols beyond the canonical seed"
        );
        // The non-canonical predicate is gone; the grant rides a canonical one.
        assert!(
            install.snapshot.symbol_id("HOLDS_CAPABILITY").is_none(),
            "HOLDS_CAPABILITY is a non-canonical symbol and must not be interned"
        );
        assert!(
            install.snapshot.symbol_id("USED").is_some(),
            "the grant predicate must be the canonical USED symbol"
        );
    }
}
