use std::{
    env,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};
use universe_e2e::covalidity::{default_config, run};

fn main() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let artifact_root = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after Unix epoch")
            .as_nanos();
        repository
            .join("artifacts/ontology-physics")
            .join(format!("run-{}-{nonce}", process::id()))
    });
    match run(&default_config(&repository, artifact_root)) {
        Ok(manifest) => {
            println!("{}", manifest.manifest_path.display());
            println!("ontology_physics={}", manifest.receipt.status);
            println!("loop_loop={}", manifest.loop_loop.receipt.status);
        }
        Err(error) => {
            eprintln!("{error:?}");
            process::exit(1);
        }
    }
}
