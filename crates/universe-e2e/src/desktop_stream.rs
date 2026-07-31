//! Branche the executed neighborhood arc to the desktop.
//!
//! It streams each executed bond's MEASURED energy transfers through the
//! membrane (`ProtocolStream`, which rejects any transfer whose epistemic is not
//! `Measured` — see `universe-protocol` `EnergyTransferMessage::validate`). This
//! is the whole point of the executed arc: its energy is `Measured`, so — unlike
//! the derived ride — it publishes. Visual descriptors come from the graph-owned
//! mapping CodeDefinition, never hardcoded, and nothing bypasses
//! `ProtocolStream.publish` (no raw file-frame smuggling).

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use universe_core::{EntityKey, Revision, Tick, UniverseId};
use universe_ir::{CodeDefinition, QuerySpec, Value};
use universe_physics::{AtomTransfer, BondPolarity};
use universe_protocol::{
    CorrelationId, EnergyTransferDirection, EnergyTransferEpistemic, EnergyTransferMessage,
    EnergyTransferOutcome, EnergyTransferPolarity, EnergyTransferVisualMessage,
    EnergyTransferVisualPrimitive, ProtocolStream, QueryCompletion, ResumeResult, ServerFrame,
    ServerPayload, SituationSnapshotMessage, StreamBudget, StreamSequence, VISUAL_MICROUNITS,
};
use universe_vm::{execute_program, ExecutionLimits, VmHost};

use crate::neighborhood_arc::execute_neighborhood;
use crate::E2eError;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DesktopStreamManifest {
    pub start: EntityKey,
    pub bonds_executed: usize,
    pub measured_transfer_count: usize,
    pub frames: usize,
    pub serialized_frame_readback: bool,
    pub resume_readback: bool,
    pub manifest_path: PathBuf,
}

