//! Minimal deterministic snapshot, content, and replay store.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use universe_core::{
    ContentPtr, EntityKey, RelationKey, Revision, Tick, UniverseError, UniverseId, VersionEnvelope,
};

pub const SNAPSHOT_FORMAT_VERSION: u16 = 0;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EntityRecord {
    pub key: EntityKey,
    pub generation: u32,
    pub symbol: u32,
    pub content: Option<ContentPtr>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationRecord {
    pub key: RelationKey,
    pub generation: u32,
    pub source: EntityKey,
    pub target: EntityKey,
    pub predicate: u32,
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
        let entities: BTreeSet<_> = self.entities.iter().map(|e| e.key).collect();
        if entities.len() != self.entities.len() {
            return Err(UniverseError::Validation("duplicate entity key".into()));
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
        Ok(())
    }

    pub fn canonical_hash(&self) -> Result<String, UniverseError> {
        canonical_hash(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UniverseMutation {
    PutEntity { entity: EntityRecord },
    PutRelation { relation: RelationRecord },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    pub envelope: VersionEnvelope<UniverseMutation>,
    pub idempotency_key: String,
    pub previous_revision: Revision,
    pub checksum: String,
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
    match &event.envelope.payload {
        UniverseMutation::PutEntity { entity } => {
            if snapshot.entities.iter().any(|e| e.key == entity.key) {
                return Err(UniverseError::Validation("duplicate entity key".into()));
            }
            snapshot.entities.push(entity.clone());
            snapshot.entities.sort_by_key(|e| e.key);
        }
        UniverseMutation::PutRelation { relation } => {
            let endpoints = |key| snapshot.entities.iter().any(|e| e.key == key);
            if !endpoints(relation.source) || !endpoints(relation.target) {
                return Err(UniverseError::Validation(
                    "missing relation endpoint".into(),
                ));
            }
            if snapshot.relations.iter().any(|r| r.key == relation.key) {
                return Err(UniverseError::Validation("duplicate relation key".into()));
            }
            snapshot.relations.push(relation.clone());
            snapshot.relations.sort_by_key(|r| r.key);
        }
    }
    snapshot.revision = event.envelope.revision;
    snapshot.tick = event.envelope.tick;
    snapshot.event_keys.insert(event.idempotency_key.clone());
    Ok(true)
}

pub struct UniverseStore {
    root: PathBuf,
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
        let bytes = fs::read(self.root.join("snapshot.json")).map_err(io_error)?;
        let snapshot: UniverseSnapshot = serde_json::from_slice(&bytes).map_err(json_error)?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn append_event(&self, event: &EventRecord) -> Result<(), UniverseError> {
        event.verify()?;
        let mut line = serde_json::to_vec(event).map_err(json_error)?;
        line.push(b'\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("events.jsonl"))
            .map_err(io_error)?;
        file.write_all(&line).map_err(io_error)?;
        file.sync_data().map_err(io_error)
    }

    /// Replays valid records and truncates only an incomplete final record.
    pub fn replay(
        &self,
        mut snapshot: UniverseSnapshot,
    ) -> Result<UniverseSnapshot, UniverseError> {
        let path = self.root.join("events.jsonl");
        if !path.exists() {
            return Ok(snapshot);
        }
        let bytes = fs::read(&path).map_err(io_error)?;
        let complete_len = bytes
            .iter()
            .rposition(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        for line in bytes[..complete_len]
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
        {
            let event: EventRecord = serde_json::from_slice(line).map_err(|e| {
                UniverseError::CorruptLog(format!("invalid complete event record: {e}"))
            })?;
            apply_event(&mut snapshot, &event)?;
        }
        if complete_len != bytes.len() {
            let file = OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(io_error)?;
            file.set_len(complete_len as u64).map_err(io_error)?;
        }
        Ok(snapshot)
    }

    pub fn append_content(
        &self,
        value: &serde_json::Value,
    ) -> Result<(ContentPtr, String), UniverseError> {
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
        Ok((
            ContentPtr {
                segment: 0,
                offset,
                length: line.len() as u32,
            },
            hash,
        ))
    }

    pub fn read_content(&self, ptr: ContentPtr) -> Result<serde_json::Value, UniverseError> {
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
        serde_json::from_slice(&bytes[..bytes.len() - 1]).map_err(json_error)
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenesisEnvelope {
    pub contract: String,
    pub version: u16,
    pub sha256: String,
    pub snapshot: UniverseSnapshot,
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

    #[test]
    fn boot_mutate_checkpoint_crash_replay_equivalence() {
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let base = UniverseSnapshot::empty(UniverseId(7));
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
        let base = UniverseSnapshot::empty(UniverseId(7));
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
        let (ptr, _) = store.append_content(&value).unwrap();
        assert_eq!(store.read_content(ptr).unwrap(), value);
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
}
