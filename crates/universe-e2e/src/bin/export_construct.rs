//! Export construct(s) from the LIVE store to `exports/` as JSON — READ-ONLY.
//!
//! Usage:
//!   export_construct <construct-root-canonical-id> [store-dir] [exports-dir]
//!   export_construct --all                         [store-dir] [exports-dir]
//!   export_construct --list                        [store-dir]
//!
//! Defaults: store-dir `artifacts/ontology-registry/current/store`,
//!           exports-dir `exports`.
//!
//! The Universe is never written: the bin opens the store, replays the valid
//! event log, hydrates content and projects it. The only write is the export
//! file, and each one is read back from disk and re-hashed before it is
//! reported — a receipt here means the bytes were observed, not assumed.

use std::{env, path::PathBuf, process};

use universe_e2e::construct_export::{
    export_all_constructs, export_construct, list_construct_roots, ExportReceipt,
    DEFAULT_EXPORTS_DIR, DEFAULT_STORE_DIR,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("EXPORT_CONSTRUCT FAILED: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let target = args.first().ok_or(
        "usage: export_construct <construct-root-canonical-id> | --all | --list [store-dir] [exports-dir]",
    )?;
    let store_dir = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_STORE_DIR));
    let exports_dir = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_EXPORTS_DIR));

    match target.as_str() {
        "--list" => {
            let roots = list_construct_roots(&store_dir).map_err(|e| format!("{e:?}"))?;
            println!("CONSTRUCT ROOTS   store: {}", store_dir.display());
            if roots.is_empty() {
                println!("  (none in this snapshot — `unknown`, not `known_absent`)");
            }
            for root in &roots {
                println!("  {root}");
            }
        }
        "--all" => {
            let receipts =
                export_all_constructs(&store_dir, &exports_dir).map_err(|e| format!("{e:?}"))?;
            println!(
                "EXPORTED {} construct(s)   store: {}   -> {}",
                receipts.len(),
                store_dir.display(),
                exports_dir.display()
            );
            for receipt in &receipts {
                print_receipt(receipt);
            }
        }
        root_id => {
            let receipt =
                export_construct(&store_dir, root_id, &exports_dir).map_err(|e| format!("{e:?}"))?;
            println!(
                "EXPORTED 1 construct   store: {}   -> {}",
                store_dir.display(),
                exports_dir.display()
            );
            print_receipt(&receipt);
        }
    }

    println!("END — READ-ONLY on the Universe: no event appended, no transaction committed.");
    Ok(())
}

fn print_receipt(receipt: &ExportReceipt) {
    println!("  {}", receipt.root_id);
    println!("    name        {}", receipt.name);
    println!("    file        {}", receipt.path.display());
    println!(
        "    revision    {}   members {}   internal edges {}   boundary edges {}",
        receipt.store_revision,
        receipt.member_count,
        receipt.internal_relation_count,
        receipt.boundary_relation_count
    );
    println!(
        "    bytes       {}   sha256 {}",
        receipt.bytes_written, receipt.sha256
    );
    println!(
        "    readback    {}   observation status: {}",
        if receipt.readback_verified {
            "verified (re-read from disk, hash matched, re-parsed)"
        } else {
            "NOT VERIFIED — the bytes on disk did not reproduce the hash"
        },
        receipt.observation_status
    );
}
