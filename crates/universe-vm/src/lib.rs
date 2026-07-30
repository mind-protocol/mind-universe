//! Fuel-bounded Graph IR virtual machine with a mutation-free host boundary.

use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use universe_compiler::{compile, Bytecode, CompileError};
use universe_core::{Revision, Tick};
use universe_ir::{CodeDefinition, Operator, Register, Value};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionLimits {
    pub fuel: u64,
    pub max_proposals: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionTrace {
    pub instruction: u32,
    pub source_node: u32,
    pub fuel_before: u64,
    pub operation: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WriteProposal {
    pub command: Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionReceipt {
    pub code_revision: Revision,
    pub starting_universe_revision: Revision,
    pub starting_tick: Tick,
    pub code_hash: String,
    pub fuel_used: u64,
    pub result: Value,
    pub proposals: Vec<WriteProposal>,
    pub trace: Vec<ExecutionTrace>,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum VmError {
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error("missing input {0}")]
    MissingInput(String),
    #[error("register {0} is unavailable")]
    MissingRegister(Register),
    #[error("VM fuel exhausted")]
    FuelExhausted,
    #[error("execution cancelled")]
    Cancelled,
    #[error("capability {0} is unavailable")]
    CapabilityUnavailable(String),
    #[error("host operation failed: {0}")]
    Host(String),
    #[error("expected {expected}, received another value type")]
    Type { expected: &'static str },
    #[error("proposal budget exhausted")]
    ProposalBudgetExhausted,
}

pub trait VmHost {
    fn is_cancelled(&self) -> bool;
    fn capabilities(&self) -> BTreeSet<String>;
    fn open_query(
        &mut self,
        spec: &universe_ir::QuerySpec,
        origin: &Value,
        selector: &Value,
    ) -> Result<Value, String>;
    fn await_query(&mut self, handle: &Value) -> Result<Value, String>;
    fn follow_one(&mut self, source: &Value, predicate: &Value) -> Result<Value, String>;
    fn entity_symbol(&mut self, entity: &Value) -> Result<Value, String>;
    fn hydrate(&mut self, selected: &[Value], max_bytes: u32) -> Result<Vec<Value>, String>;
}

fn register(registers: &BTreeMap<Register, Value>, id: Register) -> Result<&Value, VmError> {
    registers.get(&id).ok_or(VmError::MissingRegister(id))
}

pub fn execute_program(
    code: &CodeDefinition,
    host: &mut impl VmHost,
    inputs: &BTreeMap<String, Value>,
    starting_universe_revision: Revision,
    starting_tick: Tick,
    limits: ExecutionLimits,
) -> Result<ExecutionReceipt, VmError> {
    execute(
        &compile(code)?,
        host,
        inputs,
        starting_universe_revision,
        starting_tick,
        limits,
    )
}

pub fn execute(
    bytecode: &Bytecode,
    host: &mut impl VmHost,
    inputs: &BTreeMap<String, Value>,
    starting_universe_revision: Revision,
    starting_tick: Tick,
    limits: ExecutionLimits,
) -> Result<ExecutionReceipt, VmError> {
    let available = host.capabilities();
    for required in &bytecode.required_capabilities {
        if !available.contains(required) {
            return Err(VmError::CapabilityUnavailable(required.clone()));
        }
    }
    let mut fuel = limits.fuel;
    let mut registers = BTreeMap::new();
    let mut proposals = Vec::new();
    let mut trace = Vec::new();
    let mut result = Value::Unit;

    for (index, op) in bytecode.instructions.iter().enumerate() {
        if host.is_cancelled() {
            return Err(VmError::Cancelled);
        }
        if fuel == 0 {
            return Err(VmError::FuelExhausted);
        }
        trace.push(ExecutionTrace {
            instruction: index as u32,
            source_node: bytecode.source_nodes[index],
            fuel_before: fuel,
            operation: format!("{op:?}"),
        });
        fuel -= 1;
        let output = match op {
            Operator::Input { name, .. } => Some(
                inputs
                    .get(name)
                    .cloned()
                    .ok_or_else(|| VmError::MissingInput(name.clone()))?,
            ),
            Operator::Constant { value, .. } => Some(value.clone()),
            Operator::QueryOpen { spec, .. } => Some(
                host.open_query(
                    spec,
                    register(&registers, spec.origin)?,
                    register(&registers, spec.selector)?,
                )
                .map_err(VmError::Host)?,
            ),
            Operator::QueryAwait { handle, .. } => Some(
                host.await_query(register(&registers, *handle)?)
                    .map_err(VmError::Host)?,
            ),
            Operator::FollowOne {
                source, predicate, ..
            } => Some(
                host.follow_one(
                    register(&registers, *source)?,
                    register(&registers, *predicate)?,
                )
                .map_err(VmError::Host)?,
            ),
            Operator::EntitySymbol { entity, .. } => Some(
                host.entity_symbol(register(&registers, *entity)?)
                    .map_err(VmError::Host)?,
            ),
            Operator::SelectMembers {
                input,
                allowed,
                max_items,
                ..
            } => {
                let Value::List(items) = register(&registers, *input)? else {
                    return Err(VmError::Type { expected: "list" });
                };
                let Value::List(allowed) = register(&registers, *allowed)? else {
                    return Err(VmError::Type { expected: "list" });
                };
                Some(Value::List(
                    items
                        .iter()
                        .filter(|item| allowed.contains(item))
                        .take(*max_items as usize)
                        .cloned()
                        .collect(),
                ))
            }
            Operator::OrderByPreference {
                input,
                preference,
                max_items,
                ..
            } => {
                let Value::List(items) = register(&registers, *input)? else {
                    return Err(VmError::Type { expected: "list" });
                };
                let Value::List(preference) = register(&registers, *preference)? else {
                    return Err(VmError::Type { expected: "list" });
                };
                let mut ordered = items.clone();
                ordered.sort_by_key(|item| {
                    preference
                        .iter()
                        .position(|preferred| preferred == item)
                        .unwrap_or(usize::MAX)
                });
                ordered.truncate(*max_items as usize);
                Some(Value::List(ordered))
            }
            Operator::FilterTruthy {
                input,
                field,
                max_items,
                ..
            } => {
                let Value::List(items) = register(&registers, *input)? else {
                    return Err(VmError::Type { expected: "list" });
                };
                Some(Value::List(
                    items.iter().take(*max_items as usize).filter(|item| {
                        matches!(item, Value::Record(record) if record.get(field) == Some(&Value::Bool(true)))
                    }).cloned().collect(),
                ))
            }
            Operator::TopK {
                input,
                score_field,
                limit,
                ..
            } => {
                let Value::List(items) = register(&registers, *input)? else {
                    return Err(VmError::Type { expected: "list" });
                };
                let mut ranked = items.clone();
                ranked.sort_by_key(|value| Reverse(score(value, score_field)));
                ranked.truncate(*limit as usize);
                Some(Value::List(ranked))
            }
            Operator::Hydrate {
                input,
                max_items,
                max_bytes,
                ..
            } => {
                let Value::List(items) = register(&registers, *input)? else {
                    return Err(VmError::Type { expected: "list" });
                };
                Some(Value::List(
                    host.hydrate(&items[..items.len().min(*max_items as usize)], *max_bytes)
                        .map_err(VmError::Host)?,
                ))
            }
            Operator::MakeRecord { fields, .. } => Some(Value::Record(
                fields
                    .iter()
                    .map(|(name, register_id)| {
                        Ok((name.clone(), register(&registers, *register_id)?.clone()))
                    })
                    .collect::<Result<BTreeMap<_, _>, VmError>>()?,
            )),
            Operator::Propose { command, .. } => {
                if proposals.len() >= limits.max_proposals as usize {
                    return Err(VmError::ProposalBudgetExhausted);
                }
                let proposal = WriteProposal {
                    command: register(&registers, *command)?.clone(),
                };
                proposals.push(proposal.clone());
                Some(proposal.command)
            }
            Operator::Return { value } => {
                result = register(&registers, *value)?.clone();
                None
            }
        };
        if let (Some(output_id), Some(value)) = (op.output(), output) {
            registers.insert(output_id, value);
        }
    }
    Ok(ExecutionReceipt {
        code_revision: bytecode.code_revision,
        starting_universe_revision,
        starting_tick,
        code_hash: bytecode.canonical_hash.clone(),
        fuel_used: limits.fuel - fuel,
        result,
        proposals,
        trace,
    })
}

fn score(value: &Value, field: &str) -> i64 {
    match value {
        Value::Record(record) => match record.get(field) {
            Some(Value::Integer(value)) => *value,
            _ => i64::MIN,
        },
        _ => i64::MIN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_ir::QuerySpec;

    #[derive(Default)]
    struct Host;
    impl VmHost for Host {
        fn is_cancelled(&self) -> bool {
            false
        }
        fn capabilities(&self) -> BTreeSet<String> {
            BTreeSet::from(["local_query".into()])
        }
        fn open_query(&mut self, _: &QuerySpec, _: &Value, _: &Value) -> Result<Value, String> {
            Ok(Value::Text("bounded-handle".into()))
        }
        fn await_query(&mut self, _: &Value) -> Result<Value, String> {
            Ok(Value::List(vec![
                Value::Record(BTreeMap::from([
                    ("entity".into(), Value::Entity(universe_core::EntityKey(3))),
                    ("eligible".into(), Value::Bool(true)),
                    ("resonance".into(), Value::Integer(900)),
                ])),
                Value::Record(BTreeMap::from([
                    ("entity".into(), Value::Entity(universe_core::EntityKey(4))),
                    ("eligible".into(), Value::Bool(false)),
                    ("resonance".into(), Value::Integer(100)),
                ])),
            ]))
        }
        fn follow_one(&mut self, source: &Value, predicate: &Value) -> Result<Value, String> {
            assert_eq!(source, &Value::Entity(universe_core::EntityKey(5)));
            assert_eq!(predicate, &Value::Text("result_type".into()));
            Ok(Value::Entity(universe_core::EntityKey(10)))
        }
        fn entity_symbol(&mut self, entity: &Value) -> Result<Value, String> {
            assert_eq!(entity, &Value::Entity(universe_core::EntityKey(10)));
            Ok(Value::Integer(7))
        }
        fn hydrate(&mut self, selected: &[Value], _: u32) -> Result<Vec<Value>, String> {
            Ok(selected.to_vec())
        }
    }

    fn fixture() -> CodeDefinition {
        serde_json::from_str(include_str!("../../../fixtures/graph-ir/minimal-read.json")).unwrap()
    }

    #[test]
    fn graph_program_queries_selects_hydrates_and_proposes_without_mutation() {
        let code = fixture();
        let bytecode = compile(&code).unwrap();
        let inputs = BTreeMap::from([
            ("actor".into(), Value::Entity(universe_core::EntityKey(1))),
            (
                "result_entity".into(),
                Value::Entity(universe_core::EntityKey(3)),
            ),
        ]);
        let interpreted = execute_program(
            &code,
            &mut Host,
            &inputs,
            Revision(10),
            Tick(5),
            ExecutionLimits {
                fuel: 20,
                max_proposals: 1,
            },
        )
        .unwrap();
        let compiled = execute(
            &bytecode,
            &mut Host,
            &inputs,
            Revision(10),
            Tick(5),
            ExecutionLimits {
                fuel: 20,
                max_proposals: 1,
            },
        )
        .unwrap();
        assert_eq!(interpreted, compiled);
        assert_eq!(compiled.proposals.len(), 1);
        assert_eq!(compiled.fuel_used, 17);
    }

    #[test]
    fn fuel_exhaustion_is_deterministic() {
        let error = execute_program(
            &fixture(),
            &mut Host,
            &BTreeMap::from([
                ("actor".into(), Value::Entity(universe_core::EntityKey(1))),
                (
                    "result_entity".into(),
                    Value::Entity(universe_core::EntityKey(3)),
                ),
            ]),
            Revision(0),
            Tick(0),
            ExecutionLimits {
                fuel: 2,
                max_proposals: 1,
            },
        )
        .unwrap_err();
        assert_eq!(error, VmError::FuelExhausted);
    }
}
