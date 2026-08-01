//! Fuel-bounded Graph IR virtual machine with a mutation-free host boundary.

use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use universe_compiler::{
    canonical_execution_request_hash, canonical_hash, compile, Bytecode, CompileError,
};
use universe_core::{EntityKey, Epistemic, Revision, Tick};
use universe_ir::{
    BooleanBinaryKind, CodeDefinition, ComparisonKind, EpistemicState, ExecutionRequest, Operator,
    Register, Value, TRIGGER_CONTRACT_VERSION,
};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggeredExecutionState {
    Completed,
    Rejected,
    Trapped,
}

/// Epistemic receipt for one pinned trigger execution.
///
/// A deterministic rejection or VM trap is a measured outcome, not a
/// `MeasurementFailed` claim. The latter remains reserved for inability to
/// measure what occurred.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TriggeredExecutionReceipt {
    pub request_id: String,
    pub request_hash: String,
    pub idempotency_key: String,
    pub subscription: EntityKey,
    pub subscription_revision: Revision,
    pub event_id: String,
    pub code_definition: EntityKey,
    pub code_revision: Revision,
    pub state: Epistemic<TriggeredExecutionState>,
    pub execution: Option<ExecutionReceipt>,
    pub reason: Option<String>,
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
    #[error("epistemic value is unavailable in state {0:?}")]
    EvidenceUnavailable(EpistemicState),
    #[error("proposal budget exhausted")]
    ProposalBudgetExhausted,
    #[error("call depth budget {limit} exceeded")]
    CallDepthExceeded { limit: u32 },
    #[error("loop iteration budget {limit} exhausted")]
    IterationBudgetExhausted { limit: u32 },
    #[error("selection holds {found} candidates, not exactly one")]
    NotExactlyOne { found: usize },
    #[error("record carries no field {field}")]
    MissingField { field: String },
}

/// One live call frame: where control resumes in the caller and which caller
/// register receives the callee's returned value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CallFrame {
    return_pc: usize,
    output: Register,
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
    fn call_capability(&mut self, capability: &str, input: &Value) -> Result<Value, String> {
        let _ = input;
        Err(format!("capability {capability} has no host adapter"))
    }
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

