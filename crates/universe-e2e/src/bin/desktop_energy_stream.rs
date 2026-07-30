use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};
use universe_core::{Revision, Tick};
use universe_e2e::behavior_runtime::{
    default_genesis_path, run as run_behavior, BehaviorRuntimeConfig,
};
use universe_ir::{CodeDefinition, QuerySpec, Value};
use universe_physics::{AtomTransfer, BondPolarity};
use universe_protocol::{
    CorrelationId, EnergyTransferDirection, EnergyTransferEpistemic, EnergyTransferMessage,
    EnergyTransferOutcome, EnergyTransferPolarity, EnergyTransferVisualMessage,
    EnergyTransferVisualPrimitive, ProtocolStream, QueryCompletion, ResumeResult, ServerFrame,
    ServerPayload, SituationSnapshotMessage, StreamBudget, StreamSequence, VISUAL_MICROUNITS,
};
use universe_store::UniverseStore;
use universe_vm::{execute_program, ExecutionLimits, ExecutionReceipt, VmHost};

const MAPPING_NODE_ID: &str = "code:mind-desktop:map-energy-transfer:v0";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct DesktopEnergyStreamManifest {
    behavior_correlation: String,
    authority_revision: Revision,
    mapping_node_id: String,
    mapping_code_path: PathBuf,
    mapping_executions: Vec<ExecutionReceipt>,
    frames: Vec<ServerFrame>,
    serialized_frame_readback: bool,
    measured_transfer_count: usize,
    production_transport_wired: bool,
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

fn main() {
    if let Err(error) = run() {
        eprintln!("desktop_energy_stream_failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let artifact_root = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| repository.join("artifacts/desktop-energy-stream"));
    let behavior = run_behavior(&BehaviorRuntimeConfig {
        artifact_root: artifact_root.join("behavior"),
        genesis_path: default_genesis_path(),
    })
    .map_err(debug_error)?;
    let independent_store = UniverseStore::open(&behavior.store_root)?;
    let independent_snapshot = independent_store.load_snapshot()?;
    let mapping_code_path = repository.join("fixtures/graph-ir/energy-transfer-visual.json");
    let mapping_code: CodeDefinition = serde_json::from_slice(&fs::read(&mapping_code_path)?)?;
    universe_compiler::validate(&mapping_code)?;

    let mut stream = ProtocolStream::new(
        "mind-desktop-energy-e2e",
        StreamBudget {
            max_pending_frames: 256,
            max_pending_bytes: 2 * 1024 * 1024,
            max_frame_bytes: 64 * 1024,
        },
    )
    .map_err(debug_error)?;
    let snapshot_frame = stream
        .publish(
            CorrelationId(behavior.correlation.clone()),
            ServerPayload::Snapshot(SituationSnapshotMessage {
                universe: independent_snapshot.universe,
                revision: behavior.authority_revision,
                tick: behavior.execution.physical.run.start_tick,
                origin: behavior.execution.plan.source.atom,
                completion: QueryCompletion::Partial,
                entities: Vec::new(),
                relations: Vec::new(),
            }),
        )
        .map_err(debug_error)?;

    let mut mapping_executions = Vec::new();
    let mut published = vec![snapshot_frame];
    let mut ordinal = 0usize;
    for step in &behavior.execution.physical.run.steps {
        for transfer in &step.transfers {
            let inputs = BTreeMap::from([
                (
                    "polarity".into(),
                    Value::Text(polarity_name(transfer.polarity).into()),
                ),
                ("outcome".into(), Value::Text("measured".into())),
            ]);
            let execution = execute_program(
                &mapping_code,
                &mut MappingHost,
                &inputs,
                behavior.authority_revision,
                step.tick,
                ExecutionLimits {
                    fuel: 32,
                    max_proposals: 0,
                },
            )?;
            let visual = decode_visual_descriptor(&execution.result)?;
            let message = energy_message(
                independent_snapshot.universe,
                behavior.authority_revision,
                &behavior.correlation,
                behavior.execution.plan.objective.to_string(),
                transfer,
                step.tick,
                ordinal,
                visual,
            )?;
            let frame = stream
                .publish(
                    CorrelationId(behavior.correlation.clone()),
                    ServerPayload::EnergyTransfer(message),
                )
                .map_err(debug_error)?;
            mapping_executions.push(execution);
            published.push(frame);
            ordinal += 1;
        }
    }
    if ordinal == 0 {
        return Err(io::Error::other("physical receipt contained no measured transfer").into());
    }

    let resumed = stream.resume(StreamSequence(0)).map_err(debug_error)?;
    let ResumeResult::Frames { frames, .. } = resumed else {
        return Err(
            io::Error::other("fresh stream unexpectedly required resynchronization").into(),
        );
    };
    if frames != published {
        return Err(io::Error::other("stream resume readback differs from publication").into());
    }

    let run_root = artifact_root.join(&behavior.correlation);
    fs::create_dir_all(&run_root)?;
    let frames_path = run_root.join("frames.json");
    fs::write(&frames_path, serde_json::to_vec_pretty(&frames)?)?;
    let serialized_frames: Vec<ServerFrame> = serde_json::from_slice(&fs::read(&frames_path)?)?;
    let serialized_frame_readback = serialized_frames == frames;
    if !serialized_frame_readback {
        return Err(io::Error::other("serialized frame readback differs").into());
    }

    let manifest = DesktopEnergyStreamManifest {
        behavior_correlation: behavior.correlation,
        authority_revision: behavior.authority_revision,
        mapping_node_id: MAPPING_NODE_ID.into(),
        mapping_code_path,
        mapping_executions,
        frames,
        serialized_frame_readback,
        measured_transfer_count: ordinal,
        production_transport_wired: false,
    };
    let manifest_path = run_root.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    let readback: DesktopEnergyStreamManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    if readback != manifest {
        return Err(io::Error::other("manifest readback differs").into());
    }

    println!(
        "desktop_energy_stream_observed correlation={} transfers={} frames={} manifest={}",
        manifest.behavior_correlation,
        manifest.measured_transfer_count,
        manifest.frames.len(),
        manifest_path.display()
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn energy_message(
    universe: universe_core::UniverseId,
    revision: Revision,
    execution_id: &str,
    intention_id: String,
    transfer: &AtomTransfer,
    tick: Tick,
    ordinal: usize,
    visual: EnergyTransferVisualMessage,
) -> Result<EnergyTransferMessage, Box<dyn Error>> {
    let direction = if transfer.source != transfer.target {
        EnergyTransferDirection::SourceToTarget
    } else {
        return Err(io::Error::other("physical transfer has identical endpoints").into());
    };
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
        direction,
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
    message.validate().map_err(debug_error)?;
    Ok(message)
}

fn polarity_name(polarity: BondPolarity) -> &'static str {
    match polarity {
        BondPolarity::Support => "support",
        BondPolarity::Inhibit => "inhibit",
        BondPolarity::Neutral => "neutral",
    }
}

fn decode_visual_descriptor(value: &Value) -> Result<EnergyTransferVisualMessage, Box<dyn Error>> {
    let Value::Record(record) = value else {
        return Err(io::Error::other("mapping result is not a visual descriptor record").into());
    };
    let primitive = match text(record, "primitive")? {
        "energy_packet" => EnergyTransferVisualPrimitive::EnergyPacket,
        "inhibitory_wave" => EnergyTransferVisualPrimitive::InhibitoryWave,
        "rupture" => EnergyTransferVisualPrimitive::Rupture,
        other => {
            return Err(io::Error::other(format!("unsupported renderer primitive {other}")).into())
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

fn text<'a>(record: &'a BTreeMap<String, Value>, field: &str) -> Result<&'a str, Box<dyn Error>> {
    match record.get(field) {
        Some(Value::Text(value)) => Ok(value),
        _ => Err(io::Error::other(format!("{field} is not text")).into()),
    }
}

fn integer(record: &BTreeMap<String, Value>, field: &str) -> Result<u32, Box<dyn Error>> {
    match record.get(field) {
        Some(Value::Integer(value)) => u32::try_from(*value)
            .map_err(|_| io::Error::other(format!("{field} is outside u32")).into()),
        _ => Err(io::Error::other(format!("{field} is not an integer")).into()),
    }
}

fn debug_error(error: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{error:?}"))
}

#[allow(dead_code)]
fn _assert_materialization_path_is_repository_relative(path: &Path) -> bool {
    path.ends_with("fixtures/graph-ir/energy-transfer-visual.json")
}
