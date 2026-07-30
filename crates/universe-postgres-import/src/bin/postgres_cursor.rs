use std::path::PathBuf;
use universe_postgres_import::cursor::{
    apply_cursor_batch, bootstrap_cursor_store, inspect_cursor_store, load_cursor_manifest,
};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let operation = args
        .next()
        .and_then(|value| value.into_string().ok())
        .expect(
        "usage: postgres_cursor <bootstrap|apply|inspect> <manifest.json> <store> [batch-index]",
    );
    let manifest_path = args
        .next()
        .map(PathBuf::from)
        .expect("cursor manifest path is required");
    let store = args
        .next()
        .map(PathBuf::from)
        .expect("cursor store path is required");
    let manifest = load_cursor_manifest(manifest_path).expect("load cursor manifest");
    let evidence = match operation.as_str() {
        "bootstrap" => bootstrap_cursor_store(&manifest, store),
        "apply" => {
            let index = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| value.parse::<usize>().ok())
                .expect("apply requires a numeric batch index");
            apply_cursor_batch(&manifest, store, index)
        }
        "inspect" => inspect_cursor_store(&manifest, store),
        _ => panic!("unknown cursor operation {operation}"),
    }
    .expect("execute cursor operation");
    assert!(args.next().is_none(), "unexpected extra arguments");
    println!(
        "{}",
        serde_json::to_string_pretty(&evidence).expect("serialize cursor evidence")
    );
}
