use std::{env, path::PathBuf};
use universe_e2e::{run, RunConfig};

fn main() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| repository.join("artifacts/verification"));
    let store = output.join("store");
    match run(&RunConfig {
        genesis_path: repository.join("fixtures/genesis/minimal-genesis.json"),
        code_path: repository.join("fixtures/graph-ir/minimal-read.json"),
        store_root: store,
        artifact_root: output,
    }) {
        Ok(manifest) => println!("{}", manifest.correlation.0),
        Err(error) => {
            eprintln!("{error:?}");
            std::process::exit(1);
        }
    }
}
