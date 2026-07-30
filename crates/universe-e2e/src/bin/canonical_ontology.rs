use std::{
    env,
    error::Error,
    fs,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};
use universe_store::{
    load_seed,
    ontology::{OntologyLoadBudget, OntologyRegistry},
    UniverseStore,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let artifact_root = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after Unix epoch")
            .as_nanos();
        repository
            .join("artifacts/ontology-registry")
            .join(format!("run-{}-{nonce}", process::id()))
    });
    fs::create_dir_all(&artifact_root)?;

    let seed_path = repository.join("fixtures/ontology/canonical-ontology.json");
    let store_root = artifact_root.join("store");
    let seed = load_seed(&seed_path)?;
    let store = UniverseStore::open(&store_root)?;
    let installed = store.install_seed(&seed)?;

    let independent_store = UniverseStore::open(&store_root)?;
    let independent = independent_store.load_snapshot()?;
    let installed_hash = installed.canonical_hash()?;
    let independent_hash = independent.canonical_hash()?;
    let store_readback_observed = installed == independent && installed_hash == independent_hash;
    let verified_content_records = verify_all_content(&independent_store, &independent)?;
    let registry = OntologyRegistry::load(
        &independent_store,
        &independent,
        OntologyLoadBudget::default(),
    )?;

    let status = if store_readback_observed
        && verified_content_records == independent.entities.len() + independent.relations.len()
        && registry.status == "reconstructed_with_explicit_gaps"
    {
        "validated_with_explicit_gaps"
    } else {
        "not_validated"
    };
    let gaps: Vec<_> = registry
        .gaps
        .values()
        .map(|gap| {
            serde_json::json!({
                "id": gap.canonical_id,
                "subject": gap.subject,
                "missing": gap.missing,
                "status": gap.status,
            })
        })
        .collect();
    let receipt = serde_json::json!({
        "kind": "canonical_ontology_reconstruction_receipt",
        "status": status,
        "seed_path": seed_path,
        "store_root": store_root,
        "store_readback_observed": store_readback_observed,
        "snapshot_hash": independent_hash,
        "verified_content_records": verified_content_records,
        "ontology_id": registry.ontology_id,
        "schema_version": registry.schema_version,
        "mapping_version": registry.mapping_version,
        "counts": registry.counts,
        "source_hashes": registry.source_hashes,
        "known_gaps": registry.known_gaps,
        "gaps": gaps,
        "compatibility_predicates": registry
            .compatibility_predicates
            .keys()
            .collect::<Vec<_>>(),
    });
    let receipt_path = artifact_root.join("ontology-reconstruction-receipt.json");
    fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt)?)?;

    println!("{}", receipt_path.display());
    println!("ontology={status}");
    println!(
        "counts={}/{}",
        independent.entities.len(),
        independent.relations.len()
    );
    println!(
        "gaps={}",
        registry
            .known_gaps
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
    );
    Ok(())
}

fn verify_all_content(
    store: &UniverseStore,
    snapshot: &universe_store::UniverseSnapshot,
) -> Result<usize, Box<dyn Error>> {
    let contents = snapshot
        .entities
        .iter()
        .filter_map(|entity| entity.content.as_ref())
        .chain(
            snapshot
                .relations
                .iter()
                .filter_map(|relation| relation.content.as_ref()),
        );
    let mut count = 0;
    for content in contents {
        store.read_content(content)?;
        count += 1;
    }
    Ok(count)
}