/// Run the executed neighborhood arc and stream its measured transfers to the
/// desktop artifact directory as membrane-validated frames.
pub fn stream_neighborhood_to_desktop(
    repository: &Path,
    artifact_root: &Path,
    max_bonds: usize,
) -> Result<DesktopStreamManifest, E2eError> {
    let execution = execute_neighborhood(repository, &artifact_root.join("arc"), max_bonds)?;

    let mapping_code: CodeDefinition = serde_json::from_slice(
        &fs::read(repository.join("fixtures/graph-ir/energy-transfer-visual.json"))
            .map_err(|error| E2eError::Io(error.to_string()))?,
    )
    .map_err(|error| E2eError::Contract(error.to_string()))?;
    universe_compiler::validate(&mapping_code).map_err(opaque)?;

    let mut stream = ProtocolStream::new(
        "mind-desktop-neighborhood",
        StreamBudget {
            max_pending_frames: 512,
            max_pending_bytes: 4 * 1024 * 1024,
            max_frame_bytes: 64 * 1024,
        },
    )
    .map_err(opaque)?;

    let correlation = format!("neighborhood-{}", execution.arc.start.0);
    let start_tick = execution
        .receipts
        .first()
        .map(|receipt| receipt.physical.run.start_tick)
        .unwrap_or(Tick(0));
    let snapshot_frame = stream
        .publish(
            CorrelationId(correlation.clone()),
            ServerPayload::Snapshot(SituationSnapshotMessage {
                universe: execution.universe,
                revision: execution.revision,
                tick: start_tick,
                origin: execution.arc.start,
                completion: QueryCompletion::Partial,
                entities: Vec::new(),
                relations: Vec::new(),
            }),
        )
        .map_err(opaque)?;

    let mut published = vec![snapshot_frame];
    let mut ordinal = 0usize;
    for receipt in &execution.receipts {
        let intention_id = receipt.plan.objective.to_string();
        for step in &receipt.physical.run.steps {
            for transfer in &step.transfers {
                let inputs = BTreeMap::from([
                    (
                        "polarity".into(),
                        Value::Text(polarity_name(transfer.polarity).into()),
                    ),
                    ("outcome".into(), Value::Text("measured".into())),
                ]);
                let mapping = execute_program(
                    &mapping_code,
                    &mut MappingHost,
                    &inputs,
                    execution.revision,
                    step.tick,
                    ExecutionLimits {
                        fuel: 32,
                        max_proposals: 0,
                    },
                )
                .map_err(|error| E2eError::Vm(format!("{error:?}")))?;
                let visual = decode_visual_descriptor(&mapping.result)?;
                let message = energy_message(
                    execution.universe,
                    execution.revision,
                    &correlation,
                    intention_id.clone(),
                    transfer,
                    step.tick,
                    ordinal,
                    visual,
                )?;
                let frame = stream
                    .publish(
                        CorrelationId(correlation.clone()),
                        ServerPayload::EnergyTransfer(message),
                    )
                    .map_err(opaque)?;
                published.push(frame);
                ordinal += 1;
            }
        }
    }
    if ordinal == 0 {
        return Err(E2eError::Contract(
            "executed neighborhood produced no measured transfer to stream".into(),
        ));
    }

    let resumed = stream.resume(StreamSequence(0)).map_err(opaque)?;
    let ResumeResult::Frames { frames, .. } = resumed else {
        return Err(E2eError::Contract(
            "fresh stream unexpectedly required resynchronization".into(),
        ));
    };
    let resume_readback = frames == published;

    let run_root = artifact_root.join(&correlation);
    fs::create_dir_all(&run_root).map_err(|error| E2eError::Io(error.to_string()))?;
    let frames_path = run_root.join("frames.json");
    fs::write(
        &frames_path,
        serde_json::to_vec_pretty(&frames).map_err(|error| E2eError::Io(error.to_string()))?,
    )
    .map_err(|error| E2eError::Io(error.to_string()))?;
    let serialized: Vec<ServerFrame> = serde_json::from_slice(
        &fs::read(&frames_path).map_err(|error| E2eError::Io(error.to_string()))?,
    )
    .map_err(|error| E2eError::Contract(error.to_string()))?;
    let serialized_frame_readback = serialized == frames;

    let manifest_path = run_root.join("manifest.json");
    let manifest = DesktopStreamManifest {
        start: execution.arc.start,
        bonds_executed: execution.receipts.len(),
        measured_transfer_count: ordinal,
        frames: frames.len(),
        serialized_frame_readback,
        resume_readback,
        manifest_path: manifest_path.clone(),
    };
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).map_err(|error| E2eError::Io(error.to_string()))?,
    )
    .map_err(|error| E2eError::Io(error.to_string()))?;
    Ok(manifest)
}

fn opaque(error: impl std::fmt::Debug) -> E2eError {
    E2eError::Contract(format!("{error:?}"))
}

#[allow(clippy::too_many_arguments)]
fn energy_message(
    universe: UniverseId,
    revision: Revision,
    execution_id: &str,
    intention_id: String,
    transfer: &AtomTransfer,
    tick: Tick,
    ordinal: usize,
    visual: EnergyTransferVisualMessage,
) -> Result<EnergyTransferMessage, E2eError> {
    if transfer.source == transfer.target {
        return Err(E2eError::Contract(
            "physical transfer has identical endpoints".into(),
        ));
    }
    let message = EnergyTransferMessage {
        universe,
        revision,
        tick,
        transfer_id: format!("{execution_id}:{}:{}:{ordinal}", tick.0, transfer.bond),
        execution_id: execution_id.into(),
        intention_id,
        relation_id: transfer.bond,
        source: transfer.source,
        target: transfer.target,
        direction: EnergyTransferDirection::SourceToTarget,
        polarity: match transfer.polarity {
            BondPolarity::Support => EnergyTransferPolarity::Support,
            BondPolarity::Inhibit => EnergyTransferPolarity::Inhibit,
            BondPolarity::Neutral => EnergyTransferPolarity::Neutral,
        },
        energy: transfer.energy,
        gate_microunits: VISUAL_MICROUNITS,
        outcome: EnergyTransferOutcome::Measured,
        epistemic: EnergyTransferEpistemic::Measured,
        visual,
    };
    message.validate().map_err(opaque)?;
    Ok(message)
}

