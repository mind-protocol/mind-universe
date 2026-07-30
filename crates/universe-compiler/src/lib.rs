//! Deterministic validation and compilation of graph-materialized IR.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use thiserror::Error;
use universe_core::Revision;
use universe_ir::{CodeDefinition, Operator, Register, IR_VERSION};

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CompileError {
    #[error("unsupported IR version {0}")]
    UnsupportedVersion(u16),
    #[error("program is empty")]
    Empty,
    #[error("register {0} is read before assignment")]
    ReadBeforeAssignment(Register),
    #[error("register {0} is assigned more than once")]
    DuplicateAssignment(Register),
    #[error("operator {0} has a zero bound")]
    ZeroBound(usize),
    #[error("return must be the final operator")]
    InvalidReturn,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Bytecode {
    pub ir_version: u16,
    pub code_revision: Revision,
    pub canonical_hash: String,
    pub required_capabilities: Vec<String>,
    pub instructions: Vec<Operator>,
    pub source_nodes: Vec<u32>,
}

pub fn canonical_hash(code: &CodeDefinition) -> Result<String, CompileError> {
    let bytes = serde_json::to_vec(code).expect("CodeDefinition serialization is infallible");
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub fn validate(code: &CodeDefinition) -> Result<(), CompileError> {
    if code.ir_version != IR_VERSION {
        return Err(CompileError::UnsupportedVersion(code.ir_version));
    }
    if code.operators.is_empty() {
        return Err(CompileError::Empty);
    }
    let mut assigned = BTreeSet::new();
    for (index, op) in code.operators.iter().enumerate() {
        for input in op.inputs() {
            if !assigned.contains(&input) {
                return Err(CompileError::ReadBeforeAssignment(input));
            }
        }
        match op {
            Operator::QueryOpen { spec, .. }
                if spec.budget.max_entities == 0
                    || spec.budget.max_relations == 0
                    || spec.budget.max_depth == 0
                    || spec.timeout_ticks == 0 =>
            {
                return Err(CompileError::ZeroBound(index));
            }
            Operator::FilterTruthy { max_items: 0, .. }
            | Operator::SelectMembers { max_items: 0, .. }
            | Operator::OrderByPreference { max_items: 0, .. }
            | Operator::TopK { limit: 0, .. }
            | Operator::Hydrate { max_items: 0, .. }
            | Operator::Hydrate { max_bytes: 0, .. } => {
                return Err(CompileError::ZeroBound(index));
            }
            Operator::Return { .. } if index + 1 != code.operators.len() => {
                return Err(CompileError::InvalidReturn);
            }
            _ => {}
        }
        if let Some(output) = op.output() {
            if !assigned.insert(output) {
                return Err(CompileError::DuplicateAssignment(output));
            }
        }
    }
    if !matches!(code.operators.last(), Some(Operator::Return { .. })) {
        return Err(CompileError::InvalidReturn);
    }
    Ok(())
}

pub fn compile(code: &CodeDefinition) -> Result<Bytecode, CompileError> {
    validate(code)?;
    let mut capabilities = code.required_capabilities.clone();
    capabilities.sort();
    capabilities.dedup();
    Ok(Bytecode {
        ir_version: code.ir_version,
        code_revision: code.revision,
        canonical_hash: canonical_hash(code)?,
        required_capabilities: capabilities,
        instructions: code.operators.clone(),
        source_nodes: (0..code.operators.len() as u32).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_ir::{Value, IR_VERSION};

    #[test]
    fn compilation_is_deterministic() {
        let code = CodeDefinition {
            ir_version: IR_VERSION,
            revision: Revision(3),
            required_capabilities: vec![],
            operators: vec![
                Operator::Constant {
                    value: Value::Unit,
                    output: 0,
                },
                Operator::Return { value: 0 },
            ],
        };
        assert_eq!(compile(&code).unwrap(), compile(&code).unwrap());
    }

    #[test]
    fn rejects_read_before_assignment() {
        let code = CodeDefinition {
            ir_version: IR_VERSION,
            revision: Revision(0),
            required_capabilities: vec![],
            operators: vec![Operator::Return { value: 9 }],
        };
        assert_eq!(validate(&code), Err(CompileError::ReadBeforeAssignment(9)));
    }

    #[test]
    fn graph_fixture_loads_validates_and_compiles() {
        let fixture = include_str!("../../../fixtures/graph-ir/minimal-read.json");
        let code: CodeDefinition = serde_json::from_str(fixture).unwrap();
        let artifact = compile(&code).unwrap();
        assert_eq!(artifact.instructions.len(), 17);
        assert_eq!(artifact.source_nodes, (0..17).collect::<Vec<_>>());
    }
}