/// Executes only the CodeDefinition and authority revision pinned into a
/// bounded request produced by the trigger compiler.
///
/// Tick budget is enforced at admission through the inclusive request
/// deadline. Graph VM execution is synchronous and cannot advance Universe
/// time internally.
pub fn execute_trigger_request(
    code: &CodeDefinition,
    host: &mut impl VmHost,
    inputs: &BTreeMap<String, Value>,
    request: &ExecutionRequest,
    current_universe_revision: Revision,
    current_tick: Tick,
) -> TriggeredExecutionReceipt {
    let request_hash = canonical_execution_request_hash(request);
    let rejected = |reason: String| TriggeredExecutionReceipt {
        request_id: request.request_id.clone(),
        request_hash: request_hash.clone(),
        idempotency_key: request.idempotency_key.clone(),
        subscription: request.subscription,
        subscription_revision: request.subscription_revision,
        event_id: request.trigger.event_id.clone(),
        code_definition: request.code_definition,
        code_revision: request.code_revision,
        state: Epistemic::Measured(TriggeredExecutionState::Rejected),
        execution: None,
        reason: Some(reason),
    };
    if request.contract_version != TRIGGER_CONTRACT_VERSION {
        return rejected(format!(
            "unsupported trigger contract version {}",
            request.contract_version
        ));
    }
    if request.request_id.trim().is_empty() || request.idempotency_key.trim().is_empty() {
        return rejected("request identity is empty".into());
    }
    if current_universe_revision != request.starting_universe_revision {
        return rejected(format!(
            "starting Universe revision mismatch: request {}, current {}",
            request.starting_universe_revision.0, current_universe_revision.0
        ));
    }
    if current_tick.0 < request.issued_at_tick.0 {
        return rejected(format!(
            "execution tick {} precedes request tick {}",
            current_tick.0, request.issued_at_tick.0
        ));
    }
    if current_tick.0 > request.deadline_tick.0 {
        return rejected(format!(
            "execution tick {} exceeds deadline {}",
            current_tick.0, request.deadline_tick.0
        ));
    }
    if code.revision != request.code_revision {
        return rejected(format!(
            "CodeDefinition revision mismatch: request {}, supplied {}",
            request.code_revision.0, code.revision.0
        ));
    }
    let supplied_code_hash = match canonical_hash(code) {
        Ok(hash) => hash,
        Err(error) => return rejected(format!("CodeDefinition hash failed: {error}")),
    };
    if supplied_code_hash != request.code_hash {
        return rejected(format!(
            "CodeDefinition hash mismatch: request {}, supplied {}",
            request.code_hash, supplied_code_hash
        ));
    }
    if request.budgets.fuel == 0
        || request.budgets.max_mutations == 0
        || request.budgets.max_ticks == 0
    {
        return rejected("execution request contains a zero budget".into());
    }

    match execute_program(
        code,
        host,
        inputs,
        request.starting_universe_revision,
        current_tick,
        ExecutionLimits {
            fuel: request.budgets.fuel,
            max_proposals: request.budgets.max_mutations,
        },
    ) {
        Ok(execution) => TriggeredExecutionReceipt {
            request_id: request.request_id.clone(),
            request_hash,
            idempotency_key: request.idempotency_key.clone(),
            subscription: request.subscription,
            subscription_revision: request.subscription_revision,
            event_id: request.trigger.event_id.clone(),
            code_definition: request.code_definition,
            code_revision: request.code_revision,
            state: Epistemic::Measured(TriggeredExecutionState::Completed),
            execution: Some(execution),
            reason: None,
        },
        Err(error) => TriggeredExecutionReceipt {
            request_id: request.request_id.clone(),
            request_hash,
            idempotency_key: request.idempotency_key.clone(),
            subscription: request.subscription,
            subscription_revision: request.subscription_revision,
            event_id: request.trigger.event_id.clone(),
            code_definition: request.code_definition,
            code_revision: request.code_revision,
            state: Epistemic::Measured(TriggeredExecutionState::Trapped),
            execution: None,
            reason: Some(error.to_string()),
        },
    }
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
    let mut program_counter = 0usize;
    let mut call_stack: Vec<CallFrame> = Vec::new();
    // Iterations already charged against each bounded-loop latch, keyed by the
    // latch's fixed instruction index. Never reset across the program, so the
    // graph-owned budget is a hard ceiling on back-edge traversals.
    let mut loop_iterations: BTreeMap<usize, u32> = BTreeMap::new();

    while program_counter < bytecode.instructions.len() {
        let index = program_counter;
        let op = &bytecode.instructions[index];
        let mut next_program_counter = index + 1;
        let mut returned = false;
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
            Operator::Compare {
                left, right, kind, ..
            } => Some(Value::Bool(compare(
                register(&registers, *left)?,
                register(&registers, *right)?,
                *kind,
            )?)),
            Operator::BooleanBinary {
                left, right, kind, ..
            } => {
                let Value::Bool(left) = register(&registers, *left)? else {
                    return Err(VmError::Type { expected: "bool" });
                };
                let Value::Bool(right) = register(&registers, *right)? else {
                    return Err(VmError::Type { expected: "bool" });
                };
                Some(Value::Bool(match kind {
                    BooleanBinaryKind::And => *left && *right,
                    BooleanBinaryKind::Or => *left || *right,
                }))
            }
            Operator::BooleanNot { input, .. } => {
                let Value::Bool(value) = register(&registers, *input)? else {
                    return Err(VmError::Type { expected: "bool" });
                };
                Some(Value::Bool(!value))
            }
            Operator::EvidenceState { input, .. } => {
                let Value::Epistemic(evidence) = register(&registers, *input)? else {
                    return Err(VmError::Type {
                        expected: "epistemic value",
                    });
                };
                Some(Value::EpistemicState(epistemic_state(evidence)))
            }
            Operator::EvidenceValue { input, .. } => {
                let Value::Epistemic(evidence) = register(&registers, *input)? else {
                    return Err(VmError::Type {
                        expected: "epistemic value",
                    });
                };
                match evidence {
                    Epistemic::Observed(value) | Epistemic::Measured(value) => {
                        Some(value.as_ref().clone())
                    }
                    _ => return Err(VmError::EvidenceUnavailable(epistemic_state(evidence))),
                }
            }
            Operator::EvidenceCompare {
                left, right, kind, ..
            } => {
                let Value::Epistemic(left) = register(&registers, *left)? else {
                    return Err(VmError::Type {
                        expected: "epistemic value",
                    });
                };
                let Value::Epistemic(right) = register(&registers, *right)? else {
                    return Err(VmError::Type {
                        expected: "epistemic value",
                    });
                };
                Some(Value::Epistemic(evidence_compare(left, right, *kind)?))
            }
            Operator::EvidenceAll { inputs, .. } => {
                let evidence = inputs
                    .iter()
                    .map(|input| {
                        let Value::Epistemic(value) = register(&registers, *input)? else {
                            return Err(VmError::Type {
                                expected: "epistemic value",
                            });
                        };
                        Ok(value)
                    })
                    .collect::<Result<Vec<_>, VmError>>()?;
                Some(Value::Epistemic(evidence_all(&evidence)?))
            }
            Operator::Branch {
                condition,
                true_next,
                false_next,
            } => {
                let Value::Bool(condition) = register(&registers, *condition)? else {
                    return Err(VmError::Type { expected: "bool" });
                };
                next_program_counter = if *condition {
                    *true_next as usize
                } else {
                    *false_next as usize
                };
                None
            }
            Operator::BranchOnEvidence {
                input,
                observed_next,
                measured_next,
                known_absent_next,
                unknown_next,
                not_measured_next,
                measurement_failed_next,
            } => {
                let Value::Epistemic(evidence) = register(&registers, *input)? else {
                    return Err(VmError::Type {
                        expected: "epistemic value",
                    });
                };
                next_program_counter = match evidence {
                    Epistemic::Observed(_) => *observed_next,
                    Epistemic::Measured(_) => *measured_next,
                    Epistemic::KnownAbsent => *known_absent_next,
                    Epistemic::Unknown => *unknown_next,
                    Epistemic::NotMeasured => *not_measured_next,
                    Epistemic::MeasurementFailed { .. } => *measurement_failed_next,
                } as usize;
                None
            }
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
            Operator::CapabilityCall {
                capability, input, ..
            } => Some(
                host.call_capability(capability, register(&registers, *input)?)
                    .map_err(VmError::Host)?,
            ),
            Operator::MakeRecord { fields, .. } => Some(Value::Record(
                fields
                    .iter()
                    .map(|(name, register_id)| {
                        Ok((name.clone(), register(&registers, *register_id)?.clone()))
                    })
                    .collect::<Result<BTreeMap<_, _>, VmError>>()?,
            )),
            Operator::GetField { input, field, .. } => {
                let Value::Record(record) = register(&registers, *input)? else {
                    return Err(VmError::Type { expected: "record" });
                };
                // Absent is not Unit. A record that does not carry the field is a
                // fact, and coercing it into a value here would reintroduce
                // exactly the "missing data as zero" mistake.
                record
                    .get(field)
                    .cloned()
                    .map(Some)
                    .ok_or_else(|| VmError::MissingField {
                        field: field.clone(),
                    })?
            }
            Operator::Only { input, .. } => {
                let Value::List(items) = register(&registers, *input)? else {
                    return Err(VmError::Type { expected: "list" });
                };
                // Neither zero nor many is coerced. A selection that did not
                // narrow to one is a broken selection, and it must surface here
                // rather than downstream as a mutation of the wrong node.
                if items.len() != 1 {
                    return Err(VmError::NotExactlyOne { found: items.len() });
                }
                Some(items[0].clone())
            }
            Operator::ExtendRecord { input, fields, .. } => {
                // Start from the record as it IS. Every field the program does
                // not name survives untouched — that pass-through is the whole
                // reason this operator exists, and it is why a revision can be
                // committed without silently discarding the provenance of the
                // node being revised.
                let Value::Record(existing) = register(&registers, *input)? else {
                    return Err(VmError::Type { expected: "record" });
                };
                let mut extended = existing.clone();
                for (name, register_id) in fields {
                    extended.insert(name.clone(), register(&registers, *register_id)?.clone());
                }
                Some(Value::Record(extended))
            }
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
            Operator::Call {
                target,
                output,
                max_depth,
            } => {
                if call_stack.len() as u64 + 1 > u64::from(*max_depth) {
                    return Err(VmError::CallDepthExceeded { limit: *max_depth });
                }
                call_stack.push(CallFrame {
                    return_pc: index + 1,
                    output: *output,
                });
                next_program_counter = *target as usize;
                None
            }
            Operator::Return { value } => {
                let value = register(&registers, *value)?.clone();
                match call_stack.pop() {
                    Some(frame) => {
                        registers.insert(frame.output, value);
                        next_program_counter = frame.return_pc;
                    }
                    None => {
                        result = value;
                        returned = true;
                    }
                }
                None
            }
            Operator::RepeatUntilWithLimit {
                condition,
                loop_next,
                exit_next,
                max_iterations,
            } => {
                let Value::Epistemic(evidence) = register(&registers, *condition)? else {
                    return Err(VmError::Type {
                        expected: "epistemic bool",
                    });
                };
                // The condition is interpreted as "until": true closes the loop.
                // Unavailable evidence is preserved as a trap, never coerced into
                // a continue-or-stop decision.
                let until_satisfied = match evidence {
                    Epistemic::Observed(inner) | Epistemic::Measured(inner) => {
                        let Value::Bool(value) = inner.as_ref() else {
                            return Err(VmError::Type {
                                expected: "epistemic bool",
                            });
                        };
                        *value
                    }
                    _ => return Err(VmError::EvidenceUnavailable(epistemic_state(evidence))),
                };
                if until_satisfied {
                    next_program_counter = *exit_next as usize;
                } else {
                    let charged = loop_iterations.entry(index).or_insert(0);
                    if *charged >= *max_iterations {
                        return Err(VmError::IterationBudgetExhausted {
                            limit: *max_iterations,
                        });
                    }
                    *charged += 1;
                    next_program_counter = *loop_next as usize;
                }
                None
            }
        };
        if let (Some(output_id), Some(value)) = (op.output(), output) {
            registers.insert(output_id, value);
        }
        if returned {
            break;
        }
        program_counter = next_program_counter;
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

fn compare(left: &Value, right: &Value, kind: ComparisonKind) -> Result<bool, VmError> {
    match kind {
        ComparisonKind::Equal => Ok(left == right),
        ComparisonKind::NotEqual => Ok(left != right),
        ComparisonKind::LessThan
        | ComparisonKind::LessThanOrEqual
        | ComparisonKind::GreaterThan
        | ComparisonKind::GreaterThanOrEqual => {
            let (Value::Integer(left), Value::Integer(right)) = (left, right) else {
                return Err(VmError::Type {
                    expected: "integer operands",
                });
            };
            Ok(match kind {
                ComparisonKind::LessThan => left < right,
                ComparisonKind::LessThanOrEqual => left <= right,
                ComparisonKind::GreaterThan => left > right,
                ComparisonKind::GreaterThanOrEqual => left >= right,
                ComparisonKind::Equal | ComparisonKind::NotEqual => unreachable!(),
            })
        }
    }
}

fn epistemic_state(value: &Epistemic<Box<Value>>) -> EpistemicState {
    match value {
        Epistemic::Observed(_) => EpistemicState::Observed,
        Epistemic::Measured(_) => EpistemicState::Measured,
        Epistemic::KnownAbsent => EpistemicState::KnownAbsent,
        Epistemic::Unknown => EpistemicState::Unknown,
        Epistemic::NotMeasured => EpistemicState::NotMeasured,
        Epistemic::MeasurementFailed { .. } => EpistemicState::MeasurementFailed,
    }
}

fn evidence_compare(
    left: &Epistemic<Box<Value>>,
    right: &Epistemic<Box<Value>>,
    kind: ComparisonKind,
) -> Result<Epistemic<Box<Value>>, VmError> {
    match (left, right) {
        (Epistemic::MeasurementFailed { reason }, _)
        | (_, Epistemic::MeasurementFailed { reason }) => {
            return Ok(Epistemic::MeasurementFailed {
                reason: reason.clone(),
            });
        }
        (Epistemic::NotMeasured, _) | (_, Epistemic::NotMeasured) => {
            return Ok(Epistemic::NotMeasured);
        }
        (Epistemic::Unknown, _) | (_, Epistemic::Unknown) => {
            return Ok(Epistemic::Unknown);
        }
        (Epistemic::KnownAbsent, _) | (_, Epistemic::KnownAbsent) => {
            return Ok(Epistemic::KnownAbsent);
        }
        _ => {}
    }
    let (left_value, right_value) = match (left, right) {
        (Epistemic::Measured(left), Epistemic::Measured(right))
        | (Epistemic::Measured(left), Epistemic::Observed(right))
        | (Epistemic::Observed(left), Epistemic::Measured(right))
        | (Epistemic::Observed(left), Epistemic::Observed(right)) => (left, right),
        _ => unreachable!("unavailable evidence returned before value comparison"),
    };
    let result = Box::new(Value::Bool(compare(left_value, right_value, kind)?));
    Ok(
        if matches!(
            (left, right),
            (Epistemic::Measured(_), Epistemic::Measured(_))
        ) {
            Epistemic::Measured(result)
        } else {
            Epistemic::Observed(result)
        },
    )
}

fn evidence_all(evidence: &[&Epistemic<Box<Value>>]) -> Result<Epistemic<Box<Value>>, VmError> {
    let mut measured_result = true;
    for value in evidence {
        match value {
            Epistemic::Observed(inner) | Epistemic::Measured(inner) => {
                let Value::Bool(value) = inner.as_ref() else {
                    return Err(VmError::Type {
                        expected: "epistemic bool",
                    });
                };
                measured_result &= *value;
            }
            Epistemic::KnownAbsent
            | Epistemic::Unknown
            | Epistemic::NotMeasured
            | Epistemic::MeasurementFailed { .. } => {}
        }
    }
    if let Some(reason) = evidence.iter().find_map(|value| match value {
        Epistemic::MeasurementFailed { reason } => Some(reason),
        _ => None,
    }) {
        return Ok(Epistemic::MeasurementFailed {
            reason: reason.clone(),
        });
    }
    if evidence
        .iter()
        .any(|value| matches!(value, Epistemic::Observed(_) | Epistemic::NotMeasured))
    {
        Ok(Epistemic::NotMeasured)
    } else if evidence
        .iter()
        .any(|value| matches!(value, Epistemic::Unknown))
    {
        Ok(Epistemic::Unknown)
    } else if evidence
        .iter()
        .any(|value| matches!(value, Epistemic::KnownAbsent))
    {
        Ok(Epistemic::KnownAbsent)
    } else {
        Ok(Epistemic::Measured(Box::new(Value::Bool(measured_result))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_compiler::atom::{
        compile_atom_code_definition, AtomCompilationBudget, AtomCompileError,
    };
    use universe_compiler::{
        behavior_compilation_receipt_hash, behavior_loop_health_graph_inputs,
        build_execution_request, canonical_hash, decode_behavior_loop_health,
        BehaviorBondValidationReport, BehaviorCompilationReceipt, BehaviorCompilationStatus,
        BehaviorLoopHealthInput, RUNTIME_BOND_PLAN_VERSION,
    };
    use universe_ir::{
        BehaviorLoopClosure, BehaviorPhysicalEvidence, BehaviorReadbackEvidence, QuerySpec,
        TriggerEvent, TriggerEventKind, TriggerEventPayload, TriggerSubscription,
    };
    use universe_store::{GraphSeed, UniverseStore};

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

    /// Deterministic host for the bounded-loop fixture. The `poll` capability
    /// returns a fresh epistemic condition each iteration so the loop can either
    /// close cleanly, run to its iteration budget, or surface unavailable
    /// evidence without coercion.
    enum PollMode {
        CountTo(u32),
        AlwaysContinue,
        Unknown,
    }

    struct PollHost {
        mode: PollMode,
        calls: u32,
    }

    impl VmHost for PollHost {
        fn is_cancelled(&self) -> bool {
            false
        }
        fn capabilities(&self) -> BTreeSet<String> {
            BTreeSet::from(["poll".into()])
        }
        fn open_query(&mut self, _: &QuerySpec, _: &Value, _: &Value) -> Result<Value, String> {
            Err("poll host has no query support".into())
        }
        fn await_query(&mut self, _: &Value) -> Result<Value, String> {
            Err("poll host has no query support".into())
        }
        fn follow_one(&mut self, _: &Value, _: &Value) -> Result<Value, String> {
            Err("poll host has no follow support".into())
        }
        fn entity_symbol(&mut self, _: &Value) -> Result<Value, String> {
            Err("poll host has no symbol support".into())
        }
        fn hydrate(&mut self, _: &[Value], _: u32) -> Result<Vec<Value>, String> {
            Err("poll host has no hydrate support".into())
        }
        fn call_capability(&mut self, capability: &str, _: &Value) -> Result<Value, String> {
            assert_eq!(capability, "poll");
            self.calls += 1;
            Ok(match &self.mode {
                PollMode::CountTo(threshold) => Value::Epistemic(Epistemic::Measured(Box::new(
                    Value::Bool(self.calls > *threshold),
                ))),
                PollMode::AlwaysContinue => {
                    Value::Epistemic(Epistemic::Measured(Box::new(Value::Bool(false))))
                }
                PollMode::Unknown => Value::Epistemic(Epistemic::Unknown),
            })
        }
    }

    fn fixture() -> CodeDefinition {
        serde_json::from_str(include_str!("../../../fixtures/graph-ir/minimal-read.json")).unwrap()
    }

    /// A record standing in for a node's stored content as the store actually
    /// holds it: the fields a reviser knows about, and — crucially — several it
    /// does not.
    fn stored_node_content() -> Value {
        Value::Record(BTreeMap::from([
            ("canonical_id".into(), Value::Text("actor:l1:mind:claude-x".into())),
            ("provenance".into(), Value::Text("built".into())),
            ("embodied_session".into(), Value::Text("claude:x".into())),
            ("base_revision".into(), Value::Integer(44)),
            ("residency".into(), Value::Text("hot".into())),
        ]))
    }

    /// Program: take a stored record, set `residency` and attach a reason.
    fn extend_program(fields: Vec<(String, Register)>) -> CodeDefinition {
        CodeDefinition {
            ir_version: universe_ir::IR_VERSION,
            revision: Revision(1),
            required_capabilities: vec![],
            operators: vec![
                Operator::Input {
                    name: "node".into(),
                    output: 0,
                },
                Operator::Constant {
                    value: Value::Text("dormant".into()),
                    output: 1,
                },
                Operator::Constant {
                    value: Value::Text("crowded the beacon out of its budget".into()),
                    output: 2,
                },
                Operator::ExtendRecord {
                    input: 0,
                    fields,
                    output: 3,
                },
                Operator::Return { value: 3 },
            ],
        }
    }

    fn run_extend(node: Value, fields: Vec<(String, Register)>) -> Result<Value, VmError> {
        execute_program(
            &extend_program(fields),
            &mut Host,
            &BTreeMap::from([("node".to_string(), node)]),
            Revision(1),
            Tick(1),
            ExecutionLimits {
                fuel: 64,
                max_proposals: 0,
            },
        )
        .map(|receipt| receipt.result)
    }

    /// THE load-bearing property. A reviser that rebuilds a record from scratch
    /// silently discards whatever it does not understand — on a node being
    /// revised, that means discarding its provenance. Extension must carry the
    /// unnamed fields through untouched.
    #[test]
    fn extend_record_preserves_every_field_the_program_did_not_name() {
        let result = run_extend(
            stored_node_content(),
            vec![("reaping".into(), 2), ("residency".into(), 1)],
        )
        .expect("extension executes");
        let Value::Record(fields) = result else {
            panic!("expected a record, got {result:?}");
        };
        // Untouched: the program never named these, and does not know what two
        // of them mean.
        assert_eq!(
            fields.get("canonical_id"),
            Some(&Value::Text("actor:l1:mind:claude-x".into()))
        );
        assert_eq!(fields.get("provenance"), Some(&Value::Text("built".into())));
        assert_eq!(
            fields.get("embodied_session"),
            Some(&Value::Text("claude:x".into()))
        );
        assert_eq!(fields.get("base_revision"), Some(&Value::Integer(44)));
        // Added.
        assert_eq!(
            fields.get("reaping"),
            Some(&Value::Text("crowded the beacon out of its budget".into()))
        );
        // Nothing appeared that was neither stored nor named.
        assert_eq!(fields.len(), 6, "unexpected field set: {fields:?}");
    }

    /// Naming a field that already exists IS the intent to set it: revising a
    /// value in place is the point of the operator, not an accident of it.
    #[test]
    fn extend_record_writes_a_named_field_that_already_existed() {
        let before = stored_node_content();
        let Value::Record(before_fields) = &before else {
            unreachable!()
        };
        assert_eq!(
            before_fields.get("residency"),
            Some(&Value::Text("hot".into())),
            "the fixture must start hot for this test to mean anything"
        );
        let result = run_extend(before, vec![("residency".into(), 1)]).expect("extension executes");
        let Value::Record(fields) = result else {
            panic!("expected a record");
        };
        assert_eq!(fields.get("residency"), Some(&Value::Text("dormant".into())));
        // and the rest still survives the overwrite
        assert_eq!(fields.get("provenance"), Some(&Value::Text("built".into())));
        assert_eq!(fields.len(), 5);
    }

    /// `GetField` is what lets a program act on what it observed rather than on
    /// what its caller handed it. An absent field must trap, not read as `Unit`.
    #[test]
    fn get_field_binds_a_field_and_refuses_to_invent_an_absent_one() {
        let program = |field: &str| {
            execute_program(
                &CodeDefinition {
                    ir_version: universe_ir::IR_VERSION,
                    revision: Revision(1),
                    required_capabilities: vec![],
                    operators: vec![
                        Operator::Input {
                            name: "node".into(),
                            output: 0,
                        },
                        Operator::GetField {
                            input: 0,
                            field: field.to_string(),
                            output: 1,
                        },
                        Operator::Return { value: 1 },
                    ],
                },
                &mut Host,
                &BTreeMap::from([("node".to_string(), stored_node_content())]),
                Revision(1),
                Tick(1),
                ExecutionLimits {
                    fuel: 32,
                    max_proposals: 0,
                },
            )
            .map(|receipt| receipt.result)
        };

        assert_eq!(
            program("base_revision").expect("a present field binds"),
            Value::Integer(44)
        );
        // The node records no lifetime. That is not a zero and not a Unit.
        match program("expires_at") {
            Err(VmError::MissingField { field }) => assert_eq!(field, "expires_at"),
            other => panic!("an absent field must trap, got {other:?}"),
        }
    }

    /// `Only` is the single bridge from a selection (always a list) to a thing.
    /// It is strict on purpose: taking a head would turn a selection that failed
    /// to narrow into an attributable mutation of whichever node sorted first.
    #[test]
    fn only_binds_the_sole_element_and_refuses_to_choose() {
        let one = Value::List(vec![stored_node_content()]);
        let program = |input: Value| {
            execute_program(
                &CodeDefinition {
                    ir_version: universe_ir::IR_VERSION,
                    revision: Revision(1),
                    required_capabilities: vec![],
                    operators: vec![
                        Operator::Input {
                            name: "candidates".into(),
                            output: 0,
                        },
                        Operator::Only {
                            input: 0,
                            output: 1,
                        },
                        Operator::Return { value: 1 },
                    ],
                },
                &mut Host,
                &BTreeMap::from([("candidates".to_string(), input)]),
                Revision(1),
                Tick(1),
                ExecutionLimits {
                    fuel: 32,
                    max_proposals: 0,
                },
            )
            .map(|receipt| receipt.result)
        };

        assert_eq!(
            program(one).expect("a one-element selection binds"),
            stored_node_content()
        );

        // Nothing matched: not a null to carry onward.
        assert!(
            matches!(
                program(Value::List(vec![])),
                Err(VmError::NotExactlyOne { found: 0 })
            ),
            "an empty selection must trap"
        );

        // Three matched: the program believed it had one. It must not proceed on
        // whichever happened to sort first.
        assert!(
            matches!(
                program(Value::List(vec![
                    stored_node_content(),
                    stored_node_content(),
                    stored_node_content(),
                ])),
                Err(VmError::NotExactlyOne { found: 3 })
            ),
            "a selection that did not narrow must trap, never pick"
        );
    }

    /// An extension naming no field never reaches the VM: it is a copy that
    /// would advance a generation and retain nothing, so the validator refuses
    /// it exactly as it refuses an empty `EvidenceAll`.
    #[test]
    fn an_extension_that_names_no_field_is_rejected_at_validation() {
        let error =
            run_extend(stored_node_content(), vec![]).expect_err("an empty extension must not compile");
        // Operator 3 is the ExtendRecord in `extend_program`.
        assert!(
            matches!(error, VmError::Compile(CompileError::ZeroBound(3))),
            "expected a zero-bound rejection at the extension, got {error:?}"
        );
    }

    /// A non-record input is a deterministic type error. It must never be
    /// silently promoted into a fresh record — that would be `MakeRecord`
    /// wearing this operator's name, and would lose exactly what it exists to
    /// keep.
    #[test]
    fn extend_record_rejects_a_non_record_input() {
        let error = run_extend(Value::Text("not a record".into()), vec![("residency".into(), 1)])
            .expect_err("a non-record input must not extend");
        assert!(
            matches!(error, VmError::Type { expected: "record" }),
            "expected a record type error, got {error:?}"
        );
    }

    fn repeat_until_fixture() -> CodeDefinition {
        serde_json::from_str(include_str!(
            "../../../fixtures/graph-ir/repeat-until-limit.json"
        ))
        .unwrap()
    }

    fn boolean_fixture() -> CodeDefinition {
        serde_json::from_str(include_str!(
            "../../../fixtures/graph-ir/boolean-comparison.json"
        ))
        .unwrap()
    }

    fn epistemic_fixture() -> CodeDefinition {
        serde_json::from_str(include_str!(
            "../../../fixtures/graph-ir/epistemic-path.json"
        ))
        .unwrap()
    }

    fn branch_fixture() -> CodeDefinition {
        serde_json::from_str(include_str!("../../../fixtures/graph-ir/branch.json")).unwrap()
    }

    fn evidence_branch_fixture() -> CodeDefinition {
        serde_json::from_str(include_str!(
            "../../../fixtures/graph-ir/evidence-branch.json"
        ))
        .unwrap()
    }

    fn call_depth_fixture() -> CodeDefinition {
        serde_json::from_str(include_str!("../../../fixtures/graph-ir/call-depth.json")).unwrap()
    }

    fn behavior_loop_health_fixture() -> CodeDefinition {
        serde_json::from_str(include_str!(
            "../../../fixtures/graph-ir/behavior-loop-health.json"
        ))
        .unwrap()
    }

    fn triggered_code() -> CodeDefinition {
        CodeDefinition {
            ir_version: universe_ir::IR_VERSION,
            revision: Revision(11),
            required_capabilities: vec![],
            operators: vec![
                Operator::Input {
                    name: "event".into(),
                    output: 0,
                },
                Operator::Return { value: 0 },
            ],
        }
    }

    fn trigger_request(code: &CodeDefinition) -> ExecutionRequest {
        let mut subscription: TriggerSubscription = serde_json::from_str(include_str!(
            "../../../fixtures/graph-ir/trigger-subscription.json"
        ))
        .unwrap();
        subscription.code_revision = code.revision;
        subscription.code_hash = canonical_hash(code).unwrap();
        let event = TriggerEvent {
            event_id: "vm-event".into(),
            kind: TriggerEventKind::LocalObservation,
            source_revision: Revision(12),
            occurred_at: Tick(20),
            observed_at: Tick(20),
            evidence: Epistemic::Measured(TriggerEventPayload {
                subject: None,
                fields: BTreeMap::new(),
                receipt_hash: None,
            }),
            causal_ancestry: vec![],
        };
        build_execution_request(&subscription, &event, Revision(12), Tick(20))
            .request
            .unwrap()
    }

    #[test]
    fn trigger_request_executes_only_its_pinned_code_and_budgets() {
        let code = triggered_code();
        let request = trigger_request(&code);
        let inputs = BTreeMap::from([("event".into(), Value::Text("measured".into()))]);
        let receipt =
            execute_trigger_request(&code, &mut Host, &inputs, &request, Revision(12), Tick(21));
        assert_eq!(
            receipt.state,
            Epistemic::Measured(TriggeredExecutionState::Completed)
        );
        let execution = receipt.execution.unwrap();
        assert_eq!(execution.code_revision, Revision(11));
        assert_eq!(execution.starting_universe_revision, Revision(12));
        assert_eq!(execution.starting_tick, Tick(21));
        assert_eq!(execution.fuel_used, 2);
        assert_eq!(execution.result, Value::Text("measured".into()));
    }

    #[test]
    fn trigger_request_rejects_changed_code_or_stale_deadline() {
        let code = triggered_code();
        let request = trigger_request(&code);
        let mut changed_code = code.clone();
        changed_code.revision = Revision(12);
        let changed = execute_trigger_request(
            &changed_code,
            &mut Host,
            &BTreeMap::new(),
            &request,
            Revision(12),
            Tick(21),
        );
        assert_eq!(
            changed.state,
            Epistemic::Measured(TriggeredExecutionState::Rejected)
        );
        assert!(changed.reason.unwrap().contains("revision mismatch"));

        let stale = execute_trigger_request(
            &code,
            &mut Host,
            &BTreeMap::new(),
            &request,
            Revision(12),
            Tick(request.deadline_tick.0 + 1),
        );
        assert_eq!(
            stale.state,
            Epistemic::Measured(TriggeredExecutionState::Rejected)
        );
        assert!(stale.reason.unwrap().contains("exceeds deadline"));
    }

    fn measured_behavior_loop_health_input() -> BehaviorLoopHealthInput {
        let bond = universe_core::EntityKey(0x3070);
        let projection_hash = "1".repeat(64);
        let artifact_hash = "2".repeat(64);
        let execution_receipt_hash = "3".repeat(64);
        let compilation = BehaviorCompilationReceipt {
            plan_version: RUNTIME_BOND_PLAN_VERSION,
            bond,
            behavior_hash: "4".repeat(64),
            projection_hash: Some(projection_hash.clone()),
            artifact_hash: Some(artifact_hash.clone()),
            authority: None,
            status: BehaviorCompilationStatus::Compiled,
            validation: BehaviorBondValidationReport {
                bond,
                behavior_hash: "4".repeat(64),
                authority: None,
                valid: true,
                issues: Vec::new(),
            },
        };
        let compilation_receipt_hash = behavior_compilation_receipt_hash(&compilation);
        BehaviorLoopHealthInput {
            compilation: Epistemic::Measured(compilation),
            physical: Epistemic::Measured(BehaviorPhysicalEvidence {
                behavior_bond: bond,
                artifact_hash: artifact_hash.clone(),
                execution_receipt_hash: execution_receipt_hash.clone(),
                converged: true,
                energy_conserved: true,
                contained: true,
                released: true,
                lifetime_within_limit: true,
            }),
            readback: Epistemic::Measured(BehaviorReadbackEvidence {
                behavior_bond: bond,
                projection_hash,
                compilation_receipt_hash,
                artifact_hash,
                execution_receipt_hash,
                independent_readback_hash: "5".repeat(64),
                content_hashes_verified: true,
                contradictory: false,
            }),
        }
    }

    fn execute_behavior_loop_health_receipt(input: &BehaviorLoopHealthInput) -> ExecutionReceipt {
        execute_program(
            &behavior_loop_health_fixture(),
            &mut Host,
            &behavior_loop_health_graph_inputs(input),
            Revision(30),
            Tick(10),
            ExecutionLimits {
                fuel: 64,
                max_proposals: 0,
            },
        )
        .unwrap()
    }

    fn execute_behavior_loop_health(
        input: &BehaviorLoopHealthInput,
    ) -> Epistemic<BehaviorLoopClosure> {
        decode_behavior_loop_health(&execute_behavior_loop_health_receipt(input).result).unwrap()
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

    #[test]
    fn graph_boolean_and_comparison_operators_execute_deterministically() {
        let receipt = execute_program(
            &boolean_fixture(),
            &mut Host,
            &BTreeMap::new(),
            Revision(12),
            Tick(4),
            ExecutionLimits {
                fuel: 16,
                max_proposals: 0,
            },
        )
        .unwrap();
        assert_eq!(receipt.result, Value::Bool(true));
        assert_eq!(receipt.fuel_used, 8);
    }

    #[test]
    fn ordered_comparison_rejects_non_integer_operands() {
        let mut code = boolean_fixture();
        code.operators[0] = Operator::Constant {
            value: Value::Text("not-an-integer".into()),
            output: 0,
        };
        let error = execute_program(
            &code,
            &mut Host,
            &BTreeMap::new(),
            Revision(12),
            Tick(4),
            ExecutionLimits {
                fuel: 16,
                max_proposals: 0,
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            VmError::Type {
                expected: "integer operands"
            }
        );
    }

    #[test]
    fn graph_program_reads_epistemic_state_before_unwrapping_value() {
        let receipt = execute_program(
            &epistemic_fixture(),
            &mut Host,
            &BTreeMap::new(),
            Revision(20),
            Tick(7),
            ExecutionLimits {
                fuel: 16,
                max_proposals: 0,
            },
        )
        .unwrap();
        assert_eq!(receipt.result, Value::Bool(true));
    }

    #[test]
    fn unknown_evidence_is_not_coerced_to_false_or_zero() {
        let mut code = epistemic_fixture();
        code.operators[0] = Operator::Constant {
            value: Value::Epistemic(Epistemic::Unknown),
            output: 0,
        };
        let error = execute_program(
            &code,
            &mut Host,
            &BTreeMap::new(),
            Revision(20),
            Tick(7),
            ExecutionLimits {
                fuel: 16,
                max_proposals: 0,
            },
        )
        .unwrap_err();
        assert_eq!(error, VmError::EvidenceUnavailable(EpistemicState::Unknown));
    }

    #[test]
    fn graph_owned_behavior_loop_health_closes_with_complete_measured_proof() {
        let input = measured_behavior_loop_health_input();
        let receipt = execute_behavior_loop_health_receipt(&input);
        assert_eq!(receipt.fuel_used, 38);
        assert_eq!(receipt.trace.len(), 38);
        assert!(receipt.proposals.is_empty());
        let graph_health = decode_behavior_loop_health(&receipt.result).unwrap();
        assert_eq!(
            graph_health,
            Epistemic::Measured(BehaviorLoopClosure::Closed)
        );
    }

    #[test]
    fn graph_owned_behavior_loop_health_preserves_unavailable_states() {
        let mut observed = measured_behavior_loop_health_input();
        let Epistemic::Measured(receipt) = observed.compilation else {
            panic!("fixture compilation must be measured");
        };
        observed.compilation = Epistemic::Observed(receipt);
        assert_eq!(
            execute_behavior_loop_health(&observed),
            Epistemic::NotMeasured
        );

        let mut unknown = measured_behavior_loop_health_input();
        unknown.compilation = Epistemic::Unknown;
        assert_eq!(execute_behavior_loop_health(&unknown), Epistemic::Unknown);

        let mut known_absent = measured_behavior_loop_health_input();
        known_absent.physical = Epistemic::KnownAbsent;
        assert_eq!(
            execute_behavior_loop_health(&known_absent),
            Epistemic::KnownAbsent
        );

        let mut not_measured = measured_behavior_loop_health_input();
        not_measured.readback = Epistemic::NotMeasured;
        assert_eq!(
            execute_behavior_loop_health(&not_measured),
            Epistemic::NotMeasured
        );

        let mut failed = measured_behavior_loop_health_input();
        failed.physical = Epistemic::MeasurementFailed {
            reason: "physics measurement failed".into(),
        };
        assert_eq!(
            execute_behavior_loop_health(&failed),
            Epistemic::MeasurementFailed {
                reason: "physical: physics measurement failed".into(),
            }
        );
    }

    #[test]
    fn graph_owned_behavior_loop_health_is_non_compensatory() {
        let mut contradiction = measured_behavior_loop_health_input();
        let Epistemic::Measured(readback) = &mut contradiction.readback else {
            panic!("fixture readback must be measured");
        };
        readback.contradictory = true;
        assert_eq!(
            execute_behavior_loop_health(&contradiction),
            Epistemic::Measured(BehaviorLoopClosure::Open)
        );

        let mut physical_failure = measured_behavior_loop_health_input();
        let Epistemic::Measured(physical) = &mut physical_failure.physical else {
            panic!("fixture physical evidence must be measured");
        };
        physical.energy_conserved = false;
        assert_eq!(
            execute_behavior_loop_health(&physical_failure),
            Epistemic::Measured(BehaviorLoopClosure::Open)
        );

        let mut mismatched_artifact = measured_behavior_loop_health_input();
        let Epistemic::Measured(readback) = &mut mismatched_artifact.readback else {
            panic!("fixture readback must be measured");
        };
        readback.artifact_hash = "9".repeat(64);
        assert_eq!(
            execute_behavior_loop_health(&mismatched_artifact),
            Epistemic::Measured(BehaviorLoopClosure::Open)
        );
    }

    #[test]
    fn branch_only_executes_the_selected_path() {
        let false_receipt = execute_program(
            &branch_fixture(),
            &mut Host,
            &BTreeMap::from([("eligible".into(), Value::Bool(false))]),
            Revision(21),
            Tick(8),
            ExecutionLimits {
                fuel: 8,
                max_proposals: 1,
            },
        )
        .unwrap();
        assert!(false_receipt.proposals.is_empty());
        assert_eq!(false_receipt.fuel_used, 4);

        let true_receipt = execute_program(
            &branch_fixture(),
            &mut Host,
            &BTreeMap::from([("eligible".into(), Value::Bool(true))]),
            Revision(21),
            Tick(8),
            ExecutionLimits {
                fuel: 8,
                max_proposals: 1,
            },
        )
        .unwrap();
        assert_eq!(true_receipt.proposals.len(), 1);
        assert_eq!(true_receipt.fuel_used, 5);
    }

    #[test]
    fn evidence_branch_routes_each_state_without_coercion() {
        let cases = [
            (
                Value::Epistemic(Epistemic::Observed(Box::new(Value::Integer(1)))),
                "observed",
            ),
            (
                Value::Epistemic(Epistemic::Measured(Box::new(Value::Integer(2)))),
                "measured",
            ),
            (Value::Epistemic(Epistemic::KnownAbsent), "known_absent"),
            (Value::Epistemic(Epistemic::Unknown), "unknown"),
            (Value::Epistemic(Epistemic::NotMeasured), "not_measured"),
            (
                Value::Epistemic(Epistemic::MeasurementFailed {
                    reason: "sensor offline".into(),
                }),
                "measurement_failed",
            ),
        ];
        let code = evidence_branch_fixture();
        let bytecode = compile(&code).unwrap();
        for (evidence, expected) in cases {
            let inputs = BTreeMap::from([("evidence".into(), evidence)]);
            let interpreted = execute_program(
                &code,
                &mut Host,
                &inputs,
                Revision(30),
                Tick(9),
                ExecutionLimits {
                    fuel: 8,
                    max_proposals: 0,
                },
            )
            .unwrap();
            let compiled = execute(
                &bytecode,
                &mut Host,
                &inputs,
                Revision(30),
                Tick(9),
                ExecutionLimits {
                    fuel: 8,
                    max_proposals: 0,
                },
            )
            .unwrap();
            assert_eq!(interpreted, compiled);
            assert_eq!(interpreted.result, Value::Text(expected.into()));
            assert_eq!(interpreted.fuel_used, 4);
        }
    }

    #[test]
    fn call_returns_callee_value_and_resumes_caller() {
        let code = call_depth_fixture();
        let bytecode = compile(&code).unwrap();
        let interpreted = execute_program(
            &code,
            &mut Host,
            &BTreeMap::new(),
            Revision(40),
            Tick(11),
            ExecutionLimits {
                fuel: 16,
                max_proposals: 0,
            },
        )
        .unwrap();
        let compiled = execute(
            &bytecode,
            &mut Host,
            &BTreeMap::new(),
            Revision(40),
            Tick(11),
            ExecutionLimits {
                fuel: 16,
                max_proposals: 0,
            },
        )
        .unwrap();
        assert_eq!(interpreted, compiled);
        // Two nested calls (depth 2) resolve to the innermost constant, proving
        // control returns to each caller and the returned value is bound into
        // the caller's output register.
        assert_eq!(interpreted.result, Value::Integer(7));
        // call, call, constant, return, return, return.
        assert_eq!(interpreted.fuel_used, 6);
    }

    #[test]
    fn call_depth_budget_traps_deterministically() {
        // The budget lives in graph data: tightening the inner call's declared
        // max_depth below the live nesting depth must trap, never truncate.
        let mut code = call_depth_fixture();
        let Operator::Call { max_depth, .. } = &mut code.operators[2] else {
            panic!("fixture operator 2 must be a call");
        };
        *max_depth = 1;
        let error = execute_program(
            &code,
            &mut Host,
            &BTreeMap::new(),
            Revision(40),
            Tick(11),
            ExecutionLimits {
                fuel: 16,
                max_proposals: 0,
            },
        )
        .unwrap_err();
        assert_eq!(error, VmError::CallDepthExceeded { limit: 1 });
    }

    #[test]
    fn bounded_loop_repeats_then_exits_cleanly() {
        let code = repeat_until_fixture();
        let bytecode = compile(&code).unwrap();
        let interpreted = execute_program(
            &code,
            &mut PollHost {
                mode: PollMode::CountTo(3),
                calls: 0,
            },
            &BTreeMap::new(),
            Revision(50),
            Tick(12),
            ExecutionLimits {
                fuel: 64,
                max_proposals: 0,
            },
        )
        .unwrap();
        let compiled = execute(
            &bytecode,
            &mut PollHost {
                mode: PollMode::CountTo(3),
                calls: 0,
            },
            &BTreeMap::new(),
            Revision(50),
            Tick(12),
            ExecutionLimits {
                fuel: 64,
                max_proposals: 0,
            },
        )
        .unwrap();
        assert_eq!(interpreted, compiled);
        // Three back-edges (poll returns false, false, false) then a clean exit
        // on the fourth poll: constant + (poll, latch) x 4 + constant + return.
        assert_eq!(interpreted.result, Value::Text("loop-exited".into()));
        assert_eq!(interpreted.fuel_used, 11);
    }

    #[test]
    fn bounded_loop_traps_when_iteration_budget_exhausted() {
        // The graph-owned budget is 8; an always-continue condition must trap
        // deterministically instead of looping unbounded.
        let error = execute_program(
            &repeat_until_fixture(),
            &mut PollHost {
                mode: PollMode::AlwaysContinue,
                calls: 0,
            },
            &BTreeMap::new(),
            Revision(50),
            Tick(12),
            ExecutionLimits {
                fuel: 64,
                max_proposals: 0,
            },
        )
        .unwrap_err();
        assert_eq!(error, VmError::IterationBudgetExhausted { limit: 8 });
    }

    #[test]
    fn bounded_loop_does_not_coerce_unavailable_condition() {
        // An unknown loop condition is preserved as a trap, never read as
        // continue or stop.
        let error = execute_program(
            &repeat_until_fixture(),
            &mut PollHost {
                mode: PollMode::Unknown,
                calls: 0,
            },
            &BTreeMap::new(),
            Revision(50),
            Tick(12),
            ExecutionLimits {
                fuel: 64,
                max_proposals: 0,
            },
        )
        .unwrap_err();
        assert_eq!(error, VmError::EvidenceUnavailable(EpistemicState::Unknown));
    }

    #[test]
    fn evidence_branch_rejects_non_epistemic_input() {
        let error = execute_program(
            &evidence_branch_fixture(),
            &mut Host,
            &BTreeMap::from([("evidence".into(), Value::Bool(true))]),
            Revision(30),
            Tick(9),
            ExecutionLimits {
                fuel: 8,
                max_proposals: 0,
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            VmError::Type {
                expected: "epistemic value"
            }
        );
    }

    #[test]
    fn atom_repeat_loads_reopens_compiles_and_executes_with_receipt() {
        let seed: GraphSeed = serde_json::from_str(include_str!(
            "../../../fixtures/graph-ir/atom-repeat-seed.json"
        ))
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let installed = store.install_seed(&seed).unwrap();
        let installed_hash = installed.canonical_hash().unwrap();

        let independent_store = UniverseStore::open(temp.path()).unwrap();
        let independent = independent_store.load_snapshot().unwrap();
        assert_eq!(independent.canonical_hash().unwrap(), installed_hash);

        let compilation = compile_atom_code_definition(
            &independent_store,
            &independent,
            universe_core::EntityKey(0x4000),
            AtomCompilationBudget {
                max_entities: 16,
                max_relations: 16,
                max_operators: 8,
            },
        )
        .unwrap();
        assert_eq!(compilation.bytecode.instructions.len(), 5);
        assert_eq!(compilation.receipt.artifact_hash.len(), 64);
        assert_eq!(compilation.receipt.snapshot_hash, installed_hash);
        assert_eq!(
            compilation.receipt.source_atoms,
            vec![
                universe_core::EntityKey(0x4001),
                universe_core::EntityKey(0x4003),
                universe_core::EntityKey(0x4003),
                universe_core::EntityKey(0x4003),
                universe_core::EntityKey(0x4004),
            ]
        );

        let receipt = execute(
            &compilation.bytecode,
            &mut Host,
            &BTreeMap::new(),
            independent.revision,
            independent.tick,
            ExecutionLimits {
                fuel: 8,
                max_proposals: 3,
            },
        )
        .unwrap();
        assert_eq!(receipt.proposals.len(), 3);
        assert_eq!(receipt.fuel_used, 5);
        assert_eq!(receipt.result, Value::Text("bounded-atom-intent".into()));
        assert_eq!(receipt.starting_universe_revision, independent.revision);
    }

    #[test]
    fn atom_repeat_rejects_iterations_above_graph_budget() {
        let mut seed: GraphSeed = serde_json::from_str(include_str!(
            "../../../fixtures/graph-ir/atom-repeat-seed.json"
        ))
        .unwrap();
        seed.entities
            .iter_mut()
            .find(|entity| entity.key == universe_core::EntityKey(0x4020))
            .unwrap()
            .content["value"]["value"] = serde_json::json!(5);
        let temp = tempfile::tempdir().unwrap();
        let store = UniverseStore::open(temp.path()).unwrap();
        let snapshot = store.install_seed(&seed).unwrap();
        let error = compile_atom_code_definition(
            &store,
            &snapshot,
            universe_core::EntityKey(0x4000),
            AtomCompilationBudget {
                max_entities: 16,
                max_relations: 16,
                max_operators: 8,
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            AtomCompileError::Budget("REPEAT_N requests 5 iterations, budget is 4".into())
        );
    }
}
