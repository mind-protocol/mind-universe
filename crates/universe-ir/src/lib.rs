//! Canonical, graph-materialized instruction representation.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use universe_core::{ContentPtr, EntityKey, Revision};
use universe_query::QueryBudget;

pub const IR_VERSION: u16 = 0;
pub type Register = u16;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    Unit,
    Bool(bool),
    Integer(i64),
    Text(String),
    Entity(EntityKey),
    Content(ContentPtr),
    List(Vec<Value>),
    Record(BTreeMap<String, Value>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QuerySpec {
    pub origin: Register,
    pub selector: Register,
    pub budget: QueryBudget,
    pub timeout_ticks: u32,
    pub allow_approximate: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operator {
    Input {
        name: String,
        output: Register,
    },
    Constant {
        value: Value,
        output: Register,
    },
    QueryOpen {
        spec: QuerySpec,
        output: Register,
    },
    QueryAwait {
        handle: Register,
        output: Register,
    },
    FollowOne {
        source: Register,
        predicate: Register,
        output: Register,
    },
    EntitySymbol {
        entity: Register,
        output: Register,
    },
    SelectMembers {
        input: Register,
        allowed: Register,
        max_items: u32,
        output: Register,
    },
    OrderByPreference {
        input: Register,
        preference: Register,
        max_items: u32,
        output: Register,
    },
    FilterTruthy {
        input: Register,
        field: String,
        max_items: u32,
        output: Register,
    },
    TopK {
        input: Register,
        score_field: String,
        limit: u32,
        output: Register,
    },
    Hydrate {
        input: Register,
        max_items: u32,
        max_bytes: u32,
        output: Register,
    },
    MakeRecord {
        fields: Vec<(String, Register)>,
        output: Register,
    },
    Propose {
        command: Register,
        output: Register,
    },
    Return {
        value: Register,
    },
}

impl Operator {
    pub fn output(&self) -> Option<Register> {
        match self {
            Self::Input { output, .. }
            | Self::Constant { output, .. }
            | Self::QueryOpen { output, .. }
            | Self::QueryAwait { output, .. }
            | Self::FollowOne { output, .. }
            | Self::EntitySymbol { output, .. }
            | Self::SelectMembers { output, .. }
            | Self::OrderByPreference { output, .. }
            | Self::FilterTruthy { output, .. }
            | Self::TopK { output, .. }
            | Self::Hydrate { output, .. }
            | Self::MakeRecord { output, .. }
            | Self::Propose { output, .. } => Some(*output),
            Self::Return { .. } => None,
        }
    }

    pub fn inputs(&self) -> Vec<Register> {
        match self {
            Self::Input { .. } | Self::Constant { .. } => vec![],
            Self::QueryOpen { spec, .. } => vec![spec.origin, spec.selector],
            Self::QueryAwait { handle, .. } => vec![*handle],
            Self::FollowOne {
                source, predicate, ..
            } => vec![*source, *predicate],
            Self::EntitySymbol { entity, .. } => vec![*entity],
            Self::SelectMembers { input, allowed, .. } => vec![*input, *allowed],
            Self::OrderByPreference {
                input, preference, ..
            } => vec![*input, *preference],
            Self::FilterTruthy { input, .. }
            | Self::TopK { input, .. }
            | Self::Hydrate { input, .. } => vec![*input],
            Self::MakeRecord { fields, .. } => {
                fields.iter().map(|(_, register)| *register).collect()
            }
            Self::Propose { command, .. } => vec![*command],
            Self::Return { value } => vec![*value],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeDefinition {
    pub ir_version: u16,
    pub revision: Revision,
    pub required_capabilities: Vec<String>,
    pub operators: Vec<Operator>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representation_round_trips() {
        let code = CodeDefinition {
            ir_version: IR_VERSION,
            revision: Revision(1),
            required_capabilities: vec!["local_query".into()],
            operators: vec![Operator::Input {
                name: "actor".into(),
                output: 0,
            }],
        };
        let encoded = serde_json::to_vec(&code).unwrap();
        assert_eq!(
            serde_json::from_slice::<CodeDefinition>(&encoded).unwrap(),
            code
        );
    }
}
