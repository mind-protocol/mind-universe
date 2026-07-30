use std::path::PathBuf;
use universe_postgres_import::{load_manifest, run_identity_pilot};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let manifest = args
        .next()
        .map(PathBuf::from)
        .expect("usage: universe-postgres-import <manifest.json> <store-directory>");
    let output = args
        .next()
        .map(PathBuf::from)
        .expect("usage: universe-postgres-import <manifest.json> <store-directory>");
    assert!(args.next().is_none(), "unexpected extra arguments");
    let manifest = load_manifest(manifest).expect("load import manifest");
    let evidence = run_identity_pilot(&manifest, output).expect("run identity pilot");
    println!(
        "{}",
        serde_json::to_string_pretty(&evidence).expect("serialize pilot evidence")
    );
}
