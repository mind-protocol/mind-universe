use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::PathBuf;
use universe_core::{Revision, Tick};
use universe_embeddings::{EmbeddingRuntime, NodeTransformersProvider};
use universe_ir::{CodeDefinition, ComparisonKind, Operator, Value, IR_VERSION};
use universe_vm::{execute_program, ExecutionLimits, VmHost};

struct Host {
    runtime: EmbeddingRuntime<NodeTransformersProvider>,
}

impl VmHost for Host {
    fn is_cancelled(&self) -> bool {
        false
    }

    fn capabilities(&self) -> BTreeSet<String> {
        BTreeSet::from(["embedding.encode".into(), "vector.cosine".into()])
    }

    fn open_query(
        &mut self,
        _: &universe_ir::QuerySpec,
        _: &Value,
        _: &Value,
    ) -> Result<Value, String> {
        Err("query not used".into())
    }

    fn await_query(&mut self, _: &Value) -> Result<Value, String> {
        Err("query not used".into())
    }

    fn follow_one(&mut self, _: &Value, _: &Value) -> Result<Value, String> {
        Err("follow not used".into())
    }

    fn entity_symbol(&mut self, _: &Value) -> Result<Value, String> {
        Err("symbol not used".into())
    }

    fn hydrate(&mut self, _: &[Value], _: u32) -> Result<Vec<Value>, String> {
        Err("hydrate not used".into())
    }

    fn call_capability(&mut self, capability: &str, input: &Value) -> Result<Value, String> {
        self.runtime.call(capability, input)
    }
}

fn encode_request(model: &str, revision: &str, text: &str) -> Value {
    Value::Record(BTreeMap::from([
        ("model".into(), Value::Text(model.into())),
        ("model_revision".into(), Value::Text(revision.into())),
        ("text".into(), Value::Text(text.into())),
    ]))
}

fn main() {
    let module_dir = PathBuf::from(env::var("EMBEDDING_MODULE_DIR").expect("EMBEDDING_MODULE_DIR"));
    let cache_dir = PathBuf::from(env::var("EMBEDDING_CACHE_DIR").expect("EMBEDDING_CACHE_DIR"));
    let model =
        env::var("EMBEDDING_MODEL").unwrap_or_else(|_| "Xenova/multilingual-e5-small".into());
    let revision = env::var("EMBEDDING_MODEL_REVISION").expect("EMBEDDING_MODEL_REVISION");
    let subject = env::args().nth(1).unwrap_or_else(|| {
        "Une chambre intérieure avec quatre murs, une porte et un plafond.".into()
    });
    let provider = NodeTransformersProvider {
        node_executable: PathBuf::from("node"),
        module_dir,
        cache_dir,
        model: model.clone(),
        model_revision: revision.clone(),
        prefix: "query: ".into(),
        allow_remote: false,
    };
    let mut host = Host {
        runtime: EmbeddingRuntime::new(provider, 384),
    };
    let code = CodeDefinition {
        ir_version: IR_VERSION,
        revision: Revision(1),
        required_capabilities: vec!["embedding.encode".into(), "vector.cosine".into()],
        operators: vec![
            Operator::Input {
                name: "space_embedding_request".into(),
                output: 0,
            },
            Operator::CapabilityCall {
                capability: "embedding.encode".into(),
                input: 0,
                output: 1,
            },
            Operator::Constant {
                value: encode_request(
                    &model,
                    &revision,
                    "A closed interior space enclosed by walls, floor and ceiling.",
                ),
                output: 2,
            },
            Operator::CapabilityCall {
                capability: "embedding.encode".into(),
                input: 2,
                output: 3,
            },
            Operator::MakeRecord {
                fields: vec![("left".into(), 1), ("right".into(), 3)],
                output: 4,
            },
            Operator::CapabilityCall {
                capability: "vector.cosine".into(),
                input: 4,
                output: 5,
            },
            Operator::Constant {
                value: encode_request(
                    &model,
                    &revision,
                    "An open exterior space without enclosing walls or roof.",
                ),
                output: 6,
            },
            Operator::CapabilityCall {
                capability: "embedding.encode".into(),
                input: 6,
                output: 7,
            },
            Operator::MakeRecord {
                fields: vec![("left".into(), 1), ("right".into(), 7)],
                output: 8,
            },
            Operator::CapabilityCall {
                capability: "vector.cosine".into(),
                input: 8,
                output: 9,
            },
            Operator::Compare {
                left: 5,
                right: 9,
                kind: ComparisonKind::GreaterThan,
                output: 10,
            },
            Operator::Branch {
                condition: 10,
                true_next: 12,
                false_next: 15,
            },
            Operator::Constant {
                value: Value::Text("room".into()),
                output: 11,
            },
            Operator::MakeRecord {
                fields: vec![
                    ("profile".into(), 11),
                    ("closed_score".into(), 5),
                    ("open_score".into(), 9),
                ],
                output: 13,
            },
            Operator::Return { value: 13 },
            Operator::Constant {
                value: Value::Text("terrace".into()),
                output: 12,
            },
            Operator::MakeRecord {
                fields: vec![
                    ("profile".into(), 12),
                    ("closed_score".into(), 5),
                    ("open_score".into(), 9),
                ],
                output: 14,
            },
            Operator::Return { value: 14 },
        ],
    };
    let receipt = execute_program(
        &code,
        &mut host,
        &BTreeMap::from([(
            "space_embedding_request".into(),
            encode_request(&model, &revision, &subject),
        )]),
        Revision(39),
        Tick(39),
        ExecutionLimits {
            fuel: 32,
            max_proposals: 0,
        },
    )
    .expect("live graph execution");
    println!("{}", serde_json::to_string_pretty(&receipt).unwrap());
}
