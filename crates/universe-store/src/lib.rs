//! Minimal deterministic snapshot, content, and replay store.

pub mod ontology;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use universe_core::{
    ContentPtr, EntityKey, RelationKey, Revision, Tick, UniverseError, UniverseId, VersionEnvelope,
};

pub const SNAPSHOT_FORMAT_VERSION: u16 = 0;
pub const SEED_FORMAT_VERSION: u16 = 0;
pub const MAX_EVENT_MUTATIONS: usize = 4_096;
const VERSIONED_CHECKPOINT_PREFIX: &str = "checkpoint-r";
const VERSIONED_CHECKPOINT_SUFFIX: &str = ".json";
const ACTIVE_EVENT_LOG: &str = "events.jsonl";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContentRef {
    pub pointer: ContentPtr,
    pub sha256: String,
}

impl ContentRef {
    fn validate(&self) -> Result<(), UniverseError> {
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(UniverseError::Validation(
                "content hash must contain exactly 64 hex digits".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EntityRecord {
    pub key: EntityKey,
    pub generation: u32,
    pub symbol: u32,
    pub content: Option<ContentRef>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationRecord {
    pub key: RelationKey,
    pub generation: u32,
    pub source: EntityKey,
    pub target: EntityKey,
    pub predicate: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ContentRef>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UniverseSnapshot {
    pub format_version: u16,
    pub universe: UniverseId,
    pub revision: Revision,
    pub tick: Tick,
    pub symbols: Vec<String>,
    pub entities: Vec<EntityRecord>,
    pub relations: Vec<RelationRecord>,
    pub event_keys: BTreeSet<String>,
}

impl UniverseSnapshot {
    pub fn empty(universe: UniverseId) -> Self {
        Self {
            format_version: SNAPSHOT_FORMAT_VERSION,
            universe,
            revision: Revision(0),
            tick: Tick(0),
            symbols: Vec::new(),
            entities: Vec::new(),
            relations: Vec::new(),
            event_keys: BTreeSet::new(),
        }
    }

    pub fn validate(&self) -> Result<(), UniverseError> {
        if self.format_version != SNAPSHOT_FORMAT_VERSION {
            return Err(UniverseError::UnsupportedVersion(self.format_version));
        }
        let symbol_count = self.symbols.len();
        if self.symbols.iter().collect::<BTreeSet<_>>().len() != symbol_count {
            return Err(UniverseError::Validation("duplicate symbol".into()));
        }
        if self.symbols.iter().any(|symbol| symbol.trim().is_empty()) {
            return Err(UniverseError::Validation("empty symbol".into()));
        }
        if self
            .entities
            .iter()
            .any(|entity| entity.symbol as usize >= symbol_count)
        {
            return Err(UniverseError::Validation(
                "entity references an unknown symbol".into(),
            ));
        }
        if self
            .relations
            .iter()
            .any(|relation| relation.predicate as usize >= symbol_count)
        {
            return Err(UniverseError::Validation(
                "relation references an unknown predicate symbol".into(),
            ));
        }
        let entities: BTreeSet<_> = self.entities.iter().map(|e| e.key).collect();
        if entities.len() != self.entities.len() {
            return Err(UniverseError::Validation("duplicate entity key".into()));
        }
        for content in self
            .entities
            .iter()
            .filter_map(|entity| entity.content.as_ref())
        {
            content.validate()?;
        }
        let relations: BTreeSet<_> = self.relations.iter().map(|r| r.key).collect();
        if relations.len() != self.relations.len() {
            return Err(UniverseError::Validation("duplicate relation key".into()));
        }
        if self
            .relations
            .iter()
            .any(|r| !entities.contains(&r.source) || !entities.contains(&r.target))
        {
            return Err(UniverseError::Validation(
                "relation endpoint does not exist".into(),
            ));
        }
        for content in self
            .relations
            .iter()
            .filter_map(|relation| relation.content.as_ref())
        {
            content.validate()?;
        }
        Ok(())
    }

    pub fn canonical_hash(&self) -> Result<String, UniverseError> {
        canonical_hash(self)
    }

    pub fn symbol_id(&self, symbol: &str) -> Option<u32> {
        self.symbols
            .iter()
            .position(|candidate| candidate == symbol)
            .and_then(|index| u32::try_from(index).ok())
    }

    /// Plans a canonical symbol-table extension without mutating the snapshot.
    ///
    /// Existing symbols retain their IDs. New names are deduplicated and
    /// appended in lexical order so callers can construct records that refer to
    /// the exact IDs published by the same atomic event.
    pub fn plan_symbol_interning(
        &self,
        requested: &[String],
    ) -> Result<SymbolInternPlan, UniverseError> {
        let mut additions = BTreeSet::new();
        for symbol in requested {
            if symbol.trim().is_empty() {
                return Err(UniverseError::Validation("empty symbol".into()));
            }
            if self.symbol_id(symbol).is_none() {
                additions.insert(symbol.clone());
            }
        }
        let additions: Vec<_> = additions.into_iter().collect();
        let final_len = self
            .symbols
            .len()
            .checked_add(additions.len())
            .ok_or_else(|| UniverseError::Validation("symbol table size overflow".into()))?;
        if final_len > u32::MAX as usize {
            return Err(UniverseError::Validation(
                "symbol table exceeds u32 address space".into(),
            ));
        }
        let mut assignments = BTreeMap::new();
        for symbol in requested {
            let id = self.symbol_id(symbol).unwrap_or_else(|| {
                let offset = additions
                    .binary_search(symbol)
                    .expect("new symbol is present in the canonical additions");
                (self.symbols.len() + offset) as u32
            });
            assignments.insert(symbol.clone(), id);
        }
        Ok(SymbolInternPlan {
            additions,
            assignments,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SymbolInternPlan {
    pub additions: Vec<String>,
    pub assignments: BTreeMap<String, u32>,
}

/// Immutable relation adjacency for one exact authoritative snapshot.
///
/// Entity keys and offsets form a CSR-style index. Relation positions address
/// records in the accompanying snapshot and are stored once per incident
/// endpoint (once for self-relations). The index owns no graph semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotAdjacency {
    universe: UniverseId,
    revision: Revision,
    snapshot_hash: String,
    entity_keys: Vec<EntityKey>,
    offsets: Vec<u64>,
    relation_positions: Vec<u32>,
}

impl SnapshotAdjacency {
    pub fn build(snapshot: &UniverseSnapshot) -> Result<Self, UniverseError> {
        snapshot.validate()?;
        if snapshot.relations.len() > u32::MAX as usize {
            return Err(UniverseError::BudgetExhausted(
                "snapshot has more relations than the u32 CSR position space".into(),
            ));
        }

        let mut entity_keys = snapshot
            .entities
            .iter()
            .map(|entity| entity.key)
            .collect::<Vec<_>>();
        entity_keys.sort_unstable();
        let mut degrees = vec![0usize; entity_keys.len()];
        for relation in &snapshot.relations {
            let source = entity_keys
                .binary_search(&relation.source)
                .expect("validated relation source exists");
            degrees[source] = degrees[source]
                .checked_add(1)
                .ok_or_else(|| UniverseError::BudgetExhausted("CSR degree overflow".into()))?;
            if relation.target != relation.source {
                let target = entity_keys
                    .binary_search(&relation.target)
                    .expect("validated relation target exists");
                degrees[target] = degrees[target]
                    .checked_add(1)
                    .ok_or_else(|| UniverseError::BudgetExhausted("CSR degree overflow".into()))?;
            }
        }

        let mut offsets = Vec::with_capacity(entity_keys.len() + 1);
        offsets.push(0u64);
        for degree in degrees {
            let next = offsets
                .last()
                .copied()
                .expect("CSR has an initial offset")
                .checked_add(degree as u64)
                .ok_or_else(|| UniverseError::BudgetExhausted("CSR offset overflow".into()))?;
            offsets.push(next);
        }
        let incidence_count = usize::try_from(*offsets.last().expect("CSR has a final offset"))
            .map_err(|_| {
                UniverseError::BudgetExhausted("CSR incidence count exceeds address space".into())
            })?;
        let mut relation_positions = vec![u32::MAX; incidence_count];
        let mut cursors = offsets[..entity_keys.len()]
            .iter()
            .map(|offset| {
                usize::try_from(*offset).map_err(|_| {
                    UniverseError::BudgetExhausted("CSR cursor exceeds address space".into())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (position, relation) in snapshot.relations.iter().enumerate() {
            let position = u32::try_from(position).map_err(|_| {
                UniverseError::BudgetExhausted("relation position exceeds u32".into())
            })?;
            let source = entity_keys
                .binary_search(&relation.source)
                .expect("validated relation source exists");
            relation_positions[cursors[source]] = position;
            cursors[source] += 1;
            if relation.target != relation.source {
                let target = entity_keys
                    .binary_search(&relation.target)
                    .expect("validated relation target exists");
                relation_positions[cursors[target]] = position;
                cursors[target] += 1;
            }
        }
        for index in 0..entity_keys.len() {
            let start = usize::try_from(offsets[index]).expect("validated CSR start");
            let end = usize::try_from(offsets[index + 1]).expect("validated CSR end");
            relation_positions[start..end]
                .sort_unstable_by_key(|position| snapshot.relations[*position as usize].key);
        }

        Ok(Self {
            universe: snapshot.universe,
            revision: snapshot.revision,
            snapshot_hash: snapshot.canonical_hash()?,
            entity_keys,
            offsets,
            relation_positions,
        })
    }

    pub fn universe(&self) -> UniverseId {
        self.universe
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn snapshot_hash(&self) -> &str {
        &self.snapshot_hash
    }

    pub fn entity_count(&self) -> usize {
        self.entity_keys.len()
    }

    pub fn incidence_count(&self) -> usize {
        self.relation_positions.len()
    }

    pub fn contains(&self, entity: EntityKey) -> bool {
        self.entity_keys.binary_search(&entity).is_ok()
    }

    pub fn relation_positions(&self, entity: EntityKey) -> &[u32] {
        let Ok(index) = self.entity_keys.binary_search(&entity) else {
            return &[];
        };
        let start = usize::try_from(self.offsets[index]).expect("validated CSR start");
        let end = usize::try_from(self.offsets[index + 1]).expect("validated CSR end");
        &self.relation_positions[start..end]
    }

    pub fn validate_against(&self, snapshot: &UniverseSnapshot) -> Result<(), UniverseError> {
        let hash = snapshot.canonical_hash()?;
        if self.universe != snapshot.universe
            || self.revision != snapshot.revision
            || self.snapshot_hash != hash
        {
            return Err(UniverseError::Validation(
                "CSR adjacency does not match the authoritative snapshot".into(),
            ));
        }
        Ok(())
    }
}

/// One immutable truth-layer revision with its verified direct adjacency.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedUniverseSnapshot {
    snapshot: UniverseSnapshot,
    adjacency: SnapshotAdjacency,
}

impl IndexedUniverseSnapshot {
    pub fn new(snapshot: UniverseSnapshot) -> Result<Self, UniverseError> {
        let adjacency = SnapshotAdjacency::build(&snapshot)?;
        Ok(Self {
            snapshot,
            adjacency,
        })
    }

    pub fn snapshot(&self) -> &UniverseSnapshot {
        &self.snapshot
    }

    pub fn adjacency(&self) -> &SnapshotAdjacency {
        &self.adjacency
    }

    pub fn adjacent_relations(&self, entity: EntityKey) -> AdjacentRelations<'_> {
        AdjacentRelations {
            relations: &self.snapshot.relations,
            positions: self.adjacency.relation_positions(entity).iter(),
        }
    }
}

pub struct AdjacentRelations<'a> {
    relations: &'a [RelationRecord],
    positions: std::slice::Iter<'a, u32>,
}

impl<'a> Iterator for AdjacentRelations<'a> {
    type Item = &'a RelationRecord;

    fn next(&mut self) -> Option<Self::Item> {
        let position = *self.positions.next()? as usize;
        self.relations.get(position)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.positions.size_hint()
    }
}

impl ExactSizeIterator for AdjacentRelations<'_> {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdjacencyOverlayBudget {
    pub max_added_entities: usize,
    pub max_changed_relations: usize,
    pub max_tombstones: usize,
    pub max_touched_entities: usize,
    pub max_events: usize,
}

impl Default for AdjacencyOverlayBudget {
    fn default() -> Self {
        Self {
            max_added_entities: 65_536,
            max_changed_relations: 65_536,
            max_tombstones: 32_768,
            max_touched_entities: 65_536,
            max_events: 16_384,
        }
    }
}

/// Bounded mutable relation delta over one immutable CSR revision.
///
/// Only changed records and touched endpoints live here. A shadowed key hides
/// the corresponding base-CSR record; a tombstone additionally states that no
/// current replacement exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutableAdjacencyOverlay {
    base_universe: UniverseId,
    base_revision: Revision,
    base_snapshot_hash: String,
    current_revision: Revision,
    added_entities: BTreeSet<EntityKey>,
    relation_additions: BTreeMap<RelationKey, RelationRecord>,
    adjacency_additions: BTreeMap<EntityKey, Vec<RelationKey>>,
    shadowed_relations: BTreeSet<RelationKey>,
    tombstones: BTreeSet<RelationKey>,
    touched_entities: BTreeSet<EntityKey>,
    events_applied: usize,
    budget: AdjacencyOverlayBudget,
}

impl MutableAdjacencyOverlay {
    fn new(base: &SnapshotAdjacency, budget: AdjacencyOverlayBudget) -> Self {
        Self {
            base_universe: base.universe(),
            base_revision: base.revision(),
            base_snapshot_hash: base.snapshot_hash().to_owned(),
            current_revision: base.revision(),
            added_entities: BTreeSet::new(),
            relation_additions: BTreeMap::new(),
            adjacency_additions: BTreeMap::new(),
            shadowed_relations: BTreeSet::new(),
            tombstones: BTreeSet::new(),
            touched_entities: BTreeSet::new(),
            events_applied: 0,
            budget,
        }
    }

    pub fn base_revision(&self) -> Revision {
        self.base_revision
    }

    pub fn current_revision(&self) -> Revision {
        self.current_revision
    }

    pub fn base_snapshot_hash(&self) -> &str {
        &self.base_snapshot_hash
    }

    pub fn added_entity_count(&self) -> usize {
        self.added_entities.len()
    }

    pub fn relation_addition_count(&self) -> usize {
        self.relation_additions.len()
    }

    pub fn changed_relation_count(&self) -> usize {
        self.shadowed_relations.len()
            + self
                .relation_additions
                .keys()
                .filter(|key| !self.shadowed_relations.contains(key))
                .count()
    }

    pub fn tombstone_count(&self) -> usize {
        self.tombstones.len()
    }

    pub fn touched_entity_count(&self) -> usize {
        self.touched_entities.len()
    }

    pub fn events_applied(&self) -> usize {
        self.events_applied
    }

    pub fn is_empty(&self) -> bool {
        self.added_entities.is_empty()
            && self.relation_additions.is_empty()
            && self.shadowed_relations.is_empty()
            && self.tombstones.is_empty()
    }

    fn apply_event(
        &mut self,
        current: &UniverseSnapshot,
        event: &EventRecord,
    ) -> Result<(), UniverseError> {
        let mut candidate = self.clone();
        candidate.apply_mutation(current, &event.envelope.payload)?;
        candidate.current_revision = event.envelope.revision;
        candidate.events_applied = candidate
            .events_applied
            .checked_add(1)
            .ok_or_else(|| UniverseError::BudgetExhausted("overlay event count overflow".into()))?;
        candidate.enforce_budget()?;
        *self = candidate;
        Ok(())
    }

    fn apply_mutation(
        &mut self,
        current: &UniverseSnapshot,
        mutation: &UniverseMutation,
    ) -> Result<(), UniverseError> {
        match mutation {
            UniverseMutation::InternSymbols { .. } => {}
            UniverseMutation::PutEntity { entity } => {
                self.added_entities.insert(entity.key);
            }
            UniverseMutation::PutRelation { relation } => {
                self.remove_added_relation(relation.key);
                self.tombstones.remove(&relation.key);
                self.relation_additions
                    .insert(relation.key, relation.clone());
                self.insert_incidence(relation.source, relation.key);
                if relation.target != relation.source {
                    self.insert_incidence(relation.target, relation.key);
                }
                self.touched_entities.insert(relation.source);
                self.touched_entities.insert(relation.target);
            }
            UniverseMutation::TombstoneRelation {
                relation,
                generation,
            } => {
                let existing = self
                    .relation_additions
                    .get(relation)
                    .or_else(|| {
                        current
                            .relations
                            .iter()
                            .find(|candidate| candidate.key == *relation)
                    })
                    .ok_or_else(|| {
                        UniverseError::Validation("relation tombstone target is absent".into())
                    })?
                    .clone();
                if existing.generation != *generation {
                    return Err(UniverseError::Validation(
                        "relation tombstone generation is stale".into(),
                    ));
                }
                self.remove_added_relation(*relation);
                self.shadowed_relations.insert(*relation);
                self.tombstones.insert(*relation);
                self.touched_entities.insert(existing.source);
                self.touched_entities.insert(existing.target);
            }
            UniverseMutation::Batch { mutations } => {
                for mutation in mutations {
                    self.apply_mutation(current, mutation)?;
                }
            }
        }
        Ok(())
    }

    fn insert_incidence(&mut self, entity: EntityKey, relation: RelationKey) {
        let relations = self.adjacency_additions.entry(entity).or_default();
        match relations.binary_search(&relation) {
            Ok(_) => {}
            Err(index) => relations.insert(index, relation),
        }
    }

    fn remove_added_relation(&mut self, relation: RelationKey) {
        let Some(previous) = self.relation_additions.remove(&relation) else {
            return;
        };
        for entity in [previous.source, previous.target] {
            let should_remove = if let Some(relations) = self.adjacency_additions.get_mut(&entity) {
                if let Ok(index) = relations.binary_search(&relation) {
                    relations.remove(index);
                }
                relations.is_empty()
            } else {
                false
            };
            if should_remove {
                self.adjacency_additions.remove(&entity);
            }
        }
    }

    fn enforce_budget(&self) -> Result<(), UniverseError> {
        for (actual, maximum, label) in [
            (
                self.added_entity_count(),
                self.budget.max_added_entities,
                "added entities",
            ),
            (
                self.changed_relation_count(),
                self.budget.max_changed_relations,
                "changed relations",
            ),
            (
                self.tombstone_count(),
                self.budget.max_tombstones,
                "relation tombstones",
            ),
            (
                self.touched_entity_count(),
                self.budget.max_touched_entities,
                "touched entities",
            ),
            (self.events_applied, self.budget.max_events, "events"),
        ] {
            if actual > maximum {
                return Err(UniverseError::BudgetExhausted(format!(
                    "adjacency overlay has {actual} {label}, limit is {maximum}"
                )));
            }
        }
        Ok(())
    }
}

/// Current truth revision addressed through immutable base CSR plus a bounded
/// mutable overlay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OverlayIndexedUniverseSnapshot {
    base_relations: Vec<RelationRecord>,
    snapshot: UniverseSnapshot,
    base_adjacency: SnapshotAdjacency,
    overlay: MutableAdjacencyOverlay,
}

impl OverlayIndexedUniverseSnapshot {
    pub fn snapshot(&self) -> &UniverseSnapshot {
        &self.snapshot
    }

    pub fn base_adjacency(&self) -> &SnapshotAdjacency {
        &self.base_adjacency
    }

    pub fn overlay(&self) -> &MutableAdjacencyOverlay {
        &self.overlay
    }

    pub fn contains(&self, entity: EntityKey) -> bool {
        self.base_adjacency.contains(entity) || self.overlay.added_entities.contains(&entity)
    }

    pub fn adjacent_relations(&self, entity: EntityKey) -> OverlayAdjacentRelations<'_> {
        let mut relations = self
            .base_adjacency
            .relation_positions(entity)
            .iter()
            .filter_map(|position| self.base_relations.get(*position as usize))
            .filter(|relation| !self.overlay.shadowed_relations.contains(&relation.key))
            .collect::<Vec<_>>();
        if let Some(additions) = self.overlay.adjacency_additions.get(&entity) {
            relations.extend(
                additions
                    .iter()
                    .filter_map(|key| self.overlay.relation_additions.get(key)),
            );
        }
        relations.sort_unstable_by_key(|relation| relation.key);
        OverlayAdjacentRelations {
            inner: relations.into_iter(),
        }
    }

    /// Deterministically folds the bounded overlay into a new immutable CSR
    /// view. This does not rewrite the authoritative checkpoint or event log.
    pub fn compact(self) -> Result<IndexedUniverseSnapshot, UniverseError> {
        IndexedUniverseSnapshot::new(self.snapshot)
    }
}

pub struct OverlayAdjacentRelations<'a> {
    inner: std::vec::IntoIter<&'a RelationRecord>,
}

impl<'a> Iterator for OverlayAdjacentRelations<'a> {
    type Item = &'a RelationRecord;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for OverlayAdjacentRelations<'_> {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UniverseMutation {
    InternSymbols {
        symbols: Vec<String>,
    },
    PutEntity {
        entity: EntityRecord,
    },
    PutRelation {
        relation: RelationRecord,
    },
    TombstoneRelation {
        relation: RelationKey,
        generation: u32,
    },
    Batch {
        mutations: Vec<UniverseMutation>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    pub envelope: VersionEnvelope<UniverseMutation>,
    pub idempotency_key: String,
    pub previous_revision: Revision,
    pub checksum: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchivedEventLogReceipt {
    pub file_name: String,
    pub sha256: String,
    pub byte_len: u64,
    pub record_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckpointRolloverReceipt {
    pub universe: UniverseId,
    pub previous_checkpoint_revision: Revision,
    pub previous_checkpoint_hash: String,
    pub checkpoint_revision: Revision,
    pub checkpoint_tick: Tick,
    pub checkpoint_hash: String,
    pub applied_event_count: u64,
    pub archived_event_log: Option<ArchivedEventLogReceipt>,
}

impl EventRecord {
    pub fn new(
        universe: UniverseId,
        previous_revision: Revision,
        tick: Tick,
        idempotency_key: impl Into<String>,
        mutation: UniverseMutation,
    ) -> Result<Self, UniverseError> {
        let envelope =
            VersionEnvelope::v0(universe, Revision(previous_revision.0 + 1), tick, mutation);
        let mut record = Self {
            envelope,
            idempotency_key: idempotency_key.into(),
            previous_revision,
            checksum: String::new(),
        };
        record.checksum = canonical_hash(&(
            &record.envelope,
            &record.idempotency_key,
            record.previous_revision,
        ))?;
        Ok(record)
    }

    pub fn verify(&self) -> Result<(), UniverseError> {
        self.envelope.validate_version()?;
        let expected = canonical_hash(&(
            &self.envelope,
            &self.idempotency_key,
            self.previous_revision,
        ))?;
        if expected == self.checksum {
            Ok(())
        } else {
            Err(UniverseError::CorruptLog("event checksum mismatch".into()))
        }
    }
}

pub fn apply_event(
    snapshot: &mut UniverseSnapshot,
    event: &EventRecord,
) -> Result<bool, UniverseError> {
    event.verify()?;
    if event.envelope.universe != snapshot.universe {
        return Err(UniverseError::Validation("universe ID mismatch".into()));
    }
    if snapshot.event_keys.contains(&event.idempotency_key) {
        return Ok(false);
    }
    if event.previous_revision != snapshot.revision {
        return Err(UniverseError::RevisionConflict {
            expected: event.previous_revision,
            actual: snapshot.revision,
        });
    }
    let mut candidate = snapshot.clone();
    apply_mutation(&mut candidate, &event.envelope.payload, false)?;
    candidate.validate()?;
    candidate.revision = event.envelope.revision;
    candidate.tick = event.envelope.tick;
    candidate.event_keys.insert(event.idempotency_key.clone());
    *snapshot = candidate;
    Ok(true)
}

fn apply_mutation(
    snapshot: &mut UniverseSnapshot,
    mutation: &UniverseMutation,
    inside_batch: bool,
) -> Result<(), UniverseError> {
    match mutation {
        UniverseMutation::InternSymbols { symbols } => {
            if symbols.is_empty() {
                return Err(UniverseError::Validation(
                    "symbol intern mutation is empty".into(),
                ));
            }
            if !symbols.windows(2).all(|pair| pair[0] < pair[1]) {
                return Err(UniverseError::Validation(
                    "new symbols must be unique and lexically ordered".into(),
                ));
            }
            if symbols.iter().any(|symbol| symbol.trim().is_empty()) {
                return Err(UniverseError::Validation("empty symbol".into()));
            }
            if symbols
                .iter()
                .any(|symbol| snapshot.symbol_id(symbol).is_some())
            {
                return Err(UniverseError::Validation(
                    "symbol is already interned".into(),
                ));
            }
            let final_len = snapshot
                .symbols
                .len()
                .checked_add(symbols.len())
                .ok_or_else(|| UniverseError::Validation("symbol table size overflow".into()))?;
            if final_len > u32::MAX as usize {
                return Err(UniverseError::Validation(
                    "symbol table exceeds u32 address space".into(),
                ));
            }
            snapshot.symbols.extend(symbols.iter().cloned());
        }
        UniverseMutation::PutEntity { entity } => {
            if entity.symbol as usize >= snapshot.symbols.len() {
                return Err(UniverseError::Validation(
                    "entity references an unknown symbol".into(),
                ));
            }
            if snapshot
                .entities
                .iter()
                .any(|existing| existing.key == entity.key)
            {
                return Err(UniverseError::Validation("duplicate entity key".into()));
            }
            snapshot.entities.push(entity.clone());
            snapshot.entities.sort_by_key(|record| record.key);
        }
        UniverseMutation::PutRelation { relation } => {
            if relation.predicate as usize >= snapshot.symbols.len() {
                return Err(UniverseError::Validation(
                    "relation references an unknown predicate symbol".into(),
                ));
            }
            let endpoints = |key| snapshot.entities.iter().any(|entity| entity.key == key);
            if !endpoints(relation.source) || !endpoints(relation.target) {
                return Err(UniverseError::Validation(
                    "missing relation endpoint".into(),
                ));
            }
            if snapshot
                .relations
                .iter()
                .any(|existing| existing.key == relation.key)
            {
                return Err(UniverseError::Validation("duplicate relation key".into()));
            }
            snapshot.relations.push(relation.clone());
            snapshot.relations.sort_by_key(|record| record.key);
        }
        UniverseMutation::TombstoneRelation {
            relation,
            generation,
        } => {
            let index = snapshot
                .relations
                .iter()
                .position(|existing| existing.key == *relation)
                .ok_or_else(|| {
                    UniverseError::Validation("relation tombstone target is absent".into())
                })?;
            if snapshot.relations[index].generation != *generation {
                return Err(UniverseError::Validation(
                    "relation tombstone generation is stale".into(),
                ));
            }
            snapshot.relations.remove(index);
        }
        UniverseMutation::Batch { mutations } => {
            if inside_batch {
                return Err(UniverseError::Validation(
                    "nested mutation batch is forbidden".into(),
                ));
            }
            if mutations.is_empty() {
                return Err(UniverseError::Validation("mutation batch is empty".into()));
            }
            if mutations.len() > MAX_EVENT_MUTATIONS {
                return Err(UniverseError::BudgetExhausted(format!(
                    "mutation batch has {} entries, limit is {}",
                    mutations.len(),
                    MAX_EVENT_MUTATIONS
                )));
            }
            for mutation in mutations {
                apply_mutation(snapshot, mutation, true)?;
            }
        }
    }
    Ok(())
}

pub struct UniverseStore {
    root: PathBuf,
}

#[derive(Debug)]
struct EventLogRead {
    bytes: Vec<u8>,
    events: Vec<EventRecord>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RolloverStop {
    Never,
    #[cfg(test)]
    BeforeCheckpoint,
    #[cfg(test)]
    AfterCheckpoint,
    #[cfg(test)]
    AfterLogArchive,
}

impl UniverseStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, UniverseError> {
        fs::create_dir_all(root.as_ref()).map_err(io_error)?;
        Ok(Self {
            root: root.as_ref().to_owned(),
        })
    }

    pub fn checkpoint(&self, snapshot: &UniverseSnapshot) -> Result<(), UniverseError> {
        snapshot.validate()?;
        let bytes = serde_json::to_vec(snapshot).map_err(json_error)?;
        let temp = self.root.join("snapshot.json.tmp");
        let final_path = self.root.join("snapshot.json");
        let mut file = File::create(&temp).map_err(io_error)?;
        file.write_all(&bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        fs::rename(temp, final_path).map_err(io_error)
    }

    pub fn load_snapshot(&self) -> Result<UniverseSnapshot, UniverseError> {
        let Some((path, expected_revision, expected_hash)) = self.latest_versioned_checkpoint()?
        else {
            let bytes = fs::read(self.root.join("snapshot.json")).map_err(io_error)?;
            let snapshot: UniverseSnapshot = serde_json::from_slice(&bytes).map_err(json_error)?;
            snapshot.validate()?;
            return Ok(snapshot);
        };
        let bytes = fs::read(path).map_err(io_error)?;
        let snapshot: UniverseSnapshot = serde_json::from_slice(&bytes).map_err(json_error)?;
        snapshot.validate()?;
        if snapshot.revision != expected_revision {
            return Err(UniverseError::CorruptContent(format!(
                "checkpoint filename revision {:?} differs from snapshot revision {:?}",
                expected_revision, snapshot.revision
            )));
        }
        let actual_hash = snapshot.canonical_hash()?;
        if actual_hash != expected_hash {
            return Err(UniverseError::CorruptContent(
                "checkpoint filename hash differs from snapshot hash".into(),
            ));
        }
        Ok(snapshot)
    }

    /// Folds a verified bounded overlay into a durable immutable checkpoint,
    /// then archives the incorporated active log.
    ///
    /// This is a mono-writer operation. The caller must serialize it with event
    /// appends. Defensive log-byte revalidation still prevents a detected
    /// concurrent append from being archived by this call.
    pub fn rollover_checkpoint(
        &self,
        view: &OverlayIndexedUniverseSnapshot,
    ) -> Result<CheckpointRolloverReceipt, UniverseError> {
        self.rollover_checkpoint_inner(view, RolloverStop::Never)
    }

    /// Loads the committed snapshot, replays its valid log, and constructs the
    /// direct immutable adjacency for that exact resulting revision.
    pub fn load_current_indexed(&self) -> Result<IndexedUniverseSnapshot, UniverseError> {
        let current = self.replay(self.load_snapshot()?)?;
        IndexedUniverseSnapshot::new(current)
    }

    /// Loads one immutable checkpoint CSR and replays subsequent valid events
    /// into a bounded mutable overlay.
    pub fn load_current_overlay_indexed(
        &self,
        budget: AdjacencyOverlayBudget,
    ) -> Result<OverlayIndexedUniverseSnapshot, UniverseError> {
        let base = self.load_snapshot()?;
        let base_adjacency = SnapshotAdjacency::build(&base)?;
        let base_relations = base.relations.clone();
        let mut current = base;
        let mut overlay = MutableAdjacencyOverlay::new(&base_adjacency, budget);
        for event in self.read_event_records()? {
            let already_applied = current.event_keys.contains(&event.idempotency_key);
            if !already_applied {
                overlay.apply_event(&current, &event)?;
            }
            apply_event(&mut current, &event)?;
        }
        if overlay.base_universe != current.universe {
            return Err(UniverseError::Validation(
                "adjacency overlay Universe differs from replay result".into(),
            ));
        }
        if overlay.current_revision != current.revision {
            return Err(UniverseError::Validation(
                "adjacency overlay revision differs from replay result".into(),
            ));
        }
        Ok(OverlayIndexedUniverseSnapshot {
            base_relations,
            snapshot: current,
            base_adjacency,
            overlay,
        })
    }

    pub fn append_event(&self, event: &EventRecord) -> Result<(), UniverseError> {
        event.verify()?;
        let mut line = serde_json::to_vec(event).map_err(json_error)?;
        line.push(b'\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join(ACTIVE_EVENT_LOG))
            .map_err(io_error)?;
        file.write_all(&line).map_err(io_error)?;
        file.sync_data().map_err(io_error)
    }

    /// Replays valid records and truncates only an incomplete final record.
    pub fn replay(
        &self,
        mut snapshot: UniverseSnapshot,
    ) -> Result<UniverseSnapshot, UniverseError> {
        for event in self.read_event_records()? {
            apply_event(&mut snapshot, &event)?;
        }
        Ok(snapshot)
    }

    fn read_event_records(&self) -> Result<Vec<EventRecord>, UniverseError> {
        Ok(self.read_event_log()?.events)
    }

    fn read_event_log(&self) -> Result<EventLogRead, UniverseError> {
        let path = self.root.join(ACTIVE_EVENT_LOG);
        if !path.exists() {
            return Ok(EventLogRead {
                bytes: Vec::new(),
                events: Vec::new(),
            });
        }
        let bytes = fs::read(&path).map_err(io_error)?;
        let complete_len = bytes
            .iter()
            .rposition(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut events = Vec::new();
        for line in bytes[..complete_len]
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
        {
            let event: EventRecord = serde_json::from_slice(line).map_err(|e| {
                UniverseError::CorruptLog(format!("invalid complete event record: {e}"))
            })?;
            events.push(event);
        }
        if complete_len != bytes.len() {
            let file = OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(io_error)?;
            file.set_len(complete_len as u64).map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
        }
        Ok(EventLogRead {
            bytes: bytes[..complete_len].to_vec(),
            events,
        })
    }

    fn rollover_checkpoint_inner(
        &self,
        view: &OverlayIndexedUniverseSnapshot,
        stop: RolloverStop,
    ) -> Result<CheckpointRolloverReceipt, UniverseError> {
        view.snapshot.validate()?;
        let base = self.load_snapshot()?;
        let rebuilt_base_adjacency = SnapshotAdjacency::build(&base)?;
        if rebuilt_base_adjacency != view.base_adjacency || view.base_relations != base.relations {
            return Err(UniverseError::Validation(
                "checkpoint rollover refused a stale base adjacency".into(),
            ));
        }
        if view.overlay.base_universe != base.universe
            || view.overlay.base_revision != base.revision
            || view.overlay.base_snapshot_hash != view.base_adjacency.snapshot_hash
            || view.overlay.current_revision != view.snapshot.revision
        {
            return Err(UniverseError::Validation(
                "checkpoint rollover view has inconsistent index metadata".into(),
            ));
        }

        let previous_checkpoint_hash = base.canonical_hash()?;
        let log = self.read_event_log()?;
        let mut independently_replayed = base.clone();
        let mut applied_event_count = 0u64;
        for event in &log.events {
            if apply_event(&mut independently_replayed, event)? {
                applied_event_count = applied_event_count.checked_add(1).ok_or_else(|| {
                    UniverseError::BudgetExhausted(
                        "checkpoint rollover event count overflow".into(),
                    )
                })?;
            }
        }
        let expected_hash = independently_replayed.canonical_hash()?;
        let view_hash = view.snapshot.canonical_hash()?;
        if independently_replayed.universe != view.snapshot.universe
            || independently_replayed.revision != view.snapshot.revision
            || expected_hash != view_hash
        {
            return Err(UniverseError::Validation(
                "checkpoint rollover refused a stale current view".into(),
            ));
        }

        let compacted = IndexedUniverseSnapshot::new(view.snapshot.clone())?;
        compacted
            .adjacency()
            .validate_against(compacted.snapshot())?;

        #[cfg(test)]
        if stop == RolloverStop::BeforeCheckpoint {
            return Err(UniverseError::Cancelled);
        }

        self.write_versioned_checkpoint(compacted.snapshot(), &view_hash)?;

        #[cfg(test)]
        if stop == RolloverStop::AfterCheckpoint {
            return Err(UniverseError::Cancelled);
        }

        let archived_event_log =
            self.archive_active_event_log(&log, compacted.snapshot().revision)?;

        #[cfg(test)]
        if stop == RolloverStop::AfterLogArchive {
            return Err(UniverseError::Cancelled);
        }

        let _ = stop;
        Ok(CheckpointRolloverReceipt {
            universe: compacted.snapshot().universe,
            previous_checkpoint_revision: base.revision,
            previous_checkpoint_hash,
            checkpoint_revision: compacted.snapshot().revision,
            checkpoint_tick: compacted.snapshot().tick,
            checkpoint_hash: view_hash,
            applied_event_count,
            archived_event_log,
        })
    }

    fn write_versioned_checkpoint(
        &self,
        snapshot: &UniverseSnapshot,
        snapshot_hash: &str,
    ) -> Result<(), UniverseError> {
        let file_name = versioned_checkpoint_file_name(snapshot.revision, snapshot_hash);
        let final_path = self.root.join(&file_name);
        let bytes = serde_json::to_vec(snapshot).map_err(json_error)?;
        if final_path.exists() {
            let existing = fs::read(&final_path).map_err(io_error)?;
            if existing == bytes {
                return Ok(());
            }
            return Err(UniverseError::CorruptContent(format!(
                "immutable checkpoint collision for {file_name}"
            )));
        }

        let temp_path = self.root.join(format!("{file_name}.tmp"));
        let mut file = File::create(&temp_path).map_err(io_error)?;
        file.write_all(&bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        fs::rename(&temp_path, &final_path).map_err(io_error)
    }

    fn latest_versioned_checkpoint(
        &self,
    ) -> Result<Option<(PathBuf, Revision, String)>, UniverseError> {
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if !file_name.starts_with(VERSIONED_CHECKPOINT_PREFIX) {
                continue;
            }
            if file_name.ends_with(".tmp") {
                continue;
            }
            let (revision, hash) = parse_versioned_checkpoint_file_name(file_name)?;
            candidates.push((entry.path(), revision, hash));
        }
        let Some(highest_revision) = candidates.iter().map(|(_, revision, _)| *revision).max()
        else {
            return Ok(None);
        };
        let mut highest = candidates
            .into_iter()
            .filter(|(_, revision, _)| *revision == highest_revision);
        let selected = highest.next().expect("highest revision has a candidate");
        if highest.next().is_some() {
            return Err(UniverseError::CorruptContent(format!(
                "multiple immutable checkpoints claim revision {:?}",
                highest_revision
            )));
        }
        Ok(Some(selected))
    }

    fn archive_active_event_log(
        &self,
        expected: &EventLogRead,
        checkpoint_revision: Revision,
    ) -> Result<Option<ArchivedEventLogReceipt>, UniverseError> {
        if expected.bytes.is_empty() {
            return Ok(None);
        }
        let active_path = self.root.join(ACTIVE_EVENT_LOG);
        let actual = fs::read(&active_path).map_err(io_error)?;
        if actual != expected.bytes {
            return Err(UniverseError::Validation(
                "active event log changed during checkpoint rollover".into(),
            ));
        }
        let log_hash = hex::encode(Sha256::digest(&expected.bytes));
        let stem = format!("events-through-r{:020}-{}", checkpoint_revision.0, log_hash);
        let mut collision = 0u64;
        let (archive_path, archive_file_name) = loop {
            let file_name = if collision == 0 {
                format!("{stem}.jsonl")
            } else {
                format!("{stem}-{collision}.jsonl")
            };
            let path = self.root.join(&file_name);
            if !path.exists() {
                break (path, file_name);
            }
            collision = collision.checked_add(1).ok_or_else(|| {
                UniverseError::BudgetExhausted("event archive name space exhausted".into())
            })?;
        };
        fs::rename(&active_path, &archive_path).map_err(io_error)?;
        OpenOptions::new()
            .write(true)
            .open(&archive_path)
            .map_err(io_error)?
            .sync_all()
            .map_err(io_error)?;
        Ok(Some(ArchivedEventLogReceipt {
            file_name: archive_file_name,
            sha256: log_hash,
            byte_len: u64::try_from(expected.bytes.len()).map_err(|_| {
                UniverseError::BudgetExhausted("event log byte length exceeds u64".into())
            })?,
            record_count: u64::try_from(expected.events.len()).map_err(|_| {
                UniverseError::BudgetExhausted("event log record count exceeds u64".into())
            })?,
        }))
    }

    pub fn append_content(&self, value: &serde_json::Value) -> Result<ContentRef, UniverseError> {
        let path = self.root.join("content-0.jsonl");
        let mut line = serde_json::to_vec(value).map_err(json_error)?;
        line.push(b'\n');
        let offset = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(io_error)?;
        file.write_all(&line).map_err(io_error)?;
        file.sync_data().map_err(io_error)?;
        let hash = hex::encode(Sha256::digest(&line));
        Ok(ContentRef {
            pointer: ContentPtr {
                segment: 0,
                offset,
                length: line.len() as u32,
            },
            sha256: hash,
        })
    }

    pub fn read_content(&self, content: &ContentRef) -> Result<serde_json::Value, UniverseError> {
        content.validate()?;
        let ptr = content.pointer;
        if ptr.segment != 0 {
            return Err(UniverseError::CorruptContent(
                "unknown content segment".into(),
            ));
        }
        let mut file = File::open(self.root.join("content-0.jsonl")).map_err(io_error)?;
        file.seek(SeekFrom::Start(ptr.offset)).map_err(io_error)?;
        let mut bytes = vec![0; ptr.length as usize];
        file.read_exact(&mut bytes).map_err(io_error)?;
        if bytes.last() != Some(&b'\n') {
            return Err(UniverseError::CorruptContent(
                "truncated content record".into(),
            ));
        }
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != content.sha256 {
            return Err(UniverseError::CorruptContent(
                "content hash mismatch".into(),
            ));
        }
        serde_json::from_slice(&bytes[..bytes.len() - 1]).map_err(json_error)
    }

    /// Installs an inline graph seed into an empty store. Inline JSON is only a
    /// bootstrap transport: after this call, records point directly into the
    /// durable content segment and the snapshot is authoritative.
    pub fn install_seed(&self, seed: &GraphSeed) -> Result<UniverseSnapshot, UniverseError> {
        seed.validate()?;
        for name in ["snapshot.json", "events.jsonl", "content-0.jsonl"] {
            let path = self.root.join(name);
            if path.exists() && fs::metadata(&path).map_err(io_error)?.len() > 0 {
                return Err(UniverseError::Validation(format!(
                    "cannot install seed into non-empty store: {name}"
                )));
            }
        }

        let symbols: BTreeMap<_, _> = seed
            .symbols
            .iter()
            .enumerate()
            .map(|(index, symbol)| (symbol.as_str(), index as u32))
            .collect();
        let mut entities = Vec::with_capacity(seed.entities.len());
        for entity in &seed.entities {
            entities.push(EntityRecord {
                key: entity.key,
                generation: entity.generation,
                symbol: symbols[entity.symbol.as_str()],
                content: Some(self.append_content(&entity.content)?),
            });
        }
        entities.sort_by_key(|entity| entity.key);

        let mut relations = Vec::with_capacity(seed.relations.len());
        for relation in &seed.relations {
            relations.push(RelationRecord {
                key: relation.key,
                generation: relation.generation,
                source: relation.source,
                target: relation.target,
                predicate: symbols[relation.predicate.as_str()],
                content: relation
                    .content
                    .as_ref()
                    .map(|content| self.append_content(content))
                    .transpose()?,
            });
        }
        relations.sort_by_key(|relation| relation.key);

        let snapshot = UniverseSnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            universe: seed.universe,
            revision: Revision(0),
            tick: Tick(0),
            symbols: seed.symbols.clone(),
            entities,
            relations,
            event_keys: BTreeSet::new(),
        };
        snapshot.validate()?;
        self.checkpoint(&snapshot)?;
        Ok(snapshot)
    }
}

pub fn load_genesis(path: impl AsRef<Path>) -> Result<UniverseSnapshot, UniverseError> {
    let bytes = fs::read(path).map_err(io_error)?;
    let envelope: GenesisEnvelope = serde_json::from_slice(&bytes).map_err(json_error)?;
    if envelope.contract != "mind-universe-genesis" || envelope.version != 0 {
        return Err(UniverseError::UnsupportedVersion(envelope.version));
    }
    let expected = canonical_hash(&envelope.snapshot)?;
    if expected != envelope.sha256 {
        return Err(UniverseError::CorruptContent(
            "Genesis hash mismatch".into(),
        ));
    }
    envelope.snapshot.validate()?;
    Ok(envelope.snapshot)
}

pub fn load_seed(path: impl AsRef<Path>) -> Result<GraphSeed, UniverseError> {
    let bytes = fs::read(path).map_err(io_error)?;
    let envelope: GraphSeedEnvelope = serde_json::from_slice(&bytes).map_err(json_error)?;
    if envelope.contract != "mind-universe-graph-seed" || envelope.version != SEED_FORMAT_VERSION {
        return Err(UniverseError::UnsupportedVersion(envelope.version));
    }
    let expected = canonical_hash(&envelope.seed)?;
    if expected != envelope.sha256 {
        return Err(UniverseError::CorruptContent(
            "graph seed hash mismatch".into(),
        ));
    }
    envelope.seed.validate()?;
    Ok(envelope.seed)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenesisEnvelope {
    pub contract: String,
    pub version: u16,
    pub sha256: String,
    pub snapshot: UniverseSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GraphSeedEnvelope {
    pub contract: String,
    pub version: u16,
    pub sha256: String,
    pub seed: GraphSeed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GraphSeed {
    pub universe: UniverseId,
    pub symbols: Vec<String>,
    pub entities: Vec<SeedEntity>,
    pub relations: Vec<SeedRelation>,
}

impl GraphSeed {
    pub fn validate(&self) -> Result<(), UniverseError> {
        let symbols: BTreeSet<_> = self.symbols.iter().map(String::as_str).collect();
        if symbols.len() != self.symbols.len() {
            return Err(UniverseError::Validation(
                "graph seed contains duplicate symbols".into(),
            ));
        }
        if self
            .entities
            .iter()
            .any(|entity| !symbols.contains(entity.symbol.as_str()))
            || self
                .relations
                .iter()
                .any(|relation| !symbols.contains(relation.predicate.as_str()))
        {
            return Err(UniverseError::Validation(
                "graph seed references an undeclared symbol".into(),
            ));
        }
        let entities: BTreeSet<_> = self.entities.iter().map(|entity| entity.key).collect();
        if entities.len() != self.entities.len() {
            return Err(UniverseError::Validation(
                "graph seed contains duplicate entity keys".into(),
            ));
        }
        let relations: BTreeSet<_> = self.relations.iter().map(|relation| relation.key).collect();
        if relations.len() != self.relations.len() {
            return Err(UniverseError::Validation(
                "graph seed contains duplicate relation keys".into(),
            ));
        }
        if self.relations.iter().any(|relation| {
            !entities.contains(&relation.source) || !entities.contains(&relation.target)
        }) {
            return Err(UniverseError::Validation(
                "graph seed relation endpoint does not exist".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SeedEntity {
    pub key: EntityKey,
    #[serde(default)]
    pub generation: u32,
    pub symbol: String,
    pub content: serde_json::Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SeedRelation {
    pub key: RelationKey,
    #[serde(default)]
    pub generation: u32,
    pub source: EntityKey,
    pub target: EntityKey,
    pub predicate: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
}

fn versioned_checkpoint_file_name(revision: Revision, snapshot_hash: &str) -> String {
    format!(
        "{VERSIONED_CHECKPOINT_PREFIX}{:020}-{snapshot_hash}{VERSIONED_CHECKPOINT_SUFFIX}",
        revision.0
    )
}

fn parse_versioned_checkpoint_file_name(
    file_name: &str,
) -> Result<(Revision, String), UniverseError> {
    let body = file_name
        .strip_prefix(VERSIONED_CHECKPOINT_PREFIX)
        .and_then(|name| name.strip_suffix(VERSIONED_CHECKPOINT_SUFFIX))
        .ok_or_else(|| {
            UniverseError::CorruptContent(format!(
                "invalid immutable checkpoint filename {file_name}"
            ))
        })?;
    let (revision, hash) = body.split_once('-').ok_or_else(|| {
        UniverseError::CorruptContent(format!("invalid immutable checkpoint filename {file_name}"))
    })?;
    let revision = revision.parse::<u64>().map_err(|_| {
        UniverseError::CorruptContent(format!(
            "invalid immutable checkpoint revision in {file_name}"
        ))
    })?;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(UniverseError::CorruptContent(format!(
            "invalid immutable checkpoint hash in {file_name}"
        )));
    }
    Ok((Revision(revision), hash.to_owned()))
}

pub fn canonical_hash<T: Serialize>(value: &T) -> Result<String, UniverseError> {
    let bytes = serde_json::to_vec(value).map_err(json_error)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn io_error(error: std::io::Error) -> UniverseError {
    UniverseError::Io(error.to_string())
}

fn json_error(error: serde_json::Error) -> UniverseError {
    UniverseError::CorruptContent(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rollover_fixture(
        root: &Path,
    ) -> (
        UniverseStore,
        OverlayIndexedUniverseSnapshot,
        String,
        Vec<u8>,
    ) {
        let store = UniverseStore::open(root).unwrap();
        let mut base = UniverseSnapshot::empty(UniverseId(21));
        base.symbols.push("thing".into());
        store.checkpoint(&base).unwrap();
        for (previous_revision, tick, key) in
            [(Revision(0), Tick(1), 1u128), (Revision(1), Tick(2), 2u128)]
        {
            let event = EventRecord::new(
                base.universe,
                previous_revision,
                tick,
                format!("entity-{key}"),
                UniverseMutation::PutEntity {
                    entity: EntityRecord {
                        key: EntityKey(key),
                        generation: 0,
                        symbol: 0,
                        content: None,
                    },
                },
            )
            .unwrap();
            store.append_event(&event).unwrap();
        }
        let view = store
            .load_current_overlay_indexed(AdjacencyOverlayBudget::default())
            .unwrap();
        let current_hash = view.snapshot().canonical_hash().unwrap();
        let active_log = fs::read(root.join(ACTIVE_EVENT_LOG)).unwrap();
        (store, view, current_hash, active_log)
    }

    fn versioned_checkpoint_paths(root: &Path) -> Vec<PathBuf> {
        let mut paths = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(VERSIONED_CHECKPOINT_PREFIX)
                            && name.ends_with(VERSIONED_CHECKPOINT_SUFFIX)
                    })
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    #[test]
    fn boot_mutate_checkpoint_crash_replay_equivalence() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let mut base = UniverseSnapshot::empty(UniverseId(7));
        base.symbols.push("thing".into());
        store.checkpoint(&base).unwrap();
        let event = EventRecord::new(
            base.universe,
            base.revision,
            Tick(1),
            "entity-1",
            UniverseMutation::PutEntity {
                entity: EntityRecord {
                    key: EntityKey(1),
                    generation: 0,
                    symbol: 0,
                    content: None,
                },
            },
        )
        .unwrap();
        store.append_event(&event).unwrap();
        let recovered = store.replay(store.load_snapshot().unwrap()).unwrap();
        let mut expected = base;
        apply_event(&mut expected, &event).unwrap();
        assert_eq!(recovered, expected);
        assert_eq!(
            recovered.canonical_hash().unwrap(),
            expected.canonical_hash().unwrap()
        );
    }

    #[test]
    fn truncated_final_event_is_removed_but_complete_events_survive() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let mut base = UniverseSnapshot::empty(UniverseId(7));
        base.symbols.push("thing".into());
        store.checkpoint(&base).unwrap();
        let event = EventRecord::new(
            base.universe,
            base.revision,
            Tick(1),
            "entity-1",
            UniverseMutation::PutEntity {
                entity: EntityRecord {
                    key: EntityKey(1),
                    generation: 0,
                    symbol: 0,
                    content: None,
                },
            },
        )
        .unwrap();
        store.append_event(&event).unwrap();
        OpenOptions::new()
            .append(true)
            .open(temp.path().join("events.jsonl"))
            .unwrap()
            .write_all(br#"{"truncated":"#)
            .unwrap();
        let recovered = store.replay(store.load_snapshot().unwrap()).unwrap();
        assert_eq!(recovered.revision, Revision(1));
        assert!(fs::read(temp.path().join("events.jsonl"))
            .unwrap()
            .ends_with(b"\n"));
    }

    #[test]
    fn content_is_read_directly_by_pointer() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let value = serde_json::json!({"kind":"generic_graph_data"});
        let content = store.append_content(&value).unwrap();
        assert_eq!(store.read_content(&content).unwrap(), value);
    }

    #[test]
    fn content_hash_detects_in_place_corruption() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let content = store
            .append_content(&serde_json::json!({"value":"expected"}))
            .unwrap();
        let path = temp.path().join("content-0.jsonl");
        let mut bytes = fs::read(&path).unwrap();
        let index = bytes.iter().position(|byte| *byte == b'e').unwrap();
        bytes[index] = b'x';
        fs::write(path, bytes).unwrap();
        assert!(matches!(
            store.read_content(&content),
            Err(UniverseError::CorruptContent(message)) if message == "content hash mismatch"
        ));
    }

    #[test]
    fn inline_seed_becomes_pointer_backed_authoritative_store() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let seed = GraphSeed {
            universe: UniverseId(11),
            symbols: vec!["claim".into(), "SUPPORTS_ESTIMATE".into()],
            entities: vec![
                SeedEntity {
                    key: EntityKey(1),
                    generation: 0,
                    symbol: "claim".into(),
                    content: serde_json::json!({"claim":"premise"}),
                },
                SeedEntity {
                    key: EntityKey(2),
                    generation: 0,
                    symbol: "claim".into(),
                    content: serde_json::json!({"claim":"conclusion"}),
                },
            ],
            relations: vec![SeedRelation {
                key: RelationKey(1),
                generation: 0,
                source: EntityKey(1),
                target: EntityKey(2),
                predicate: "SUPPORTS_ESTIMATE".into(),
                content: Some(serde_json::json!({"kind":"bond"})),
            }],
        };

        let installed = store.install_seed(&seed).unwrap();
        let independent = UniverseStore::open(temp.path())
            .unwrap()
            .load_snapshot()
            .unwrap();
        assert_eq!(installed, independent);
        assert_eq!(
            store
                .read_content(independent.entities[0].content.as_ref().unwrap())
                .unwrap(),
            serde_json::json!({"claim":"premise"})
        );
        assert_eq!(
            store
                .read_content(independent.relations[0].content.as_ref().unwrap())
                .unwrap(),
            serde_json::json!({"kind":"bond"})
        );
    }

    #[test]
    fn current_snapshot_exposes_deterministic_csr_adjacency_after_replay() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let seed = GraphSeed {
            universe: UniverseId(13),
            symbols: vec!["thing".into(), "LINK".into()],
            entities: [3, 1, 2]
                .into_iter()
                .map(|key| SeedEntity {
                    key: EntityKey(key),
                    generation: 0,
                    symbol: "thing".into(),
                    content: serde_json::json!({"key": key}),
                })
                .collect(),
            relations: vec![
                SeedRelation {
                    key: RelationKey(2),
                    generation: 0,
                    source: EntityKey(1),
                    target: EntityKey(3),
                    predicate: "LINK".into(),
                    content: None,
                },
                SeedRelation {
                    key: RelationKey(1),
                    generation: 0,
                    source: EntityKey(1),
                    target: EntityKey(2),
                    predicate: "LINK".into(),
                    content: None,
                },
            ],
        };
        let installed = store.install_seed(&seed).unwrap();
        let event = EventRecord::new(
            installed.universe,
            installed.revision,
            Tick(1),
            "self-link",
            UniverseMutation::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(3),
                    generation: 0,
                    source: EntityKey(2),
                    target: EntityKey(2),
                    predicate: installed.symbol_id("LINK").unwrap(),
                    content: None,
                },
            },
        )
        .unwrap();
        store.append_event(&event).unwrap();

        let indexed = UniverseStore::open(temp.path())
            .unwrap()
            .load_current_indexed()
            .unwrap();
        assert_eq!(indexed.snapshot().revision, Revision(1));
        assert_eq!(indexed.adjacency().revision(), Revision(1));
        assert_eq!(indexed.adjacency().entity_count(), 3);
        assert_eq!(indexed.adjacency().incidence_count(), 5);
        assert_eq!(
            indexed
                .adjacent_relations(EntityKey(1))
                .map(|relation| relation.key)
                .collect::<Vec<_>>(),
            vec![RelationKey(1), RelationKey(2)]
        );
        assert_eq!(
            indexed
                .adjacent_relations(EntityKey(2))
                .map(|relation| relation.key)
                .collect::<Vec<_>>(),
            vec![RelationKey(1), RelationKey(3)]
        );
        assert!(indexed.adjacent_relations(EntityKey(99)).next().is_none());
        indexed
            .adjacency()
            .validate_against(indexed.snapshot())
            .unwrap();
        let mut later_revision = indexed.snapshot().clone();
        later_revision.revision = Revision(2);
        assert!(matches!(
            indexed.adjacency().validate_against(&later_revision),
            Err(UniverseError::Validation(message))
                if message == "CSR adjacency does not match the authoritative snapshot"
        ));
    }

    #[test]
    fn bounded_overlay_applies_tombstones_and_compacts_without_changing_truth() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let seed = GraphSeed {
            universe: UniverseId(14),
            symbols: vec!["thing".into(), "LINK".into()],
            entities: (1..=3)
                .map(|key| SeedEntity {
                    key: EntityKey(key),
                    generation: 0,
                    symbol: "thing".into(),
                    content: serde_json::json!({"key": key}),
                })
                .collect(),
            relations: vec![
                SeedRelation {
                    key: RelationKey(1),
                    generation: 0,
                    source: EntityKey(1),
                    target: EntityKey(2),
                    predicate: "LINK".into(),
                    content: None,
                },
                SeedRelation {
                    key: RelationKey(2),
                    generation: 0,
                    source: EntityKey(1),
                    target: EntityKey(3),
                    predicate: "LINK".into(),
                    content: None,
                },
            ],
        };
        let installed = store.install_seed(&seed).unwrap();
        let tombstone = EventRecord::new(
            installed.universe,
            Revision(0),
            Tick(1),
            "remove-relation-1",
            UniverseMutation::TombstoneRelation {
                relation: RelationKey(1),
                generation: 0,
            },
        )
        .unwrap();
        store.append_event(&tombstone).unwrap();
        let addition = EventRecord::new(
            installed.universe,
            Revision(1),
            Tick(2),
            "add-relation-3",
            UniverseMutation::PutRelation {
                relation: RelationRecord {
                    key: RelationKey(3),
                    generation: 0,
                    source: EntityKey(2),
                    target: EntityKey(3),
                    predicate: installed.symbol_id("LINK").unwrap(),
                    content: None,
                },
            },
        )
        .unwrap();
        store.append_event(&addition).unwrap();

        let overlay = store
            .load_current_overlay_indexed(AdjacencyOverlayBudget::default())
            .unwrap();
        assert_eq!(overlay.overlay().base_revision(), Revision(0));
        assert_eq!(overlay.overlay().current_revision(), Revision(2));
        assert_eq!(overlay.overlay().changed_relation_count(), 2);
        assert_eq!(overlay.overlay().relation_addition_count(), 1);
        assert_eq!(overlay.overlay().tombstone_count(), 1);
        assert_eq!(overlay.overlay().touched_entity_count(), 3);
        assert_eq!(overlay.overlay().events_applied(), 2);
        assert_eq!(
            overlay
                .adjacent_relations(EntityKey(1))
                .map(|relation| relation.key)
                .collect::<Vec<_>>(),
            vec![RelationKey(2)]
        );
        assert_eq!(
            overlay
                .adjacent_relations(EntityKey(2))
                .map(|relation| relation.key)
                .collect::<Vec<_>>(),
            vec![RelationKey(3)]
        );
        assert!(!overlay
            .snapshot()
            .relations
            .iter()
            .any(|relation| relation.key == RelationKey(1)));

        let current_hash = overlay.snapshot().canonical_hash().unwrap();
        let compacted = overlay.clone().compact().unwrap();
        assert_eq!(compacted.snapshot().canonical_hash().unwrap(), current_hash);
        assert_eq!(
            compacted
                .adjacent_relations(EntityKey(2))
                .map(|relation| relation.key)
                .collect::<Vec<_>>(),
            vec![RelationKey(3)]
        );

        let error = store
            .load_current_overlay_indexed(AdjacencyOverlayBudget {
                max_changed_relations: 1,
                ..AdjacencyOverlayBudget::default()
            })
            .unwrap_err();
        assert!(matches!(
            error,
            UniverseError::BudgetExhausted(message)
                if message == "adjacency overlay has 2 changed relations, limit is 1"
        ));
    }

    #[test]
    fn symbol_plan_is_order_independent_and_keeps_existing_ids() {
        let mut snapshot = UniverseSnapshot::empty(UniverseId(11));
        snapshot.symbols = vec!["thing".into(), "PART_OF".into()];
        let first = snapshot
            .plan_symbol_interning(&["logic_role".into(), "thing".into(), "behavior_bond".into()])
            .unwrap();
        let second = snapshot
            .plan_symbol_interning(&["behavior_bond".into(), "logic_role".into(), "thing".into()])
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.additions, vec!["behavior_bond", "logic_role"]);
        assert_eq!(first.assignments["thing"], 0);
        assert_eq!(first.assignments["behavior_bond"], 2);
        assert_eq!(first.assignments["logic_role"], 3);
    }

    #[test]
    fn invalid_batch_does_not_publish_its_valid_prefix() {
        let mut snapshot = UniverseSnapshot::empty(UniverseId(12));
        snapshot.symbols.push("thing".into());
        let before = snapshot.clone();
        let event = EventRecord::new(
            snapshot.universe,
            snapshot.revision,
            Tick(1),
            "invalid-batch",
            UniverseMutation::Batch {
                mutations: vec![
                    UniverseMutation::PutEntity {
                        entity: EntityRecord {
                            key: EntityKey(1),
                            generation: 0,
                            symbol: 0,
                            content: None,
                        },
                    },
                    UniverseMutation::PutRelation {
                        relation: RelationRecord {
                            key: RelationKey(1),
                            generation: 0,
                            source: EntityKey(1),
                            target: EntityKey(2),
                            predicate: 0,
                            content: None,
                        },
                    },
                ],
            },
        )
        .unwrap();
        assert!(matches!(
            apply_event(&mut snapshot, &event),
            Err(UniverseError::Validation(message)) if message == "missing relation endpoint"
        ));
        assert_eq!(snapshot, before);
    }

    #[test]
    fn rollover_compacts_truth_and_reopens_with_an_empty_overlay() {
        let temp = tempfile::tempdir().unwrap();
        let (store, view, before_hash, active_log) = rollover_fixture(temp.path());
        assert_eq!(view.base_adjacency().revision(), Revision(0));
        assert_eq!(view.overlay().current_revision(), Revision(2));
        assert_eq!(view.overlay().events_applied(), 2);
        assert_eq!(view.snapshot().event_keys.len(), 2);
        let compacted = view.clone().compact().unwrap();
        assert_eq!(compacted.snapshot().canonical_hash().unwrap(), before_hash);

        let receipt = store.rollover_checkpoint(&view).unwrap();
        assert_eq!(receipt.universe, UniverseId(21));
        assert_eq!(receipt.previous_checkpoint_revision, Revision(0));
        assert_eq!(receipt.checkpoint_revision, Revision(2));
        assert_eq!(receipt.checkpoint_tick, Tick(2));
        assert_eq!(receipt.checkpoint_hash, before_hash);
        assert_eq!(receipt.applied_event_count, 2);
        let archive = receipt.archived_event_log.unwrap();
        assert_eq!(archive.record_count, 2);
        assert_eq!(archive.byte_len, active_log.len() as u64);
        assert_eq!(
            fs::read(temp.path().join(&archive.file_name)).unwrap(),
            active_log
        );
        assert!(!temp.path().join(ACTIVE_EVENT_LOG).exists());

        let independent = UniverseStore::open(temp.path()).unwrap();
        let reopened = independent
            .load_current_overlay_indexed(AdjacencyOverlayBudget::default())
            .unwrap();
        assert_eq!(reopened.snapshot().canonical_hash().unwrap(), before_hash);
        assert_eq!(reopened.base_adjacency().revision(), Revision(2));
        assert_eq!(reopened.overlay().current_revision(), Revision(2));
        assert_eq!(reopened.overlay().events_applied(), 0);
        assert!(reopened.overlay().is_empty());
        reopened
            .base_adjacency()
            .validate_against(reopened.snapshot())
            .unwrap();
    }

    #[test]
    fn every_rollover_crash_window_reopens_without_loss_or_duplication() {
        for (
            stop,
            expected_base_revision,
            expected_overlay_events,
            active_log_exists,
            expected_checkpoint_count,
            expected_archive_count,
        ) in [
            (RolloverStop::BeforeCheckpoint, Revision(0), 2, true, 0, 0),
            (RolloverStop::AfterCheckpoint, Revision(2), 0, true, 1, 0),
            (RolloverStop::AfterLogArchive, Revision(2), 0, false, 1, 1),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let (store, view, expected_hash, _) = rollover_fixture(temp.path());
            assert_eq!(
                store.rollover_checkpoint_inner(&view, stop),
                Err(UniverseError::Cancelled)
            );

            let independent = UniverseStore::open(temp.path()).unwrap();
            let reopened = independent
                .load_current_overlay_indexed(AdjacencyOverlayBudget::default())
                .unwrap();
            assert_eq!(reopened.snapshot().canonical_hash().unwrap(), expected_hash);
            assert_eq!(reopened.base_adjacency().revision(), expected_base_revision);
            assert_eq!(reopened.overlay().events_applied(), expected_overlay_events);
            assert_eq!(
                temp.path().join(ACTIVE_EVENT_LOG).exists(),
                active_log_exists
            );
            assert_eq!(
                versioned_checkpoint_paths(temp.path()).len(),
                expected_checkpoint_count
            );
            assert_eq!(
                fs::read_dir(temp.path())
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .filter(|name| name.to_string_lossy().starts_with("events-through-r"))
                    .count(),
                expected_archive_count
            );
            assert_eq!(reopened.snapshot().event_keys.len(), 2);
            assert_eq!(reopened.snapshot().entities.len(), 2);
        }
    }

    #[test]
    fn archived_log_is_idempotent_when_reintroduced_before_new_events() {
        let temp = tempfile::tempdir().unwrap();
        let (store, view, compacted_hash, _) = rollover_fixture(temp.path());
        let receipt = store.rollover_checkpoint(&view).unwrap();
        let archive = receipt.archived_event_log.unwrap();
        fs::copy(
            temp.path().join(archive.file_name),
            temp.path().join(ACTIVE_EVENT_LOG),
        )
        .unwrap();

        let old_log_replayed = UniverseStore::open(temp.path())
            .unwrap()
            .load_current_overlay_indexed(AdjacencyOverlayBudget::default())
            .unwrap();
        assert_eq!(
            old_log_replayed.snapshot().canonical_hash().unwrap(),
            compacted_hash
        );
        assert_eq!(old_log_replayed.overlay().events_applied(), 0);
        assert!(old_log_replayed.overlay().is_empty());

        let new_event = EventRecord::new(
            UniverseId(21),
            Revision(2),
            Tick(3),
            "entity-3",
            UniverseMutation::PutEntity {
                entity: EntityRecord {
                    key: EntityKey(3),
                    generation: 0,
                    symbol: 0,
                    content: None,
                },
            },
        )
        .unwrap();
        store.append_event(&new_event).unwrap();
        let with_new_suffix = UniverseStore::open(temp.path())
            .unwrap()
            .load_current_overlay_indexed(AdjacencyOverlayBudget::default())
            .unwrap();
        assert_eq!(with_new_suffix.snapshot().revision, Revision(3));
        assert_eq!(with_new_suffix.snapshot().event_keys.len(), 3);
        assert_eq!(with_new_suffix.snapshot().entities.len(), 3);
        assert_eq!(with_new_suffix.overlay().events_applied(), 1);
    }

    #[test]
    fn rollover_refuses_a_stale_overlay_before_publishing_a_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let (store, stale_view, _, _) = rollover_fixture(temp.path());
        let mut stale_index = stale_view.clone();
        stale_index.base_adjacency.offsets.clear();
        assert!(matches!(
            store.rollover_checkpoint(&stale_index),
            Err(UniverseError::Validation(message))
                if message == "checkpoint rollover refused a stale base adjacency"
        ));
        let later = EventRecord::new(
            UniverseId(21),
            Revision(2),
            Tick(3),
            "entity-3",
            UniverseMutation::PutEntity {
                entity: EntityRecord {
                    key: EntityKey(3),
                    generation: 0,
                    symbol: 0,
                    content: None,
                },
            },
        )
        .unwrap();
        store.append_event(&later).unwrap();

        assert!(matches!(
            store.rollover_checkpoint(&stale_view),
            Err(UniverseError::Validation(message))
                if message == "checkpoint rollover refused a stale current view"
        ));
        assert!(versioned_checkpoint_paths(temp.path()).is_empty());
        let independently_reopened = UniverseStore::open(temp.path())
            .unwrap()
            .load_current_indexed()
            .unwrap();
        assert_eq!(independently_reopened.snapshot().revision, Revision(3));
        assert_eq!(independently_reopened.snapshot().entities.len(), 3);
    }

    #[test]
    fn versioned_checkpoint_rejects_hash_or_revision_drift() {
        let hash_temp = tempfile::tempdir().unwrap();
        let (hash_store, hash_view, _, _) = rollover_fixture(hash_temp.path());
        let hash_receipt = hash_store.rollover_checkpoint(&hash_view).unwrap();
        let hash_path = hash_temp.path().join(versioned_checkpoint_file_name(
            hash_receipt.checkpoint_revision,
            &hash_receipt.checkpoint_hash,
        ));
        let mut drifted = hash_view.snapshot().clone();
        drifted.tick = Tick(99);
        fs::write(&hash_path, serde_json::to_vec(&drifted).unwrap()).unwrap();
        assert!(matches!(
            UniverseStore::open(hash_temp.path())
                .unwrap()
                .load_snapshot(),
            Err(UniverseError::CorruptContent(message))
                if message == "checkpoint filename hash differs from snapshot hash"
        ));

        let revision_temp = tempfile::tempdir().unwrap();
        let (revision_store, revision_view, _, _) = rollover_fixture(revision_temp.path());
        let revision_receipt = revision_store.rollover_checkpoint(&revision_view).unwrap();
        let correct_path = revision_temp.path().join(versioned_checkpoint_file_name(
            revision_receipt.checkpoint_revision,
            &revision_receipt.checkpoint_hash,
        ));
        let false_revision_path = revision_temp.path().join(versioned_checkpoint_file_name(
            Revision(99),
            &revision_receipt.checkpoint_hash,
        ));
        fs::copy(correct_path, false_revision_path).unwrap();
        assert!(matches!(
            UniverseStore::open(revision_temp.path())
                .unwrap()
                .load_snapshot(),
            Err(UniverseError::CorruptContent(message))
                if message.contains("checkpoint filename revision")
        ));
    }

    #[test]
    fn repository_genesis_hash_and_contract_load() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/genesis/minimal-genesis.json");
        let snapshot = load_genesis(path).unwrap();
        assert_eq!(snapshot.universe, UniverseId(1));
        assert_eq!(snapshot.entities.len(), 18);
        assert_eq!(snapshot.relations.len(), 16);
        assert_eq!(
            snapshot.symbols[snapshot.entities[0].symbol as usize],
            "Actor"
        );
        let result_type = snapshot
            .relations
            .iter()
            .find(|relation| snapshot.symbols[relation.predicate as usize] == "result_type")
            .unwrap();
        let target = snapshot
            .entities
            .iter()
            .find(|entity| entity.key == result_type.target)
            .unwrap();
        assert_eq!(snapshot.symbols[target.symbol as usize], "Moment");
    }

    #[test]
    fn canonical_ontology_reconstructs_from_the_authoritative_store() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/ontology/canonical-ontology.json");
        let seed = load_seed(path).unwrap();
        assert_eq!(seed.entities.len(), 231);
        assert_eq!(seed.relations.len(), 784);

        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let installed = store.install_seed(&seed).unwrap();
        let independent_store = UniverseStore::open(temp.path()).unwrap();
        let independent = independent_store.load_snapshot().unwrap();
        assert_eq!(
            installed.canonical_hash().unwrap(),
            independent.canonical_hash().unwrap()
        );

        let registry = ontology::OntologyRegistry::load(
            &independent_store,
            &independent,
            ontology::OntologyLoadBudget::default(),
        )
        .unwrap();
        assert_eq!(registry.schema_version, "1.17.0");
        assert_eq!(registry.mapping_version, "0.4.0");
        assert_eq!(registry.predicates.len(), 55);
        assert_eq!(registry.physical_profiles.len(), 65);
        assert_eq!(
            registry.known_gaps,
            BTreeSet::from(["BASED_ON".into(), "PROPOSES_CHANGE_TO".into()])
        );
        assert!(registry
            .predicate("GROUNDS")
            .unwrap()
            .physical_profile
            .is_some());
        assert!(registry
            .predicate("PROPOSES_CHANGE_TO")
            .unwrap()
            .physical_profile
            .is_none());
        assert_eq!(
            registry
                .semantic_type("working_hypothesis")
                .unwrap()
                .stored_node_type
                .as_deref(),
            Some("narrative")
        );
    }
}
