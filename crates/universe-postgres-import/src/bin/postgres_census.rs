//! Live, read-only PostgreSQL source census (G2 progressive-delivery phase 1).
//!
//! Connects to the source cluster named by the `MIND_POSTGRES_DSN` environment
//! variable in a **read-only transaction** and computes a bounded `SourceCensus`
//! from real rows. It runs only `SELECT`s, stores no credentials, and writes no
//! row: PostgreSQL remains an import source, never a second live authority.
//!
//! Build/run: `cargo run -p universe-postgres-import --features live-postgres \
//!   --bin postgres_census -- <artifact-dir>`

use postgres::{Client, NoTls};
use serde_json::json;
use std::path::PathBuf;
use universe_postgres_import::SourceCensus;

const RELATION_SAMPLE: i64 = 5_000;
const PROPERTY_SAMPLE: i64 = 5_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let artifact_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: postgres_census <artifact-dir>")?;
    std::fs::create_dir_all(&artifact_dir)?;

    // The DSN carries credentials; it comes from the environment and is never
    // written to the graph, an artifact, or a log.
    let dsn = std::env::var("MIND_POSTGRES_DSN").map_err(|_| "MIND_POSTGRES_DSN is not set")?;
    let mut client = Client::connect(&dsn, NoTls)?;
    // Enforce read-only at the session level, then work inside one read-only,
    // repeatable-read transaction so the census is a consistent snapshot.
    client.batch_execute("SET default_transaction_read_only = on")?;
    let mut tx = client.build_transaction().read_only(true).start()?;

    let count = |tx: &mut postgres::Transaction, sql: &str| -> Result<i64, postgres::Error> {
        Ok(tx.query_one(sql, &[])?.get::<_, i64>(0))
    };

    let node_count = count(&mut tx, "SELECT count(*) FROM mind_nodes")?;
    let relation_count = count(&mut tx, "SELECT count(*) FROM mind_relations")?;
    let metalink_count = count(&mut tx, "SELECT count(*) FROM mind_metalinks")?;
    let moment_count = count(&mut tx, "SELECT count(*) FROM mind_moments")?;
    let claim_count = count(&mut tx, "SELECT count(*) FROM mind_execution_claims")?;
    let graph_count = count(&mut tx, "SELECT count(DISTINCT graph_id) FROM mind_nodes")?;
    let duplicate_ids = count(
        &mut tx,
        "SELECT count(*) FROM (SELECT id FROM mind_nodes GROUP BY id HAVING count(*) > 1) d",
    )?;
    let node_types = count(&mut tx, "SELECT count(DISTINCT node_type) FROM mind_nodes")?;
    let subtypes = count(&mut tx, "SELECT count(DISTINCT subtype) FROM mind_nodes")?;
    let relation_types = count(
        &mut tx,
        "SELECT count(DISTINCT relation_type) FROM mind_relations",
    )?;
    let code_candidates = tx
        .query_one(
            "SELECT count(*) FROM mind_nodes WHERE subtype ILIKE $1 OR node_type ILIKE $1",
            &[&"%code%"],
        )?
        .get::<_, i64>(0);
    let property_keys = count(
        &mut tx,
        "SELECT count(*) FROM (SELECT DISTINCT jsonb_object_keys(properties) k \
         FROM (SELECT properties FROM mind_nodes WHERE properties IS NOT NULL LIMIT 5000) s) d",
    )?;

    // Bounded relation-integrity sample: dangling endpoints and cross-graph edges.
    let sample = tx.query(
        "SELECT source_id, target_id, graph_id FROM mind_relations LIMIT $1",
        &[&RELATION_SAMPLE],
    )?;
    let mut endpoints: Vec<String> = Vec::new();
    for row in &sample {
        endpoints.push(row.get::<_, String>(0));
        endpoints.push(row.get::<_, String>(1));
    }
    let present = tx.query(
        "SELECT id, graph_id FROM mind_nodes WHERE id = ANY($1)",
        &[&endpoints],
    )?;
    let mut graph_of = std::collections::BTreeMap::new();
    for row in &present {
        graph_of.insert(row.get::<_, String>(0), row.get::<_, String>(1));
    }
    let mut dangling = 0i64;
    let mut cross_graph = 0i64;
    for row in &sample {
        let (source, target, graph): (String, String, String) =
            (row.get(0), row.get(1), row.get(2));
        if !graph_of.contains_key(&source) || !graph_of.contains_key(&target) {
            dangling += 1;
        }
        if graph_of.get(&source).is_some_and(|g| g != &graph)
            || graph_of.get(&target).is_some_and(|g| g != &graph)
        {
            cross_graph += 1;
        }
    }

    let version: String = tx.query_one("SELECT version()", &[])?.get(0);
    tx.commit()?; // read-only: releases the snapshot, writes nothing

    let census = SourceCensus {
        node_count: node_count as u64,
        relation_count: relation_count as u64,
        metalink_count: metalink_count as u64,
        moment_count: moment_count as u64,
        execution_claim_count: claim_count as u64,
        graph_count: graph_count as u64,
        duplicate_global_node_ids: duplicate_ids as u64,
        node_type_distinct_count: node_types as u64,
        subtype_distinct_count: subtypes as u64,
        relation_type_distinct_count: relation_types as u64,
        exact_code_candidate_count: code_candidates as u64,
        property_sample_size: PROPERTY_SAMPLE as u64,
        property_key_distinct_sample: property_keys as u64,
        property_code_candidate_sample: 0,
        relation_integrity_status: if (sample.len() as i64) < relation_count {
            "sampled_incomplete".into()
        } else {
            "complete".into()
        },
        relation_sample_size: sample.len() as u64,
        relation_sample_dangling_count: dangling as u64,
        relation_sample_cross_graph_count: cross_graph as u64,
    };

    let host_only = dsn.rsplit('@').next().unwrap_or("unknown");
    let artifact = json!({
        "kind": "postgres_import_source",
        "authority_id": format!("postgres:{host_only}"),
        "source_schema": "public",
        "source_dbms": version.split(',').next().unwrap_or("postgresql"),
        "observed_at": "measured_live",
        "read_only": true,
        "credentials_stored": false,
        "transport": "read_only_transaction_env_dsn",
        "row_hash_contract": "sha256:postgresql-jsonb-text-v0",
        "properties_hash_contract": "sha256:postgresql-jsonb-text-v0",
        "census": census,
    });
    std::fs::write(
        artifact_dir.join("census.json"),
        serde_json::to_vec_pretty(&artifact)?,
    )?;
    println!(
        "live census: nodes={} relations={} metalinks={} graphs={} node_types={} subtypes={} relation_types={} code_candidates={} sample_dangling={} sample_cross_graph={}",
        census.node_count,
        census.relation_count,
        census.metalink_count,
        census.graph_count,
        census.node_type_distinct_count,
        census.subtype_distinct_count,
        census.relation_type_distinct_count,
        census.exact_code_candidate_count,
        census.relation_sample_dangling_count,
        census.relation_sample_cross_graph_count,
    );
    Ok(())
}
