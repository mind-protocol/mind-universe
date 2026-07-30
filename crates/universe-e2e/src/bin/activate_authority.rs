//! Generic authority-fixture activator + independent readback receipt.
//!
//! Usage: `activate_authority <fixture.json> <out-store-dir>`
//!
//! Installs a graph-authored `mind-universe-authority-fixture` into a real
//! `UniverseStore`, then INDEPENDENTLY reopens the store from disk in a fresh
//! `UniverseStore`, runs `OntologyRegistry::load`, and prints a readback
//! receipt proving the loop was really written to and activated in the store.
//!
//! This is bootstrap tooling only: activation semantics live entirely in
//! `OntologyRegistry::load`; this bin adds no ontology policy. Epistemic
//! honesty: if the loader rejects the fixture, or a declared member cannot be
//! read back, that is reported as a failure - it is never papered over.

use std::{env, error::Error, process};

use universe_store::ontology::OntologyActivationState;
use universe_testkit::{install_authority_fixture, open_authority_store};

fn main() {
    if let Err(error) = run() {
        eprintln!("ACTIVATION FAILED: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let fixture_path = args
        .next()
        .ok_or("usage: activate_authority <fixture.json> <out-store-dir>")?;
    let store_dir = args
        .next()
        .ok_or("usage: activate_authority <fixture.json> <out-store-dir>")?;

    println!("=== activate_authority ===");
    println!("fixture   : {fixture_path}");
    println!("store dir : {store_dir}");

    // 1. Install into a real store (one additive transaction, tick boundary).
    let install = install_authority_fixture(&fixture_path, &store_dir)?;
    println!("\n-- install receipt --");
    println!("commit receipt        : {:?}", install.receipt);
    println!("installed entities    : {}", install.installed_entities);
    println!("installed relations   : {}", install.installed_relations);
    println!(
        "changeset keys (kind=ontology_changeset): {}",
        format_keys(&install.change_set_keys)
    );

    // 2. INDEPENDENT readback: fresh UniverseStore reopened from disk.
    let (snapshot, registry) = open_authority_store(&store_dir)?;
    println!("\n-- independent readback (fresh store reopened from disk) --");
    println!("total entities in store : {}", snapshot.entities.len());
    println!("total relations in store: {}", snapshot.relations.len());
    println!("universe revision       : {}", snapshot.revision.0);
    println!("activation_state        : {:?}", registry.activation_state);
    println!(
        "active_schema_version   : {}",
        registry.active_schema_version
    );
    println!("authority_hash          : {}", registry.authority_hash);
    println!(
        "overlay members total   : {}",
        registry.overlay_members_by_key.len()
    );

    if registry.activation_state == OntologyActivationState::BaseOnly {
        return Err(
            "loader reports BaseOnly: no ChangeSet was activated (fixture NOT accepted)".into(),
        );
    }
    if registry.active_change_sets.is_empty() {
        return Err("loader activated no ChangeSet despite non-BaseOnly state".into());
    }

    // 3. Per-ChangeSet readback: status + every member kind, read via the
    //    registry's own `active_member` / `overlay_members_by_key`.
    let mut loop_part_count = 0usize;
    let mut learning_count = 0usize;
    for change_set in &registry.active_change_sets {
        println!("\n-- ChangeSet {} --", change_set.key);
        println!("  change_id            : {}", change_set.change_id);
        println!(
            "  base_schema_version  : {}",
            change_set.base_schema_version
        );
        println!(
            "  target_schema_version: {}",
            change_set.target_schema_version
        );
        println!("  content_hash         : {}", change_set.content_hash);
        println!("  member count         : {}", change_set.members.len());
        println!("  members:");
        for member_key in &change_set.members {
            let member = registry.active_member(*member_key).ok_or_else(|| {
                format!(
                    "declared member {member_key} of ChangeSet {} not readable via active_member",
                    change_set.key
                )
            })?;
            let canonical = member.canonical_id.as_deref().unwrap_or("-");
            println!(
                "    {} kind={:<24} canonical_id={:<38} content_sha256={}",
                member.key, member.kind, canonical, member.content_hash
            );
            if member.kind.starts_with("loop_") {
                loop_part_count += 1;
            }
            if member.kind == "session_learning" {
                learning_count += 1;
                let title = member
                    .content
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("(no title)");
                let epistemic = member
                    .content
                    .pointer("/epistemic/state")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("(no epistemic.state)");
                println!("        learning.title    = {title}");
                println!("        learning.epistemic= {epistemic}");
            }
        }
    }

    println!("\n-- summary --");
    println!("loop_* member nodes read back : {loop_part_count}");
    println!("session_learning nodes read back: {learning_count}");
    println!(
        "activation_diagnostics        : {} entries",
        registry.activation_diagnostics.len()
    );
    for diagnostic in &registry.activation_diagnostics {
        println!(
            "    code={} subject={} runtime_blocking={} missing={:?}",
            diagnostic.code, diagnostic.subject, diagnostic.runtime_blocking, diagnostic.missing
        );
    }

    println!("\nRESULT: OntologyRegistry::load ACCEPTED the fixture and the ChangeSet is ACTIVE.");
    println!("        (activation_state above is the loader's own verdict; not_measured health");
    println!(
        "         claims inside member content are graph data, not a runtime health measurement.)"
    );
    Ok(())
}

fn format_keys<T: std::fmt::Display>(keys: &[T]) -> String {
    if keys.is_empty() {
        return "(none)".into();
    }
    keys.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
