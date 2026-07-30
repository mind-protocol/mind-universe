//! Graph-owned, bounded, resumable PostgreSQL source cursor.
//!
//! PostgreSQL transport is read-only and produces a manifest of measured row
//! metadata. This module owns only the generic bootstrap mechanics required to
//! validate, commit, replay, and resume that graph data.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};
use universe_core::{EntityKey, RelationKey, Revision, Tick, UniverseError, UniverseId};
use universe_store::{
    canonical_hash, EntityRecord, GraphSeed, RelationRecord, SeedEntity, SeedRelation,
    UniverseSnapshot, UniverseStore,
};
use universe_transactions::{
    CommitReceipt, UniverseCommand, UniverseTransaction, UniverseWriteSet,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CursorVocabulary {
    pub source: String,
    pub contract: String,
    pub cursor_state: String,
    pub batch: String,
    pub source_record: String,
    pub asset: String,
    pub receipt: String,
    pub conflict: String,
    pub governed_by: String,
    pub observed_from: String,
    pub proposes_cursor: String,
    pub advances_to: String,
    pub in_batch: String,
    pub maps_to: String,
    pub has_receipt: String,
    pub confirms_cursor: String,
    pub has_conflict: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CursorBudgets {
    pub batch_limit: usize,
    pub statement_timeout_ms: u64,
    pub transaction_timeout_ms: u64,
    pub max_retries: u32,
    pub retry_backoff_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CursorAuthority {
    pub atom: EntityKey,
    pub authority_id: String,
    pub source_schema: String,
    pub source_graph: String,
    pub server_version_num: String,
    pub source_schema_revision_sha256: String,
    pub adapter_revision: String,
    pub adapter_revision_sha256: String,
    pub mapping_revision: String,
    pub mapping_revision_sha256: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct CursorWatermark {
    pub updated_at: String,
    pub source_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CursorSourceRecord {
    pub candidate_atom: EntityKey,
    pub asset_atom: EntityKey,
    pub graph_id: String,
    pub source_id: String,
    pub node_type: String,
    pub subtype: String,
    pub source_status: Option<String>,
    pub source_revision: u64,
    pub updated_at: String,
    pub row_sha256: String,
    pub properties_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CursorBatch {
    pub atom: EntityKey,
    pub prepared_cursor_atom: EntityKey,
    pub receipt_atom: EntityKey,
    pub next_cursor_atom: EntityKey,
    pub conflict_atom: EntityKey,
    pub relation_key_start: RelationKey,
    pub conflict_relation: RelationKey,
    pub batch_id: String,
    pub index: usize,
    pub observed_at: String,
    pub source_snapshot: String,
    pub source_attempts: u32,
    pub has_more: bool,
    pub source_schema_revision_sha256: String,
    pub adapter_revision_sha256: String,
    pub mapping_revision_sha256: String,
    pub prior_watermark: CursorWatermark,
    pub next_watermark: CursorWatermark,
    pub records: Vec<CursorSourceRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CursorManifest {
    pub contract_version: u16,
    pub universe: UniverseId,
    pub contract_atom: EntityKey,
    pub initial_cursor_atom: EntityKey,
    pub relation_key_start: RelationKey,
    pub vocabulary: CursorVocabulary,
    pub authority: CursorAuthority,
    pub budgets: CursorBudgets,
    pub initial_watermark: CursorWatermark,
    pub batches: Vec<CursorBatch>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorApplyStatus {
    Committed,
    ResumedPrepared,
    AlreadyCommitted,
    ConflictRecorded,
    ConflictAlreadyRecorded,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CursorEvidence {
    pub status: CursorApplyStatus,
    pub batch_id: Option<String>,
    pub cursor_atom: EntityKey,
    pub watermark: CursorWatermark,
    pub cursor_advanced: bool,
    pub imported_records: usize,
    pub executable_records: usize,
    pub ontology_activated: bool,
    pub revision: Revision,
    pub tick: Tick,
    pub snapshot_sha256: String,
    pub content_records_read_back: usize,
}

pub fn load_cursor_manifest(path: impl AsRef<Path>) -> Result<CursorManifest, UniverseError> {
    let bytes = std::fs::read(path).map_err(|error| UniverseError::Io(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| UniverseError::CorruptContent(error.to_string()))
}

pub fn validate_cursor_manifest(manifest: &CursorManifest) -> Result<(), UniverseError> {
    if manifest.contract_version != 0 {
        return Err(UniverseError::UnsupportedVersion(manifest.contract_version));
    }
    if !manifest.authority.read_only
        || manifest.authority.authority_id.trim().is_empty()
        || manifest.authority.source_schema.trim().is_empty()
        || manifest.authority.source_graph.trim().is_empty()
    {
        return Err(validation(
            "cursor source authority must be named and strictly read-only",
        ));
    }
    if manifest.budgets.batch_limit == 0
        || manifest.budgets.statement_timeout_ms == 0
        || manifest.budgets.transaction_timeout_ms < manifest.budgets.statement_timeout_ms
        || manifest.budgets.max_retries > 16
        || manifest.budgets.retry_backoff_ms == 0
    {
        return Err(validation("cursor budgets are invalid or unbounded"));
    }
    for hash in [
        &manifest.authority.source_schema_revision_sha256,
        &manifest.authority.adapter_revision_sha256,
        &manifest.authority.mapping_revision_sha256,
    ] {
        if !valid_hash(hash) {
            return Err(validation("cursor authority revision hash is invalid"));
        }
    }
    if !valid_utc_watermark(&manifest.initial_watermark) || manifest.batches.is_empty() {
        return Err(validation(
            "cursor requires an initial UTC watermark and at least one batch",
        ));
    }
    let symbols = cursor_symbols(&manifest.vocabulary);
    if symbols.iter().collect::<BTreeSet<_>>().len() != symbols.len() {
        return Err(validation("cursor vocabulary contains duplicate symbols"));
    }

    let mut entity_keys = BTreeSet::from([
        manifest.contract_atom,
        manifest.initial_cursor_atom,
        manifest.authority.atom,
    ]);
    let mut relation_keys = BTreeSet::new();
    let mut previous_watermark = &manifest.initial_watermark;
    for (index, batch) in manifest.batches.iter().enumerate() {
        if batch.index != index
            || batch.batch_id.trim().is_empty()
            || batch.records.is_empty()
            || batch.records.len() > manifest.budgets.batch_limit
            || batch.source_attempts == 0
            || batch.source_attempts > manifest.budgets.max_retries + 1
            || batch.prior_watermark != *previous_watermark
            || !valid_utc_watermark(&batch.next_watermark)
        {
            return Err(validation(format!(
                "cursor batch {} violates ordering, size, retry, or watermark contract",
                batch.batch_id
            )));
        }
        let batch_entity_keys = [
            batch.atom,
            batch.prepared_cursor_atom,
            batch.receipt_atom,
            batch.next_cursor_atom,
            batch.conflict_atom,
        ];
        if batch_entity_keys
            .iter()
            .any(|key| !entity_keys.insert(*key))
            || !relation_keys.insert(batch.conflict_relation)
        {
            return Err(validation("cursor manifest contains duplicate stable keys"));
        }
        let mut previous_record_watermark = batch.prior_watermark.clone();
        let mut source_ids = BTreeSet::new();
        for record in &batch.records {
            let watermark = CursorWatermark {
                updated_at: record.updated_at.clone(),
                source_id: record.source_id.clone(),
            };
            if watermark <= previous_record_watermark
                || record.graph_id != manifest.authority.source_graph
                || !source_ids.insert(&record.source_id)
                || !entity_keys.insert(record.candidate_atom)
                || !entity_keys.insert(record.asset_atom)
                || !valid_hash(&record.row_sha256)
                || !valid_hash(&record.properties_sha256)
            {
                return Err(validation(format!(
                    "cursor source record {} is unordered, duplicated, out of scope, or unhashed",
                    record.source_id
                )));
            }
            previous_record_watermark = watermark;
        }
        let last = batch.records.last().expect("batch is non-empty");
        if batch.next_watermark.updated_at != last.updated_at
            || batch.next_watermark.source_id != last.source_id
        {
            return Err(validation(
                "next cursor watermark must equal the last deterministic source row",
            ));
        }
        previous_watermark = &batch.next_watermark;
    }
    Ok(())
}

pub fn bootstrap_cursor_store(
    manifest: &CursorManifest,
    output: impl AsRef<Path>,
) -> Result<CursorEvidence, UniverseError> {
    validate_cursor_manifest(manifest)?;
    let output = output.as_ref();
    let store = UniverseStore::open(output)?;
    if !output.join("snapshot.json").exists() {
        store.install_seed(&cursor_seed(manifest)?)?;
    }
    inspect_cursor_store(manifest, output)
}

pub fn apply_cursor_batch(
    manifest: &CursorManifest,
    output: impl AsRef<Path>,
    batch_index: usize,
) -> Result<CursorEvidence, UniverseError> {
    validate_cursor_manifest(manifest)?;
    let batch = manifest
        .batches
        .get(batch_index)
        .ok_or_else(|| validation("cursor batch index is outside the manifest"))?;
    let output = output.as_ref();
    let store = UniverseStore::open(output)?;
    let snapshot = store.replay(store.load_snapshot()?)?;
    verify_cursor_contract(&store, &snapshot, manifest)?;
    let batch_hash = cursor_batch_hash(batch)?;
    let prepare_key = prepare_idempotency_key(batch);
    let finalize_key = finalize_idempotency_key(batch);
    let prepare_exists = snapshot.event_keys.contains(&prepare_key);
    let finalize_exists = snapshot.event_keys.contains(&finalize_key);

    if prepare_exists {
        if stored_batch_hash(&store, &snapshot, batch.atom)?.as_deref() != Some(&batch_hash) {
            return record_cursor_conflict(
                manifest,
                batch,
                output,
                "source_revision_or_hash_drift_after_prepare",
                &batch_hash,
            );
        }
        verify_prepared_batch(&store, &snapshot, manifest, batch, &batch_hash)?;
        if finalize_exists {
            verify_finalized_batch(&store, &snapshot, manifest, batch, &batch_hash)?;
            return evidence_from_snapshot(
                &store,
                &snapshot,
                manifest,
                CursorApplyStatus::AlreadyCommitted,
                Some(batch.batch_id.clone()),
                false,
                batch.records.len(),
            );
        }
        return finalize_prepared_batch(
            manifest,
            batch,
            output,
            snapshot,
            batch_hash,
            CursorApplyStatus::ResumedPrepared,
        );
    }
    if finalize_exists {
        return Err(UniverseError::CorruptContent(
            "cursor finalization exists without its preparation event".into(),
        ));
    }

    let current = current_cursor(&store, &snapshot, manifest)?;
    if current.watermark != batch.prior_watermark {
        return record_cursor_conflict(
            manifest,
            batch,
            output,
            "cursor_position_conflict",
            &batch_hash,
        );
    }
    if batch.source_schema_revision_sha256 != manifest.authority.source_schema_revision_sha256
        || batch.adapter_revision_sha256 != manifest.authority.adapter_revision_sha256
        || batch.mapping_revision_sha256 != manifest.authority.mapping_revision_sha256
    {
        return record_cursor_conflict(
            manifest,
            batch,
            output,
            "source_or_mapping_revision_drift",
            &batch_hash,
        );
    }

    commit_prepared_batch(&store, snapshot, manifest, batch, &batch_hash)?;
    let independent_store = UniverseStore::open(output)?;
    let prepared = independent_store.replay(independent_store.load_snapshot()?)?;
    verify_prepared_batch(&independent_store, &prepared, manifest, batch, &batch_hash)?;
    finalize_prepared_batch(
        manifest,
        batch,
        output,
        prepared,
        batch_hash,
        CursorApplyStatus::Committed,
    )
}

pub fn inspect_cursor_store(
    manifest: &CursorManifest,
    output: impl AsRef<Path>,
) -> Result<CursorEvidence, UniverseError> {
    validate_cursor_manifest(manifest)?;
    let store = UniverseStore::open(output)?;
    let snapshot = store.replay(store.load_snapshot()?)?;
    verify_cursor_contract(&store, &snapshot, manifest)?;
    evidence_from_snapshot(
        &store,
        &snapshot,
        manifest,
        CursorApplyStatus::AlreadyCommitted,
        None,
        false,
        0,
    )
}

fn cursor_seed(manifest: &CursorManifest) -> Result<GraphSeed, UniverseError> {
    let vocabulary = &manifest.vocabulary;
    let symbols = cursor_symbols(vocabulary);
    let entities = vec![
        SeedEntity {
            key: manifest.authority.atom,
            generation: 0,
            symbol: vocabulary.source.clone(),
            content: source_content(manifest),
        },
        SeedEntity {
            key: manifest.contract_atom,
            generation: 0,
            symbol: vocabulary.contract.clone(),
            content: contract_content(manifest),
        },
        SeedEntity {
            key: manifest.initial_cursor_atom,
            generation: 0,
            symbol: vocabulary.cursor_state.clone(),
            content: cursor_content(
                "committed_initial",
                None,
                &manifest.initial_watermark,
                manifest,
            ),
        },
    ];
    let relations = vec![
        SeedRelation {
            key: manifest.relation_key_start,
            generation: 0,
            source: manifest.initial_cursor_atom,
            target: manifest.contract_atom,
            predicate: vocabulary.governed_by.clone(),
            content: Some(relation_content(
                "The initial cursor is governed by the bounded graph-owned contract.",
            )),
        },
        SeedRelation {
            key: RelationKey(manifest.relation_key_start.0 + 1),
            generation: 0,
            source: manifest.contract_atom,
            target: manifest.authority.atom,
            predicate: vocabulary.observed_from.clone(),
            content: Some(relation_content(
                "The contract names a read-only PostgreSQL source without credentials.",
            )),
        },
    ];
    let seed = GraphSeed {
        universe: manifest.universe,
        symbols,
        entities,
        relations,
    };
    seed.validate()?;
    Ok(seed)
}

fn commit_prepared_batch(
    store: &UniverseStore,
    mut snapshot: UniverseSnapshot,
    manifest: &CursorManifest,
    batch: &CursorBatch,
    batch_hash: &str,
) -> Result<CommitReceipt, UniverseError> {
    let mut commands = Vec::new();
    commands.push(put_entity(
        store,
        batch.atom,
        symbol(&snapshot, &manifest.vocabulary.batch)?,
        batch_content(batch, batch_hash),
    )?);
    commands.push(put_entity(
        store,
        batch.prepared_cursor_atom,
        symbol(&snapshot, &manifest.vocabulary.cursor_state)?,
        cursor_content(
            "prepared_unpublished",
            Some(&batch.batch_id),
            &batch.next_watermark,
            manifest,
        ),
    )?);
    for record in &batch.records {
        commands.push(put_entity(
            store,
            record.candidate_atom,
            symbol(&snapshot, &manifest.vocabulary.source_record)?,
            source_record_content(record, batch),
        )?);
        commands.push(put_entity(
            store,
            record.asset_atom,
            symbol(&snapshot, &manifest.vocabulary.asset)?,
            asset_content(record, batch),
        )?);
    }
    let governed_by = symbol(&snapshot, &manifest.vocabulary.governed_by)?;
    let observed_from = symbol(&snapshot, &manifest.vocabulary.observed_from)?;
    let proposes_cursor = symbol(&snapshot, &manifest.vocabulary.proposes_cursor)?;
    let maps_to = symbol(&snapshot, &manifest.vocabulary.maps_to)?;
    let in_batch = symbol(&snapshot, &manifest.vocabulary.in_batch)?;
    let mut relation_key = batch.relation_key_start.0;
    commands.push(put_relation(
        store,
        RelationKey(relation_key),
        batch.atom,
        manifest.contract_atom,
        governed_by,
        "The prepared batch is governed by the pinned cursor contract.",
    )?);
    relation_key += 1;
    commands.push(put_relation(
        store,
        RelationKey(relation_key),
        batch.atom,
        manifest.authority.atom,
        observed_from,
        "The batch was measured from the declared read-only source snapshot.",
    )?);
    relation_key += 1;
    commands.push(put_relation(
        store,
        RelationKey(relation_key),
        batch.atom,
        batch.prepared_cursor_atom,
        proposes_cursor,
        "This cursor remains unpublished until independent readback succeeds.",
    )?);
    relation_key += 1;
    for record in &batch.records {
        commands.push(put_relation(
            store,
            RelationKey(relation_key),
            record.candidate_atom,
            record.asset_atom,
            maps_to,
            "The source record maps to an inert content Asset.",
        )?);
        relation_key += 1;
        commands.push(put_relation(
            store,
            RelationKey(relation_key),
            record.asset_atom,
            batch.atom,
            in_batch,
            "The inert Asset belongs to this bounded cursor batch.",
        )?);
        relation_key += 1;
    }
    commit_commands(
        store,
        &mut snapshot,
        prepare_idempotency_key(batch),
        vec![batch.batch_id.clone(), "cursor_prepare".into()],
        commands,
    )
}

fn finalize_prepared_batch(
    manifest: &CursorManifest,
    batch: &CursorBatch,
    output: &Path,
    mut prepared: UniverseSnapshot,
    batch_hash: String,
    status: CursorApplyStatus,
) -> Result<CursorEvidence, UniverseError> {
    let store = UniverseStore::open(output)?;
    let prepared_hash = prepared.canonical_hash()?;
    let prepared_content_count = read_all_content(&store, &prepared)?;
    let current = current_cursor(&store, &prepared, manifest)?;
    if current.watermark != batch.prior_watermark {
        return Err(UniverseError::RevisionConflict {
            expected: prepared.revision,
            actual: prepared.revision,
        });
    }
    let receipt_content = serde_json::json!({
        "kind": "postgres_cursor_batch_receipt",
        "status": "measured_after_independent_readback",
        "information_status": "measured",
        "batch_id": batch.batch_id,
        "batch_sha256": batch_hash,
        "source_snapshot": batch.source_snapshot,
        "source_schema_revision_sha256": batch.source_schema_revision_sha256,
        "adapter_revision_sha256": batch.adapter_revision_sha256,
        "mapping_revision_sha256": batch.mapping_revision_sha256,
        "prior_watermark": batch.prior_watermark,
        "next_watermark": batch.next_watermark,
        "imported_records": batch.records.len(),
        "source_revision_min": batch.records.iter().map(|record| record.source_revision).min(),
        "source_revision_max": batch.records.iter().map(|record| record.source_revision).max(),
        "row_sha256": batch.records.iter().map(|record| &record.row_sha256).collect::<Vec<_>>(),
        "prepared_snapshot_sha256": prepared_hash,
        "prepared_content_records_read_back": prepared_content_count,
        "next_cursor_published": true,
        "executable_records": 0,
        "ontology_activated": false,
        "physical_mapping_activated": false
    });
    let commands = vec![
        put_entity(
            &store,
            batch.receipt_atom,
            symbol(&prepared, &manifest.vocabulary.receipt)?,
            receipt_content,
        )?,
        put_entity(
            &store,
            batch.next_cursor_atom,
            symbol(&prepared, &manifest.vocabulary.cursor_state)?,
            cursor_content(
                "committed_after_independent_readback",
                Some(&batch.batch_id),
                &batch.next_watermark,
                manifest,
            ),
        )?,
        put_relation(
            &store,
            RelationKey(batch.relation_key_start.0 + 32),
            current.atom,
            batch.next_cursor_atom,
            symbol(&prepared, &manifest.vocabulary.advances_to)?,
            "The authoritative cursor advances only after exact independent readback.",
        )?,
        put_relation(
            &store,
            RelationKey(batch.relation_key_start.0 + 33),
            batch.atom,
            batch.receipt_atom,
            symbol(&prepared, &manifest.vocabulary.has_receipt)?,
            "The committed batch owns its independently measured receipt.",
        )?,
        put_relation(
            &store,
            RelationKey(batch.relation_key_start.0 + 34),
            batch.receipt_atom,
            batch.next_cursor_atom,
            symbol(&prepared, &manifest.vocabulary.confirms_cursor)?,
            "The readback receipt confirms the exact next cursor watermark.",
        )?,
    ];
    commit_commands(
        &store,
        &mut prepared,
        finalize_idempotency_key(batch),
        vec![batch.batch_id.clone(), "cursor_finalize".into()],
        commands,
    )?;

    let independent_store = UniverseStore::open(output)?;
    let final_snapshot = independent_store.replay(independent_store.load_snapshot()?)?;
    verify_prepared_batch(
        &independent_store,
        &final_snapshot,
        manifest,
        batch,
        &batch_hash,
    )?;
    verify_finalized_batch(
        &independent_store,
        &final_snapshot,
        manifest,
        batch,
        &batch_hash,
    )?;
    evidence_from_snapshot(
        &independent_store,
        &final_snapshot,
        manifest,
        status,
        Some(batch.batch_id.clone()),
        true,
        batch.records.len(),
    )
}

fn record_cursor_conflict(
    manifest: &CursorManifest,
    batch: &CursorBatch,
    output: &Path,
    reason: &str,
    proposed_batch_hash: &str,
) -> Result<CursorEvidence, UniverseError> {
    let store = UniverseStore::open(output)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let current = current_cursor(&store, &snapshot, manifest)?;
    let idempotency_key = format!("postgres-cursor:{}:conflict:{reason}", batch.batch_id);
    let conflict_content = serde_json::json!({
        "kind": "postgres_cursor_conflict",
        "status": "conflict",
        "information_status": "measured",
        "reason": reason,
        "batch_id": batch.batch_id,
        "current_cursor_atom": current.atom,
        "current_watermark": current.watermark,
        "proposed_prior_watermark": batch.prior_watermark,
        "proposed_next_watermark": batch.next_watermark,
        "proposed_batch_sha256": proposed_batch_hash,
        "expected_source_schema_revision_sha256": manifest.authority.source_schema_revision_sha256,
        "observed_source_schema_revision_sha256": batch.source_schema_revision_sha256,
        "expected_adapter_revision_sha256": manifest.authority.adapter_revision_sha256,
        "observed_adapter_revision_sha256": batch.adapter_revision_sha256,
        "expected_mapping_revision_sha256": manifest.authority.mapping_revision_sha256,
        "observed_mapping_revision_sha256": batch.mapping_revision_sha256,
        "cursor_advanced": false,
        "ontology_activated": false,
        "executable": false
    });
    let status = if snapshot.event_keys.contains(&idempotency_key) {
        verify_entity_content(&store, &snapshot, batch.conflict_atom, &conflict_content)?;
        CursorApplyStatus::ConflictAlreadyRecorded
    } else {
        let commands = vec![
            put_entity(
                &store,
                batch.conflict_atom,
                symbol(&snapshot, &manifest.vocabulary.conflict)?,
                conflict_content.clone(),
            )?,
            put_relation(
                &store,
                batch.conflict_relation,
                current.atom,
                batch.conflict_atom,
                symbol(&snapshot, &manifest.vocabulary.has_conflict)?,
                "The conflict is recorded without publishing a new cursor state.",
            )?,
        ];
        commit_commands(
            &store,
            &mut snapshot,
            idempotency_key,
            vec![batch.batch_id.clone(), "cursor_conflict".into()],
            commands,
        )?;
        let independent_store = UniverseStore::open(output)?;
        let replayed = independent_store.replay(independent_store.load_snapshot()?)?;
        verify_entity_content(
            &independent_store,
            &replayed,
            batch.conflict_atom,
            &conflict_content,
        )?;
        let replayed_cursor = current_cursor(&independent_store, &replayed, manifest)?;
        if replayed_cursor != current {
            return Err(UniverseError::CorruptContent(
                "cursor advanced while recording a conflict".into(),
            ));
        }
        snapshot = replayed;
        CursorApplyStatus::ConflictRecorded
    };
    evidence_from_snapshot(
        &store,
        &snapshot,
        manifest,
        status,
        Some(batch.batch_id.clone()),
        false,
        0,
    )
}

fn verify_cursor_contract(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    manifest: &CursorManifest,
) -> Result<(), UniverseError> {
    if snapshot.universe != manifest.universe {
        return Err(validation("cursor store Universe differs from manifest"));
    }
    verify_entity_content(
        store,
        snapshot,
        manifest.authority.atom,
        &source_content(manifest),
    )?;
    verify_entity_content(
        store,
        snapshot,
        manifest.contract_atom,
        &contract_content(manifest),
    )?;
    verify_entity_content(
        store,
        snapshot,
        manifest.initial_cursor_atom,
        &cursor_content(
            "committed_initial",
            None,
            &manifest.initial_watermark,
            manifest,
        ),
    )
}

fn verify_prepared_batch(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    manifest: &CursorManifest,
    batch: &CursorBatch,
    batch_hash: &str,
) -> Result<(), UniverseError> {
    verify_entity_content(
        store,
        snapshot,
        batch.atom,
        &batch_content(batch, batch_hash),
    )?;
    verify_entity_content(
        store,
        snapshot,
        batch.prepared_cursor_atom,
        &cursor_content(
            "prepared_unpublished",
            Some(&batch.batch_id),
            &batch.next_watermark,
            manifest,
        ),
    )?;
    for record in &batch.records {
        verify_entity_content(
            store,
            snapshot,
            record.candidate_atom,
            &source_record_content(record, batch),
        )?;
        verify_entity_content(
            store,
            snapshot,
            record.asset_atom,
            &asset_content(record, batch),
        )?;
    }
    Ok(())
}

fn verify_finalized_batch(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    manifest: &CursorManifest,
    batch: &CursorBatch,
    batch_hash: &str,
) -> Result<(), UniverseError> {
    verify_entity_content(
        store,
        snapshot,
        batch.next_cursor_atom,
        &cursor_content(
            "committed_after_independent_readback",
            Some(&batch.batch_id),
            &batch.next_watermark,
            manifest,
        ),
    )?;
    let receipt = read_entity_content(store, snapshot, batch.receipt_atom)?;
    if receipt["kind"] != "postgres_cursor_batch_receipt"
        || receipt["status"] != "measured_after_independent_readback"
        || receipt["batch_id"] != batch.batch_id
        || receipt["batch_sha256"] != batch_hash
        || receipt["prior_watermark"]
            != serde_json::to_value(&batch.prior_watermark)
                .map_err(|error| validation(error.to_string()))?
        || receipt["next_watermark"]
            != serde_json::to_value(&batch.next_watermark)
                .map_err(|error| validation(error.to_string()))?
        || receipt["next_cursor_published"] != true
        || receipt["executable_records"] != 0
        || receipt["ontology_activated"] != false
        || receipt["physical_mapping_activated"] != false
        || !receipt["prepared_snapshot_sha256"]
            .as_str()
            .is_some_and(valid_hash)
    {
        return Err(UniverseError::CorruptContent(
            "cursor final receipt readback mismatch".into(),
        ));
    }
    let advances_to = symbol(snapshot, &manifest.vocabulary.advances_to)?;
    let has_receipt = symbol(snapshot, &manifest.vocabulary.has_receipt)?;
    let confirms_cursor = symbol(snapshot, &manifest.vocabulary.confirms_cursor)?;
    let prior_atom = if batch.index == 0 {
        manifest.initial_cursor_atom
    } else {
        manifest.batches[batch.index - 1].next_cursor_atom
    };
    for (key, source, target, predicate) in [
        (
            RelationKey(batch.relation_key_start.0 + 32),
            prior_atom,
            batch.next_cursor_atom,
            advances_to,
        ),
        (
            RelationKey(batch.relation_key_start.0 + 33),
            batch.atom,
            batch.receipt_atom,
            has_receipt,
        ),
        (
            RelationKey(batch.relation_key_start.0 + 34),
            batch.receipt_atom,
            batch.next_cursor_atom,
            confirms_cursor,
        ),
    ] {
        if !snapshot.relations.iter().any(|relation| {
            relation.key == key
                && relation.source == source
                && relation.target == target
                && relation.predicate == predicate
        }) {
            return Err(UniverseError::CorruptContent(
                "cursor final relation readback mismatch".into(),
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CurrentCursor {
    atom: EntityKey,
    watermark: CursorWatermark,
}

fn current_cursor(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    manifest: &CursorManifest,
) -> Result<CurrentCursor, UniverseError> {
    let advances_to = symbol(snapshot, &manifest.vocabulary.advances_to)?;
    let mut atom = manifest.initial_cursor_atom;
    let mut visited = BTreeSet::new();
    for _ in 0..=manifest.batches.len() {
        if !visited.insert(atom) {
            return Err(UniverseError::CorruptContent(
                "cursor ADVANCES_TO chain contains a cycle".into(),
            ));
        }
        let next = snapshot
            .relations
            .iter()
            .filter(|relation| relation.source == atom && relation.predicate == advances_to)
            .collect::<Vec<_>>();
        match next.as_slice() {
            [] => {
                let content = read_entity_content(store, snapshot, atom)?;
                let watermark: CursorWatermark =
                    serde_json::from_value(content["watermark"].clone())
                        .map_err(|error| UniverseError::CorruptContent(error.to_string()))?;
                return Ok(CurrentCursor { atom, watermark });
            }
            [relation] => atom = relation.target,
            _ => {
                return Err(UniverseError::CorruptContent(
                    "cursor has more than one authoritative successor".into(),
                ));
            }
        }
    }
    Err(UniverseError::CorruptContent(
        "cursor chain exceeds the bounded manifest".into(),
    ))
}

fn evidence_from_snapshot(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    manifest: &CursorManifest,
    status: CursorApplyStatus,
    batch_id: Option<String>,
    cursor_advanced: bool,
    imported_records: usize,
) -> Result<CursorEvidence, UniverseError> {
    let current = current_cursor(store, snapshot, manifest)?;
    Ok(CursorEvidence {
        status,
        batch_id,
        cursor_atom: current.atom,
        watermark: current.watermark,
        cursor_advanced,
        imported_records,
        executable_records: 0,
        ontology_activated: false,
        revision: snapshot.revision,
        tick: snapshot.tick,
        snapshot_sha256: snapshot.canonical_hash()?,
        content_records_read_back: read_all_content(store, snapshot)?,
    })
}

fn commit_commands(
    store: &UniverseStore,
    snapshot: &mut UniverseSnapshot,
    idempotency_key: String,
    causal_ancestry: Vec<String>,
    commands: Vec<UniverseCommand>,
) -> Result<CommitReceipt, UniverseError> {
    let transaction = UniverseTransaction::prepare(
        snapshot,
        UniverseWriteSet {
            base_revision: snapshot.revision,
            idempotency_key,
            causal_ancestry,
            commands,
        },
    )?;
    transaction.commit(store, snapshot, Tick(snapshot.tick.0 + 1))
}

fn put_entity(
    store: &UniverseStore,
    key: EntityKey,
    symbol: u32,
    content: serde_json::Value,
) -> Result<UniverseCommand, UniverseError> {
    Ok(UniverseCommand::PutEntity {
        entity: EntityRecord {
            key,
            generation: 0,
            symbol,
            content: Some(store.append_content(&content)?),
        },
    })
}

fn put_relation(
    store: &UniverseStore,
    key: RelationKey,
    source: EntityKey,
    target: EntityKey,
    predicate: u32,
    justification: &str,
) -> Result<UniverseCommand, UniverseError> {
    Ok(UniverseCommand::PutRelation {
        relation: RelationRecord {
            key,
            generation: 0,
            source,
            target,
            predicate,
            content: Some(store.append_content(&relation_content(justification))?),
        },
    })
}

fn symbol(snapshot: &UniverseSnapshot, value: &str) -> Result<u32, UniverseError> {
    snapshot
        .symbol_id(value)
        .ok_or_else(|| validation(format!("cursor symbol {value} is not interned")))
}

fn read_entity_content(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    key: EntityKey,
) -> Result<serde_json::Value, UniverseError> {
    let entity = snapshot
        .entities
        .iter()
        .find(|entity| entity.key == key)
        .ok_or_else(|| validation(format!("cursor entity {key} is missing")))?;
    store.read_content(
        entity
            .content
            .as_ref()
            .ok_or_else(|| validation(format!("cursor entity {key} has no content")))?,
    )
}

fn verify_entity_content(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    key: EntityKey,
    expected: &serde_json::Value,
) -> Result<(), UniverseError> {
    if read_entity_content(store, snapshot, key)? != *expected {
        return Err(UniverseError::CorruptContent(format!(
            "cursor entity {key} content readback mismatch"
        )));
    }
    Ok(())
}

fn stored_batch_hash(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    key: EntityKey,
) -> Result<Option<String>, UniverseError> {
    let Some(entity) = snapshot.entities.iter().find(|entity| entity.key == key) else {
        return Ok(None);
    };
    let content = store.read_content(
        entity
            .content
            .as_ref()
            .ok_or_else(|| validation("stored cursor batch has no content"))?,
    )?;
    Ok(content["batch_sha256"].as_str().map(ToOwned::to_owned))
}

fn source_content(manifest: &CursorManifest) -> serde_json::Value {
    serde_json::json!({
        "kind": "postgres_cursor_source",
        "authority_id": manifest.authority.authority_id,
        "source_schema": manifest.authority.source_schema,
        "source_graph": manifest.authority.source_graph,
        "server_version_num": manifest.authority.server_version_num,
        "source_schema_revision_sha256": manifest.authority.source_schema_revision_sha256,
        "adapter_revision": manifest.authority.adapter_revision,
        "adapter_revision_sha256": manifest.authority.adapter_revision_sha256,
        "mapping_revision": manifest.authority.mapping_revision,
        "mapping_revision_sha256": manifest.authority.mapping_revision_sha256,
        "read_only": manifest.authority.read_only,
        "credentials_stored": false
    })
}

fn contract_content(manifest: &CursorManifest) -> serde_json::Value {
    serde_json::json!({
        "kind": "postgres_cursor_contract",
        "contract_version": manifest.contract_version,
        "ordering": ["updated_at_utc", "global_source_id"],
        "isolation_level": "repeatable_read",
        "read_only": true,
        "batch_limit": manifest.budgets.batch_limit,
        "statement_timeout_ms": manifest.budgets.statement_timeout_ms,
        "transaction_timeout_ms": manifest.budgets.transaction_timeout_ms,
        "max_retries": manifest.budgets.max_retries,
        "retry_backoff_ms": manifest.budgets.retry_backoff_ms,
        "next_cursor_publication": "after_atomic_commit_and_independent_readback",
        "idempotency": "batch_id_plus_pinned_source_and_mapping_revisions",
        "invalid_batch_advances_cursor": false,
        "source_status_activates_target": false
    })
}

fn cursor_content(
    status: &str,
    batch_id: Option<&str>,
    watermark: &CursorWatermark,
    manifest: &CursorManifest,
) -> serde_json::Value {
    serde_json::json!({
        "kind": "postgres_cursor_state",
        "status": status,
        "batch_id": batch_id,
        "watermark": watermark,
        "source_schema_revision_sha256": manifest.authority.source_schema_revision_sha256,
        "adapter_revision_sha256": manifest.authority.adapter_revision_sha256,
        "mapping_revision_sha256": manifest.authority.mapping_revision_sha256,
        "ontology_activated": false,
        "executable": false
    })
}

fn batch_content(batch: &CursorBatch, batch_hash: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "postgres_cursor_batch",
        "status": "prepared_inert",
        "batch_id": batch.batch_id,
        "index": batch.index,
        "observed_at": batch.observed_at,
        "source_snapshot": batch.source_snapshot,
        "source_attempts": batch.source_attempts,
        "has_more": batch.has_more,
        "source_schema_revision_sha256": batch.source_schema_revision_sha256,
        "adapter_revision_sha256": batch.adapter_revision_sha256,
        "mapping_revision_sha256": batch.mapping_revision_sha256,
        "prior_watermark": batch.prior_watermark,
        "next_watermark": batch.next_watermark,
        "record_count": batch.records.len(),
        "batch_sha256": batch_hash,
        "ontology_activated": false,
        "executable": false
    })
}

fn source_record_content(record: &CursorSourceRecord, batch: &CursorBatch) -> serde_json::Value {
    serde_json::json!({
        "kind": "postgres_cursor_source_record",
        "batch_id": batch.batch_id,
        "source_snapshot": batch.source_snapshot,
        "graph_id": record.graph_id,
        "source_id": record.source_id,
        "node_type": record.node_type,
        "subtype": record.subtype,
        "source_status": record.source_status,
        "source_revision": record.source_revision,
        "updated_at": record.updated_at,
        "row_sha256": record.row_sha256,
        "properties_sha256": record.properties_sha256,
        "properties_imported": false
    })
}

fn asset_content(record: &CursorSourceRecord, batch: &CursorBatch) -> serde_json::Value {
    serde_json::json!({
        "kind": "inert_cursor_asset",
        "batch_id": batch.batch_id,
        "graph_id": record.graph_id,
        "source_id": record.source_id,
        "source_revision": record.source_revision,
        "updated_at": record.updated_at,
        "row_sha256": record.row_sha256,
        "properties_sha256": record.properties_sha256,
        "target_status": "imported_inert",
        "payload_imported": false,
        "ontology_activated": false,
        "physical_mapping_activated": false,
        "executable": false
    })
}

fn relation_content(justification: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "postgres_cursor_relation",
        "justification": justification
    })
}

fn cursor_batch_hash(batch: &CursorBatch) -> Result<String, UniverseError> {
    canonical_hash(&(
        &batch.batch_id,
        batch.index,
        &batch.observed_at,
        &batch.source_snapshot,
        batch.source_attempts,
        batch.has_more,
        &batch.source_schema_revision_sha256,
        &batch.adapter_revision_sha256,
        &batch.mapping_revision_sha256,
        &batch.prior_watermark,
        &batch.next_watermark,
        &batch.records,
    ))
}

fn prepare_idempotency_key(batch: &CursorBatch) -> String {
    format!("postgres-cursor:{}:prepare", batch.batch_id)
}

fn finalize_idempotency_key(batch: &CursorBatch) -> String {
    format!("postgres-cursor:{}:finalize", batch.batch_id)
}

fn cursor_symbols(vocabulary: &CursorVocabulary) -> Vec<String> {
    vec![
        vocabulary.source.clone(),
        vocabulary.contract.clone(),
        vocabulary.cursor_state.clone(),
        vocabulary.batch.clone(),
        vocabulary.source_record.clone(),
        vocabulary.asset.clone(),
        vocabulary.receipt.clone(),
        vocabulary.conflict.clone(),
        vocabulary.governed_by.clone(),
        vocabulary.observed_from.clone(),
        vocabulary.proposes_cursor.clone(),
        vocabulary.advances_to.clone(),
        vocabulary.in_batch.clone(),
        vocabulary.maps_to.clone(),
        vocabulary.has_receipt.clone(),
        vocabulary.confirms_cursor.clone(),
        vocabulary.has_conflict.clone(),
    ]
}

fn valid_utc_watermark(watermark: &CursorWatermark) -> bool {
    watermark.updated_at.ends_with('Z')
        && watermark.updated_at.contains('T')
        && !watermark.source_id.trim().is_empty()
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_all_content(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
) -> Result<usize, UniverseError> {
    let mut count = 0;
    for content in snapshot
        .entities
        .iter()
        .filter_map(|entity| entity.content.as_ref())
        .chain(
            snapshot
                .relations
                .iter()
                .filter_map(|relation| relation.content.as_ref()),
        )
    {
        store.read_content(content)?;
        count += 1;
    }
    Ok(count)
}

fn validation(message: impl Into<String>) -> UniverseError {
    UniverseError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> CursorManifest {
        serde_json::from_str(include_str!(
            "../../../fixtures/import/postgres-cursor-batches.json"
        ))
        .unwrap()
    }

    #[test]
    fn restart_resumes_from_last_read_back_watermark() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = manifest();
        let boot = bootstrap_cursor_store(&manifest, temp.path()).unwrap();
        assert_eq!(boot.watermark, manifest.initial_watermark);
        let first = apply_cursor_batch(&manifest, temp.path(), 0).unwrap();
        assert_eq!(first.status, CursorApplyStatus::Committed);
        assert_eq!(first.watermark, manifest.batches[0].next_watermark);
        drop(UniverseStore::open(temp.path()).unwrap());
        let second = apply_cursor_batch(&manifest, temp.path(), 1).unwrap();
        assert_eq!(second.status, CursorApplyStatus::Committed);
        assert_eq!(second.watermark, manifest.batches[1].next_watermark);
        assert_eq!(second.revision, Revision(4));
    }

    #[test]
    fn rerun_is_idempotent_after_replay() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = manifest();
        bootstrap_cursor_store(&manifest, temp.path()).unwrap();
        apply_cursor_batch(&manifest, temp.path(), 0).unwrap();
        apply_cursor_batch(&manifest, temp.path(), 1).unwrap();
        let before = inspect_cursor_store(&manifest, temp.path()).unwrap();
        let rerun = apply_cursor_batch(&manifest, temp.path(), 0).unwrap();
        let after = inspect_cursor_store(&manifest, temp.path()).unwrap();
        assert_eq!(rerun.status, CursorApplyStatus::AlreadyCommitted);
        assert!(!rerun.cursor_advanced);
        assert_eq!(before.revision, after.revision);
        assert_eq!(before.snapshot_sha256, after.snapshot_sha256);
        assert_eq!(before.watermark, after.watermark);
    }

    #[test]
    fn invalid_batch_writes_nothing_and_does_not_advance() {
        let temp = tempfile::tempdir().unwrap();
        let valid_manifest = manifest();
        bootstrap_cursor_store(&valid_manifest, temp.path()).unwrap();
        let before = inspect_cursor_store(&valid_manifest, temp.path()).unwrap();
        let mut invalid_manifest = valid_manifest.clone();
        invalid_manifest.batches[0].records.swap(0, 1);
        assert!(apply_cursor_batch(&invalid_manifest, temp.path(), 0).is_err());
        let after = inspect_cursor_store(&valid_manifest, temp.path()).unwrap();
        assert_eq!(before.revision, after.revision);
        assert_eq!(before.snapshot_sha256, after.snapshot_sha256);
        assert_eq!(before.watermark, after.watermark);
    }

    #[test]
    fn mapping_revision_drift_is_graph_owned_without_cursor_advance() {
        let temp = tempfile::tempdir().unwrap();
        let mut manifest = manifest();
        bootstrap_cursor_store(&manifest, temp.path()).unwrap();
        let before = inspect_cursor_store(&manifest, temp.path()).unwrap();
        manifest.batches[0].mapping_revision_sha256 = "0".repeat(64);
        let conflict = apply_cursor_batch(&manifest, temp.path(), 0).unwrap();
        assert_eq!(conflict.status, CursorApplyStatus::ConflictRecorded);
        assert!(!conflict.cursor_advanced);
        assert_eq!(conflict.watermark, before.watermark);
        assert!(conflict.revision > before.revision);
    }

    #[test]
    fn source_row_drift_after_commit_records_conflict_without_duplicate() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = manifest();
        bootstrap_cursor_store(&manifest, temp.path()).unwrap();
        let committed = apply_cursor_batch(&manifest, temp.path(), 0).unwrap();
        let mut drifted = manifest.clone();
        drifted.batches[0].records[0].source_revision += 1;
        drifted.batches[0].records[0].row_sha256 = "f".repeat(64);
        let conflict = apply_cursor_batch(&drifted, temp.path(), 0).unwrap();
        assert_eq!(conflict.status, CursorApplyStatus::ConflictRecorded);
        assert_eq!(conflict.watermark, committed.watermark);
        let repeated = apply_cursor_batch(&drifted, temp.path(), 0).unwrap();
        assert_eq!(repeated.status, CursorApplyStatus::ConflictAlreadyRecorded);
        assert_eq!(repeated.revision, conflict.revision);
    }
}