fn polarity_name(polarity: BondPolarity) -> &'static str {
    match polarity {
        BondPolarity::Support => "support",
        BondPolarity::Inhibit => "inhibit",
        BondPolarity::Neutral => "neutral",
    }
}

fn decode_visual_descriptor(value: &Value) -> Result<EnergyTransferVisualMessage, E2eError> {
    let Value::Record(record) = value else {
        return Err(E2eError::Contract(
            "mapping result is not a visual descriptor record".into(),
        ));
    };
    let primitive = match text(record, "primitive")? {
        "energy_packet" => EnergyTransferVisualPrimitive::EnergyPacket,
        "inhibitory_wave" => EnergyTransferVisualPrimitive::InhibitoryWave,
        "rupture" => EnergyTransferVisualPrimitive::Rupture,
        other => {
            return Err(E2eError::Contract(format!(
                "unsupported renderer primitive {other}"
            )))
        }
    };
    Ok(EnergyTransferVisualMessage {
        primitive,
        color: text(record, "color")?.into(),
        emissive: text(record, "emissive")?.into(),
        emissive_intensity_microunits: integer(record, "emissive_intensity_microunits")?,
        radius_microunits: integer(record, "radius_microunits")?,
        opacity_microunits: integer(record, "opacity_microunits")?,
        duration_ms: integer(record, "duration_ms")?,
    })
}

fn text<'a>(record: &'a BTreeMap<String, Value>, field: &str) -> Result<&'a str, E2eError> {
    match record.get(field) {
        Some(Value::Text(value)) => Ok(value),
        _ => Err(E2eError::Contract(format!("{field} is not text"))),
    }
}

fn integer(record: &BTreeMap<String, Value>, field: &str) -> Result<u32, E2eError> {
    match record.get(field) {
        Some(Value::Integer(value)) => {
            u32::try_from(*value).map_err(|_| E2eError::Contract(format!("{field} is outside u32")))
        }
        _ => Err(E2eError::Contract(format!("{field} is not an integer"))),
    }
}

#[derive(Default)]
struct MappingHost;

impl VmHost for MappingHost {
    fn is_cancelled(&self) -> bool {
        false
    }

    fn capabilities(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn open_query(
        &mut self,
        _spec: &QuerySpec,
        _origin: &Value,
        _selector: &Value,
    ) -> Result<Value, String> {
        Err("energy visual mapping cannot query".into())
    }

    fn await_query(&mut self, _handle: &Value) -> Result<Value, String> {
        Err("energy visual mapping cannot await queries".into())
    }

    fn follow_one(&mut self, _source: &Value, _predicate: &Value) -> Result<Value, String> {
        Err("energy visual mapping cannot traverse".into())
    }

    fn entity_symbol(&mut self, _entity: &Value) -> Result<Value, String> {
        Err("energy visual mapping cannot inspect symbols".into())
    }

    fn hydrate(&mut self, _selected: &[Value], _max_bytes: u32) -> Result<Vec<Value>, String> {
        Err("energy visual mapping cannot hydrate".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_the_executed_neighborhood_as_measured_transfers() {
        let temp = tempfile::tempdir().unwrap();
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        // A large cap streams the whole neighborhood (truncated to what exists).
        let manifest = stream_neighborhood_to_desktop(&repository, temp.path(), 64).unwrap();
        println!("{manifest:#?}");

        assert!(manifest.bonds_executed >= 3, "stream the full neighborhood, not just the cap");
        // Every executed bond emits at least one measured transfer.
        assert!(manifest.measured_transfer_count >= manifest.bonds_executed);
        // Snapshot frame + one frame per transfer.
        assert_eq!(manifest.frames, manifest.measured_transfer_count + 1);
        assert!(manifest.serialized_frame_readback);
        assert!(manifest.resume_readback);
        assert!(manifest.manifest_path.is_file());
    }
}
