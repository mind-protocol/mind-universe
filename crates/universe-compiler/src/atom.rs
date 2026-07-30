//! Bounded materialization of canonical Graph IR Atoms into derived bytecode.

use crate::{compile, Bytecode, CompileError};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use universe_core::{EntityKey, Revision, UniverseError};
use universe_ir::{CodeDefinition, Operator, Register, Value, IR_VERSION};
use universe_store::{ContentRef, RelationRecord, UniverseSnapshot, UniverseStore};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AtomCompilationBudget {
    pub max_entities: usize,
    pub max_relations: usize,
    pub max_operators: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomCompilationReceipt {
    pub code_root: EntityKey,
    pub universe_revision: Revision,
    pub snapshot_hash: String,
    pub artifact_hash: String,
    pub source_atoms: Vec<EntityKey>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtomCompilation {
    pub bytecode: Bytecode,
    pub receipt: AtomCompilationReceipt,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum AtomCompileError {
    #[error(transparent)]
    Store(#[from] UniverseError),
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error("Atom compilation budget exhausted: {0}")]
    Budget(String),
    #[error("invalid Graph IR Atom cluster: {0}")]
    Invalid(String),
    #[error("unsupported graph opcode {0}")]
    UnsupportedOpcode(String),
}

#[derive(Debug, Deserialize)]
struct StoredCodeDefinition {
    kind: String,
    ir_version: u16,
    revision: Revision,
    #[serde(default)]
    required_capabilities: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct StoredOperator {
    kind: String,
    opcode: String,
    value: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct StoredRegister {
    kind: String,
    register: Register,
}

#[derive(Debug, Deserialize)]
struct StoredValue {
    kind: String,
    value: Value,
}

#[derive(Debug, Deserialize)]
struct StoredLoopBudget {
    kind: String,
    max_iterations: u32,
}

#[derive(Debug, Deserialize)]
struct StoredBinding {
    role: String,
}

pub fn compile_atom_code_definition(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    root: EntityKey,
    budget: AtomCompilationBudget,
) -> Result<AtomCompilation, AtomCompileError> {
    let (code, source_atoms) = materialize_with_sources(store, snapshot, root, budget)?;
    let bytecode = compile(&code)?;
    if bytecode.instructions.len() != source_atoms.len() {
        return Err(invalid(
            "derived instruction count differs from Atom provenance count",
        ));
    }
    let snapshot_hash = snapshot.canonical_hash()?;
    let artifact_hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&(
            root,
            snapshot.revision,
            &snapshot_hash,
            &bytecode,
            &source_atoms,
        ))
        .expect("Atom compilation artifact serialization is infallible"),
    ));
    Ok(AtomCompilation {
        bytecode,
        receipt: AtomCompilationReceipt {
            code_root: root,
            universe_revision: snapshot.revision,
            snapshot_hash,
            artifact_hash,
            source_atoms,
        },
    })
}

pub fn materialize_atom_code_definition(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    root: EntityKey,
    budget: AtomCompilationBudget,
) -> Result<CodeDefinition, AtomCompileError> {
    Ok(materialize_with_sources(store, snapshot, root, budget)?.0)
}

fn materialize_with_sources(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    root: EntityKey,
    budget: AtomCompilationBudget,
) -> Result<(CodeDefinition, Vec<EntityKey>), AtomCompileError> {
    if snapshot.entities.len() > budget.max_entities {
        return Err(AtomCompileError::Budget(format!(
            "{} entities exceed limit {}",
            snapshot.entities.len(),
            budget.max_entities
        )));
    }
    if snapshot.relations.len() > budget.max_relations {
        return Err(AtomCompileError::Budget(format!(
            "{} relations exceed limit {}",
            snapshot.relations.len(),
            budget.max_relations
        )));
    }

    let stored: StoredCodeDefinition = read_entity(store, snapshot, root)?;
    if stored.kind != "graph_ir_code_definition" {
        return Err(invalid("root is not a graph_ir_code_definition"));
    }
    if stored.ir_version != IR_VERSION {
        return Err(CompileError::UnsupportedVersion(stored.ir_version).into());
    }

    let mut current = exact_target(store, snapshot, root, "ENTRY", None)?;
    let mut operators = Vec::new();
    let mut source_atoms = Vec::new();
    let mut visited = std::collections::BTreeSet::new();
    loop {
        if !visited.insert(current) {
            return Err(invalid("unbounded NEXT cycle in Atom cluster"));
        }
        let atom: StoredOperator = read_entity(store, snapshot, current)?;
        if atom.kind != "graph_ir_operator" {
            return Err(invalid(format!(
                "entity {current} is not a graph_ir_operator"
            )));
        }
        match atom.opcode.as_str() {
            "constant" => {
                let output = register_target(store, snapshot, current, "OUTPUT", None)?;
                let value = atom
                    .value
                    .ok_or_else(|| invalid("constant Atom has no value"))?;
                operators.push(Operator::Constant { value, output });
                source_atoms.push(current);
                current = exact_target(store, snapshot, current, "NEXT", None)?;
            }
            "repeat_n" => {
                let iterations_atom =
                    exact_target(store, snapshot, current, "INPUT", Some("iterations"))?;
                let iterations = match read_entity::<StoredValue>(store, snapshot, iterations_atom)?
                {
                    StoredValue {
                        kind,
                        value: Value::Integer(value),
                    } if kind == "graph_ir_value" && value > 0 => u32::try_from(value)
                        .map_err(|_| invalid("REPEAT_N iterations exceed u32"))?,
                    _ => return Err(invalid("REPEAT_N iterations must be a positive integer")),
                };
                let loop_budget: StoredLoopBudget = read_entity(
                    store,
                    snapshot,
                    exact_target(store, snapshot, current, "BUDGET", None)?,
                )?;
                if loop_budget.kind != "graph_ir_loop_budget" || loop_budget.max_iterations == 0 {
                    return Err(invalid("REPEAT_N requires a non-zero loop budget"));
                }
                if iterations > loop_budget.max_iterations {
                    return Err(AtomCompileError::Budget(format!(
                        "REPEAT_N requests {iterations} iterations, budget is {}",
                        loop_budget.max_iterations
                    )));
                }
                let body = exact_target(store, snapshot, current, "BODY", None)?;
                lower_repeated_body(
                    store,
                    snapshot,
                    body,
                    iterations,
                    &mut operators,
                    &mut source_atoms,
                    budget.max_operators,
                )?;
                current = exact_target(store, snapshot, current, "NEXT", None)?;
            }
            "return" => {
                let value = register_target(store, snapshot, current, "INPUT", Some("value"))?;
                operators.push(Operator::Return { value });
                source_atoms.push(current);
                break;
            }
            opcode => return Err(AtomCompileError::UnsupportedOpcode(opcode.into())),
        }
        if operators.len() > budget.max_operators {
            return Err(AtomCompileError::Budget(format!(
                "{} materialized operators exceed limit {}",
                operators.len(),
                budget.max_operators
            )));
        }
    }

    Ok((
        CodeDefinition {
            ir_version: stored.ir_version,
            revision: stored.revision,
            required_capabilities: stored.required_capabilities,
            operators,
        },
        source_atoms,
    ))
}

fn lower_repeated_body(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    body: EntityKey,
    iterations: u32,
    operators: &mut Vec<Operator>,
    source_atoms: &mut Vec<EntityKey>,
    max_operators: usize,
) -> Result<(), AtomCompileError> {
    let atom: StoredOperator = read_entity(store, snapshot, body)?;
    if atom.kind != "graph_ir_operator" || atom.opcode != "propose" {
        return Err(AtomCompileError::UnsupportedOpcode(format!(
            "REPEAT_N body {}",
            atom.opcode
        )));
    }
    let command = register_target(store, snapshot, body, "INPUT", Some("command"))?;
    let output_template = register_target(store, snapshot, body, "OUTPUT", None)?;
    for iteration in 0..iterations {
        if operators.len() >= max_operators {
            return Err(AtomCompileError::Budget(format!(
                "materialized operators exceed limit {max_operators}"
            )));
        }
        let offset = u16::try_from(iteration)
            .map_err(|_| invalid("REPEAT_N register allocation exceeds u16"))?;
        let output = output_template
            .checked_add(offset)
            .ok_or_else(|| invalid("REPEAT_N register allocation overflow"))?;
        operators.push(Operator::Propose { command, output });
        source_atoms.push(body);
    }
    Ok(())
}

fn register_target(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    source: EntityKey,
    predicate: &str,
    role: Option<&str>,
) -> Result<Register, AtomCompileError> {
    let target = exact_target(store, snapshot, source, predicate, role)?;
    let stored: StoredRegister = read_entity(store, snapshot, target)?;
    if stored.kind != "graph_ir_register" {
        return Err(invalid(format!("entity {target} is not a register Atom")));
    }
    Ok(stored.register)
}

fn exact_target(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    source: EntityKey,
    predicate: &str,
    role: Option<&str>,
) -> Result<EntityKey, AtomCompileError> {
    let predicate_id = snapshot
        .symbol_id(predicate)
        .ok_or_else(|| invalid(format!("predicate {predicate} is not interned")))?;
    let mut targets = Vec::new();
    for relation in snapshot
        .relations
        .iter()
        .filter(|relation| relation.source == source && relation.predicate == predicate_id)
    {
        if let Some(expected_role) = role {
            let binding: StoredBinding = read_relation(store, relation)?;
            if binding.role != expected_role {
                continue;
            }
        }
        targets.push(relation.target);
    }
    if targets.len() != 1 {
        return Err(invalid(format!(
            "{source} must have exactly one {predicate} binding{}, found {}",
            role.map(|role| format!(" for role {role}"))
                .unwrap_or_default(),
            targets.len()
        )));
    }
    Ok(targets[0])
}

fn read_entity<T: DeserializeOwned>(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
    key: EntityKey,
) -> Result<T, AtomCompileError> {
    let entity = snapshot
        .entities
        .iter()
        .find(|entity| entity.key == key)
        .ok_or_else(|| invalid(format!("missing Atom {key}")))?;
    read_content(
        store,
        entity
            .content
            .as_ref()
            .ok_or_else(|| invalid(format!("Atom {key} has no content")))?,
    )
}

fn read_relation<T: DeserializeOwned>(
    store: &UniverseStore,
    relation: &RelationRecord,
) -> Result<T, AtomCompileError> {
    read_content(
        store,
        relation
            .content
            .as_ref()
            .ok_or_else(|| invalid(format!("relation {} has no role content", relation.key)))?,
    )
}

fn read_content<T: DeserializeOwned>(
    store: &UniverseStore,
    content: &ContentRef,
) -> Result<T, AtomCompileError> {
    serde_json::from_value(store.read_content(content)?)
        .map_err(|error| invalid(format!("invalid Atom content: {error}")))
}

fn invalid(message: impl Into<String>) -> AtomCompileError {
    AtomCompileError::Invalid(message.into())
}
