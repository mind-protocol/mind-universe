//! Generic, policy-free embedding primitives for graph-selected models and axes.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use thiserror::Error;
use universe_ir::Value;

pub const VECTOR_SCALE: i64 = 1_000_000;
pub const SCORE_SCALE: i64 = 1_000_000;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QuantizedEmbedding {
    pub model: String,
    pub model_revision: String,
    pub input_sha256: String,
    pub dimensions: u32,
    pub scale: i64,
    pub values: Vec<i32>,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum EmbeddingError {
    #[error("embedding input is empty")]
    EmptyInput,
    #[error("embedding dimension mismatch: expected {expected}, observed {observed}")]
    DimensionMismatch { expected: usize, observed: usize },
    #[error("embedding contains a non-finite value")]
    NonFinite,
    #[error("embedding vector is zero")]
    ZeroVector,
    #[error("invalid capability input: {0}")]
    InvalidInput(String),
    #[error("embedding provider failed: {0}")]
    Provider(String),
}

pub trait EmbeddingProvider {
    fn model(&self) -> &str;
    fn model_revision(&self) -> &str;
    fn encode(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError>;
}

#[derive(Clone, Debug)]
pub struct NodeTransformersProvider {
    pub node_executable: PathBuf,
    pub module_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub model: String,
    pub model_revision: String,
    pub prefix: String,
    pub allow_remote: bool,
}

impl EmbeddingProvider for NodeTransformersProvider {
    fn model(&self) -> &str {
        &self.model
    }

    fn model_revision(&self) -> &str {
        &self.model_revision
    }

    fn encode(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        #[derive(Serialize)]
        struct Request<'a> {
            module_dir: &'a str,
            cache_dir: &'a str,
            model: &'a str,
            prefix: &'a str,
            allow_remote: bool,
            texts: &'a [String],
        }
        #[derive(Deserialize)]
        struct Response {
            vectors: Vec<Vec<f32>>,
        }

        let module_dir = self
            .module_dir
            .to_str()
            .ok_or_else(|| EmbeddingError::Provider("module path is not UTF-8".into()))?;
        let cache_dir = self
            .cache_dir
            .to_str()
            .ok_or_else(|| EmbeddingError::Provider("cache path is not UTF-8".into()))?;
        let mut child = Command::new(&self.node_executable)
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/bridge.mjs"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| EmbeddingError::Provider(error.to_string()))?;
        serde_json::to_writer(
            child.stdin.as_mut().expect("piped stdin exists"),
            &Request {
                module_dir,
                cache_dir,
                model: &self.model,
                prefix: &self.prefix,
                allow_remote: self.allow_remote,
                texts,
            },
        )
        .map_err(|error| EmbeddingError::Provider(error.to_string()))?;
        child
            .stdin
            .take()
            .expect("piped stdin exists")
            .flush()
            .map_err(|error| EmbeddingError::Provider(error.to_string()))?;
        let output = child
            .wait_with_output()
            .map_err(|error| EmbeddingError::Provider(error.to_string()))?;
        if !output.status.success() {
            return Err(EmbeddingError::Provider(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        let response: Response = serde_json::from_slice(&output.stdout)
            .map_err(|error| EmbeddingError::Provider(error.to_string()))?;
        Ok(response.vectors)
    }
}

pub struct EmbeddingRuntime<P> {
    provider: P,
    expected_dimensions: usize,
    cache: BTreeMap<String, QuantizedEmbedding>,
}

impl<P: EmbeddingProvider> EmbeddingRuntime<P> {
    pub fn new(provider: P, expected_dimensions: usize) -> Self {
        Self {
            provider,
            expected_dimensions,
            cache: BTreeMap::new(),
        }
    }

    pub fn encode(&mut self, texts: &[String]) -> Result<Vec<QuantizedEmbedding>, EmbeddingError> {
        if texts.is_empty() || texts.iter().any(|text| text.trim().is_empty()) {
            return Err(EmbeddingError::EmptyInput);
        }
        let keys: Vec<_> = texts
            .iter()
            .map(|text| cache_key(self.provider.model(), self.provider.model_revision(), text))
            .collect();
        let missing: Vec<_> = texts
            .iter()
            .zip(&keys)
            .filter(|(_, key)| !self.cache.contains_key(*key))
            .map(|(text, _)| text.clone())
            .collect();
        if !missing.is_empty() {
            let vectors = self.provider.encode(&missing)?;
            if vectors.len() != missing.len() {
                return Err(EmbeddingError::DimensionMismatch {
                    expected: missing.len(),
                    observed: vectors.len(),
                });
            }
            for (text, vector) in missing.iter().zip(vectors) {
                let embedding = quantize(
                    self.provider.model(),
                    self.provider.model_revision(),
                    text,
                    &vector,
                    self.expected_dimensions,
                )?;
                self.cache.insert(
                    cache_key(self.provider.model(), self.provider.model_revision(), text),
                    embedding,
                );
            }
        }
        Ok(keys
            .iter()
            .map(|key| self.cache.get(key).expect("cache filled").clone())
            .collect())
    }

    pub fn call(&mut self, capability: &str, input: &Value) -> Result<Value, String> {
        match capability {
            "embedding.encode" => self.call_encode(input),
            "vector.cosine" => call_cosine(input),
            "vector.centroid" => call_centroid(input),
            "vector.successive_projections" => call_successive_projections(input),
            other => Err(format!("unsupported embedding capability {other}")),
        }
    }

    fn call_encode(&mut self, input: &Value) -> Result<Value, String> {
        let record = record(input)?;
        let requested_model = text_field(record, "model")?;
        let requested_revision = text_field(record, "model_revision")?;
        if requested_model != self.provider.model()
            || requested_revision != self.provider.model_revision()
        {
            return Err("requested model identity does not match the loaded adapter".into());
        }
        let single = record.get("text").is_some();
        let texts = if single {
            vec![text_field(record, "text")?.to_owned()]
        } else {
            text_list_field(record, "texts")?
        };
        let embeddings = self.encode(&texts).map_err(|error| error.to_string())?;
        if single {
            Ok(embedding_value(&embeddings[0]))
        } else {
            Ok(Value::List(
                embeddings.iter().map(embedding_value).collect(),
            ))
        }
    }
}

pub fn quantize(
    model: &str,
    model_revision: &str,
    text: &str,
    values: &[f32],
    expected_dimensions: usize,
) -> Result<QuantizedEmbedding, EmbeddingError> {
    if values.len() != expected_dimensions {
        return Err(EmbeddingError::DimensionMismatch {
            expected: expected_dimensions,
            observed: values.len(),
        });
    }
    if values.iter().any(|value| !value.is_finite()) {
        return Err(EmbeddingError::NonFinite);
    }
    if values
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        <= 1e-24
    {
        return Err(EmbeddingError::ZeroVector);
    }
    Ok(QuantizedEmbedding {
        model: model.into(),
        model_revision: model_revision.into(),
        input_sha256: sha256(text.as_bytes()),
        dimensions: values.len() as u32,
        scale: VECTOR_SCALE,
        values: values
            .iter()
            .map(|value| (f64::from(*value) * VECTOR_SCALE as f64).round() as i32)
            .collect(),
    })
}

pub fn cosine(
    left: &QuantizedEmbedding,
    right: &QuantizedEmbedding,
) -> Result<i64, EmbeddingError> {
    validate_pair(left, right)?;
    let dot = left
        .values
        .iter()
        .zip(&right.values)
        .map(|(left, right)| i128::from(*left) * i128::from(*right))
        .sum::<i128>() as f64;
    let left_norm = left
        .values
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    let right_norm = right
        .values
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        return Err(EmbeddingError::ZeroVector);
    }
    Ok((dot / (left_norm * right_norm) * SCORE_SCALE as f64).round() as i64)
}

pub fn centroid(vectors: &[QuantizedEmbedding]) -> Result<QuantizedEmbedding, EmbeddingError> {
    let first = vectors.first().ok_or(EmbeddingError::EmptyInput)?;
    for vector in &vectors[1..] {
        validate_pair(first, vector)?;
    }
    let mut sums = vec![0i64; first.values.len()];
    for vector in vectors {
        for (sum, value) in sums.iter_mut().zip(&vector.values) {
            *sum += i64::from(*value);
        }
    }
    let values: Vec<i32> = sums
        .into_iter()
        .map(|sum| (sum as f64 / vectors.len() as f64).round() as i32)
        .collect();
    if values.iter().all(|value| *value == 0) {
        return Err(EmbeddingError::ZeroVector);
    }
    let input_hashes = vectors
        .iter()
        .map(|vector| vector.input_sha256.as_str())
        .collect::<Vec<_>>()
        .join(":");
    Ok(QuantizedEmbedding {
        model: first.model.clone(),
        model_revision: first.model_revision.clone(),
        input_sha256: sha256(input_hashes.as_bytes()),
        dimensions: first.dimensions,
        scale: first.scale,
        values,
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    pub axis: String,
    pub coefficient: i64,
    pub residual_norm: i64,
}

pub fn successive_projections(
    vector: &QuantizedEmbedding,
    axes: &BTreeMap<String, QuantizedEmbedding>,
    max_projections: usize,
    min_abs_coefficient: i64,
) -> Result<Vec<Projection>, EmbeddingError> {
    if max_projections == 0 {
        return Err(EmbeddingError::InvalidInput(
            "max_projections is zero".into(),
        ));
    }
    let mut residual = vector.clone();
    let mut available = axes.clone();
    let mut result = Vec::new();
    while result.len() < max_projections && !available.is_empty() {
        let mut candidates = available
            .iter()
            .map(|(id, axis)| Ok((id.clone(), projection_coefficient(&residual, axis)?)))
            .collect::<Result<Vec<_>, EmbeddingError>>()?;
        candidates.sort_by(|left, right| {
            right
                .1
                .abs()
                .cmp(&left.1.abs())
                .then_with(|| left.0.cmp(&right.0))
        });
        let (axis_id, coefficient) = candidates.remove(0);
        if coefficient.abs() < min_abs_coefficient {
            break;
        }
        let axis = available.remove(&axis_id).expect("selected axis exists");
        for (value, axis_value) in residual.values.iter_mut().zip(axis.values) {
            let projected =
                (i128::from(axis_value) * i128::from(coefficient) / i128::from(SCORE_SCALE)) as i32;
            *value = value.saturating_sub(projected);
        }
        let residual_norm = residual
            .values
            .iter()
            .map(|value| f64::from(*value).powi(2))
            .sum::<f64>()
            .sqrt()
            .round() as i64;
        result.push(Projection {
            axis: axis_id,
            coefficient,
            residual_norm,
        });
        if residual_norm == 0 {
            break;
        }
    }
    Ok(result)
}

fn projection_coefficient(
    vector: &QuantizedEmbedding,
    axis: &QuantizedEmbedding,
) -> Result<i64, EmbeddingError> {
    validate_pair(vector, axis)?;
    let dot = vector
        .values
        .iter()
        .zip(&axis.values)
        .map(|(left, right)| i128::from(*left) * i128::from(*right))
        .sum::<i128>();
    let axis_squared = axis
        .values
        .iter()
        .map(|value| i128::from(*value) * i128::from(*value))
        .sum::<i128>();
    if axis_squared == 0 {
        return Err(EmbeddingError::ZeroVector);
    }
    Ok((dot * i128::from(SCORE_SCALE) / axis_squared) as i64)
}

fn validate_pair(
    left: &QuantizedEmbedding,
    right: &QuantizedEmbedding,
) -> Result<(), EmbeddingError> {
    if left.model != right.model
        || left.model_revision != right.model_revision
        || left.scale != right.scale
        || left.dimensions != right.dimensions
        || left.values.len() != right.values.len()
    {
        return Err(EmbeddingError::DimensionMismatch {
            expected: left.values.len(),
            observed: right.values.len(),
        });
    }
    Ok(())
}

fn call_cosine(input: &Value) -> Result<Value, String> {
    let record = record(input)?;
    let left = embedding_from_value(field(record, "left")?)?;
    let right = embedding_from_value(field(record, "right")?)?;
    cosine(&left, &right)
        .map(Value::Integer)
        .map_err(|error| error.to_string())
}

fn call_centroid(input: &Value) -> Result<Value, String> {
    let record = record(input)?;
    let Value::List(items) = field(record, "vectors")? else {
        return Err("vectors must be a list".into());
    };
    let vectors = items
        .iter()
        .map(embedding_from_value)
        .collect::<Result<Vec<_>, _>>()?;
    centroid(&vectors)
        .map(|value| embedding_value(&value))
        .map_err(|error| error.to_string())
}

fn call_successive_projections(input: &Value) -> Result<Value, String> {
    let request = record(input)?;
    let vector = embedding_from_value(field(request, "vector")?)?;
    let axes_record = record(field(request, "axes")?)?;
    let axes = axes_record
        .iter()
        .map(|(id, value)| Ok((id.clone(), embedding_from_value(value)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let max = integer_field(request, "max_projections")?;
    let min = integer_field(request, "min_abs_coefficient")?;
    let projections =
        successive_projections(&vector, &axes, max as usize, min).map_err(|e| e.to_string())?;
    Ok(Value::List(
        projections
            .into_iter()
            .map(|projection| {
                Value::Record(BTreeMap::from([
                    ("axis".into(), Value::Text(projection.axis)),
                    ("coefficient".into(), Value::Integer(projection.coefficient)),
                    (
                        "residual_norm".into(),
                        Value::Integer(projection.residual_norm),
                    ),
                ]))
            })
            .collect(),
    ))
}

fn embedding_value(embedding: &QuantizedEmbedding) -> Value {
    Value::Record(BTreeMap::from([
        ("model".into(), Value::Text(embedding.model.clone())),
        (
            "model_revision".into(),
            Value::Text(embedding.model_revision.clone()),
        ),
        (
            "input_sha256".into(),
            Value::Text(embedding.input_sha256.clone()),
        ),
        (
            "dimensions".into(),
            Value::Integer(embedding.dimensions.into()),
        ),
        ("scale".into(), Value::Integer(embedding.scale)),
        (
            "values".into(),
            Value::List(
                embedding
                    .values
                    .iter()
                    .map(|value| Value::Integer(i64::from(*value)))
                    .collect(),
            ),
        ),
    ]))
}

fn embedding_from_value(value: &Value) -> Result<QuantizedEmbedding, String> {
    let record = record(value)?;
    let values = match field(record, "values")? {
        Value::List(values) => values
            .iter()
            .map(|value| match value {
                Value::Integer(value) => i32::try_from(*value)
                    .map_err(|_| "embedding component does not fit i32".to_owned()),
                _ => Err("embedding values must be integers".into()),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("embedding values must be a list".into()),
    };
    Ok(QuantizedEmbedding {
        model: text_field(record, "model")?.into(),
        model_revision: text_field(record, "model_revision")?.into(),
        input_sha256: text_field(record, "input_sha256")?.into(),
        dimensions: u32::try_from(integer_field(record, "dimensions")?)
            .map_err(|_| "invalid dimensions".to_owned())?,
        scale: integer_field(record, "scale")?,
        values,
    })
}

fn record(value: &Value) -> Result<&BTreeMap<String, Value>, String> {
    match value {
        Value::Record(record) => Ok(record),
        _ => Err("expected record".into()),
    }
}

fn field<'a>(record: &'a BTreeMap<String, Value>, name: &str) -> Result<&'a Value, String> {
    record
        .get(name)
        .ok_or_else(|| format!("missing field {name}"))
}

fn text_field<'a>(record: &'a BTreeMap<String, Value>, name: &str) -> Result<&'a str, String> {
    match field(record, name)? {
        Value::Text(value) => Ok(value),
        _ => Err(format!("{name} must be text")),
    }
}

fn integer_field(record: &BTreeMap<String, Value>, name: &str) -> Result<i64, String> {
    match field(record, name)? {
        Value::Integer(value) => Ok(*value),
        _ => Err(format!("{name} must be integer")),
    }
}

fn text_list_field(record: &BTreeMap<String, Value>, name: &str) -> Result<Vec<String>, String> {
    match field(record, name)? {
        Value::List(values) => values
            .iter()
            .map(|value| match value {
                Value::Text(value) => Ok(value.clone()),
                _ => Err(format!("{name} entries must be text")),
            })
            .collect(),
        _ => Err(format!("{name} must be a list")),
    }
}

fn cache_key(model: &str, revision: &str, text: &str) -> String {
    sha256(format!("{model}\0{revision}\0{text}").as_bytes())
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use universe_compiler::{compile, CompileError};
    use universe_core::Revision;
    use universe_ir::{CodeDefinition, Operator, IR_VERSION};

    struct FixtureProvider {
        calls: usize,
    }

    impl EmbeddingProvider for FixtureProvider {
        fn model(&self) -> &str {
            "fixture"
        }

        fn model_revision(&self) -> &str {
            "sha256:fixture"
        }

        fn encode(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
            self.calls += 1;
            Ok(texts
                .iter()
                .map(|text| match text.as_str() {
                    "closed" => vec![1.0, 0.0, 0.0],
                    "open" => vec![-1.0, 0.0, 0.0],
                    "sample" => vec![0.8, 0.2, 0.1],
                    _ => vec![0.0, 1.0, 0.0],
                })
                .collect())
        }
    }

    #[test]
    fn cache_identity_and_cosine_are_deterministic() {
        let mut runtime = EmbeddingRuntime::new(FixtureProvider { calls: 0 }, 3);
        let first = runtime
            .encode(&["closed".into(), "open".into(), "closed".into()])
            .unwrap();
        assert_eq!(first[0], first[2]);
        assert_eq!(cosine(&first[0], &first[1]).unwrap(), -SCORE_SCALE);
        assert_eq!(runtime.provider.calls, 1);
        runtime.encode(&["closed".into()]).unwrap();
        assert_eq!(runtime.provider.calls, 1);
    }

    #[test]
    fn rejects_invalid_vectors_without_coercion() {
        assert_eq!(
            quantize("m", "r", "x", &[f32::NAN, 1.0], 2),
            Err(EmbeddingError::NonFinite)
        );
        assert_eq!(
            quantize("m", "r", "x", &[0.0, 0.0], 2),
            Err(EmbeddingError::ZeroVector)
        );
        assert!(matches!(
            quantize("m", "r", "x", &[1.0], 2),
            Err(EmbeddingError::DimensionMismatch { .. })
        ));
    }

    #[test]
    fn residual_projection_preserves_unexplained_content() {
        let vector = quantize("m", "r", "v", &[0.8, 0.4, 0.2], 3).unwrap();
        let axes = BTreeMap::from([
            (
                "enclosure".into(),
                quantize("m", "r", "a", &[1.0, 0.0, 0.0], 3).unwrap(),
            ),
            (
                "access".into(),
                quantize("m", "r", "b", &[0.0, 1.0, 0.0], 3).unwrap(),
            ),
        ]);
        let projections = successive_projections(&vector, &axes, 2, 1).unwrap();
        assert_eq!(projections[0].axis, "enclosure");
        assert_eq!(projections[1].axis, "access");
        assert!(projections[1].residual_norm > 0);
    }

    #[test]
    fn graph_ir_rejects_an_undeclared_capability_call() {
        let code = CodeDefinition {
            ir_version: IR_VERSION,
            revision: Revision(1),
            required_capabilities: vec![],
            operators: vec![
                Operator::Constant {
                    value: Value::Unit,
                    output: 0,
                },
                Operator::CapabilityCall {
                    capability: "embedding.encode".into(),
                    input: 0,
                    output: 1,
                },
                Operator::Return { value: 1 },
            ],
        };
        assert!(matches!(
            compile(&code),
            Err(CompileError::UndeclaredCapability {
                operator: 1,
                capability
            }) if capability == "embedding.encode"
        ));
    }
}
