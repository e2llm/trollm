//! In-memory tool argument validation. Size, nesting and regex budgets are
//! bounded; the library does not provide a hard overall CPU deadline.
//! Undeclared schemas default to 2020-12; explicit standard drafts use the
//! library's dialect detection. Custom meta-schemas cannot be fetched offline.

use anyhow::{anyhow, bail, Result};
use jsonschema::{Draft, PatternOptions, Validator};
use serde_json::Value;
use std::io::{self, Write};

const SCHEMA_BYTES: usize = 64 * 1024;
const SCHEMA_DEPTH: usize = 32;
pub(crate) const ARG_BYTES: usize = 256 * 1024;
pub(crate) const ARG_DEPTH: usize = 64;
const PATH_CHARS: usize = 128;
const REGEX_BACKTRACKS: usize = 10_000;
const REGEX_BYTES: usize = 1024 * 1024;

pub(crate) struct ToolSchema {
    validator: Validator,
}

/// Offline, bounded JSON Schema validation for full artifact output values.
/// Callers supply their artifact-byte budget rather than a tool-argument limit.
pub struct ArtifactSchema(ToolSchema);

impl ArtifactSchema {
    pub fn compile(schema: &Value) -> Result<Self> {
        if !schema.is_object() {
            bail!("artifact output schema must be a JSON object");
        }
        Ok(Self(ToolSchema::compile_labelled(
            schema,
            "artifact output schema",
        )?))
    }

    pub fn validate(&self, value: &Value, max_bytes: usize) -> Result<()> {
        check_limits(value, max_bytes, ARG_DEPTH, "artifact output")?;
        self.0.validate_labelled(value, "artifact output")
    }
}

impl ToolSchema {
    pub(crate) fn compile(schema: &Value) -> Result<Self> {
        Self::compile_labelled(schema, "tool schema")
    }

    pub(crate) fn compile_result(schema: &Value) -> Result<Self> {
        Self::compile_labelled(schema, "tool result schema")
    }

    fn compile_labelled(schema: &Value, label: &str) -> Result<Self> {
        check_limits(schema, SCHEMA_BYTES, SCHEMA_DEPTH, label)?;
        if let Some(declared) = schema.get("$schema") {
            if declared
                .as_str()
                .is_none_or(|uri| Draft::from_schema_uri(uri) == Draft::Unknown)
            {
                bail!("unsupported or malformed {label} dialect");
            }
        }
        let validator = jsonschema::options()
            .offline()
            .with_pattern_options(
                PatternOptions::fancy_regex()
                    .backtrack_limit(REGEX_BACKTRACKS)
                    .size_limit(REGEX_BYTES)
                    .dfa_size_limit(REGEX_BYTES),
            )
            .build(schema)
            .map_err(|error| {
                anyhow!(
                    "invalid or unresolved {label} at {}",
                    bounded_path(error.schema_path().to_string())
                )
            })?;
        Ok(Self { validator })
    }

    pub(crate) fn validate(&self, args: &Value) -> Result<()> {
        self.validate_with_byte_limit(args, ARG_BYTES)
    }

    pub(crate) fn validate_with_byte_limit(&self, args: &Value, max_bytes: usize) -> Result<()> {
        check_limits(args, max_bytes, ARG_DEPTH, "tool arguments")?;
        self.validate_labelled(args, "tool argument")
    }

    pub(crate) fn validate_result(&self, value: &Value, max_bytes: usize) -> Result<()> {
        // The bounded byte/depth walk precedes schema evaluation. These are
        // serialized JSON bytes, not a model-token or inference budget.
        check_limits(value, max_bytes, ARG_DEPTH, "tool result")?;
        self.validate_labelled(value, "tool result")
    }

    fn validate_labelled(&self, value: &Value, label: &str) -> Result<()> {
        self.validator.validate(value).map_err(|error| {
            // Never format the library error or its kind: both may contain
            // argument values, schema operands or external reference URLs.
            anyhow!(
                "{label} validation failed at {} (schema {})",
                bounded_path(error.instance_path().to_string()),
                bounded_path(error.schema_path().to_string())
            )
        })
    }
}

fn bounded_path(path: String) -> String {
    if path.is_empty() {
        return "<root>".into();
    }
    let mut chars = path.chars();
    let mut result: String = chars
        .by_ref()
        .take(PATH_CHARS)
        .map(|ch| if ch.is_control() { '?' } else { ch })
        .collect();
    if chars.next().is_some() {
        result.push_str("...");
    }
    result
}

pub(crate) fn check_limits(
    value: &Value,
    max_bytes: usize,
    max_depth: usize,
    label: &str,
) -> Result<()> {
    // Reject excessive nesting before serialization; recursion is bounded by
    // max_depth even when the caller constructed the Value programmatically.
    fn walk(value: &Value, depth: usize, max: usize) -> bool {
        if depth > max {
            return false;
        }
        match value {
            Value::Array(items) => items.iter().all(|item| walk(item, depth + 1, max)),
            Value::Object(items) => items.values().all(|item| walk(item, depth + 1, max)),
            _ => true,
        }
    }
    if !walk(value, 0, max_depth) {
        bail!("{label} exceeds nesting limit");
    }
    // Count serialized bytes without allocating another potentially huge JSON
    // buffer. Stop as soon as the byte limit is exceeded.
    struct Counter {
        bytes: usize,
        max: usize,
    }
    impl Write for Counter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.len() > self.max.saturating_sub(self.bytes) {
                return Err(io::Error::other("serialized size limit"));
            }
            self.bytes += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    if serde_json::to_writer(
        &mut Counter {
            bytes: 0,
            max: max_bytes,
        },
        value,
    )
    .is_err()
    {
        bail!("{label} exceeds serialized size limit");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

    #[test]
    fn nested_constraints_and_combinations() {
        let schema = ToolSchema::compile(&json!({
            "type": "object", "required": ["item"],
            "properties": {"item": {
                "type": "object", "required": ["kind", "count"],
                "properties": {
                    "kind": {"enum": ["note", "task"]},
                    "count": {"allOf": [{"type": "integer"}, {"minimum": 1}]},
                    "choice": {"oneOf": [{"type": "boolean"}, {"const": "auto"}]}
                }, "additionalProperties": false
            }}, "additionalProperties": false
        }))
        .unwrap();
        assert!(schema
            .validate(&json!({"item": {"kind": "note", "count": 1, "choice": "auto"}}))
            .is_ok());
        for invalid in [
            json!({}),
            json!({"item": {"kind": "note"}}),
            json!({"item": {"kind": "other", "count": 1}}),
            json!({"item": {"kind": "task", "count": "1"}}),
            json!({"item": {"kind": "task", "count": 0}}),
            json!({"item": {"kind": "task", "count": 1, "choice": 3}}),
            json!({"item": {"kind": "task", "count": 1, "extra": true}}),
        ] {
            assert!(schema.validate(&invalid).is_err());
        }
    }

    #[test]
    fn local_references_and_format_annotations() {
        let schema = ToolSchema::compile(&json!({
            "$schema": DIALECT,
            "$defs": {"a/b~c": {"type": "string", "format": "email"}},
            "$ref": "#/$defs/a~1b~0c"
        }))
        .unwrap();
        assert!(schema.validate(&json!("not an email")).is_ok());
        assert!(schema.validate(&json!(12)).is_err());
        assert!(ToolSchema::compile(&json!(true))
            .unwrap()
            .validate(&Value::Null)
            .is_ok());
        assert!(ToolSchema::compile(&json!(false))
            .unwrap()
            .validate(&Value::Null)
            .is_err());
    }

    #[test]
    fn malformed_dialects_and_external_references_fail_closed() {
        for schema in [
            Value::Null,
            json!({"type": "not-a-type"}),
            json!({"required": "name"}),
            json!({"$schema": "https://example.invalid/custom-draft"}),
            json!({"$schema": 12}),
            json!({"$ref": "https://example.invalid/private-schema"}),
            json!({"$ref": "file:///private-schema.json"}),
            json!({"$ref": "#/$defs/missing"}),
        ] {
            assert!(ToolSchema::compile(&schema).is_err());
        }
    }

    fn nested(depth: usize) -> Value {
        (0..depth).fold(Value::Null, |inner, _| json!([inner]))
    }

    #[test]
    fn default_and_explicit_standard_dialects() {
        let schema = ToolSchema::compile(&json!({"type": "string"})).unwrap();
        assert_eq!(schema.validator.draft(), Draft::Draft202012);
        let schema = ToolSchema::compile(&json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "integer"
        }))
        .unwrap();
        assert_eq!(schema.validator.draft(), Draft::Draft7);
        assert!(schema.validate(&json!(1)).is_ok());
        assert!(schema.validate(&json!("1")).is_err());
    }

    #[test]
    fn size_and_depth_limits() {
        assert!(ToolSchema::compile(&json!({"description": "x".repeat(SCHEMA_BYTES)})).is_err());
        assert!(ToolSchema::compile(&json!({"const": nested(SCHEMA_DEPTH)})).is_err());
        let schema = ToolSchema::compile(&json!(true)).unwrap();
        assert!(schema.validate(&nested(ARG_DEPTH)).is_ok());
        assert!(schema.validate(&nested(ARG_DEPTH + 1)).is_err());
        assert!(schema.validate(&json!("x".repeat(ARG_BYTES))).is_err());
        assert!(schema.validate(&json!("x".repeat(ARG_BYTES - 2))).is_ok());
    }

    #[test]
    fn regex_budget_and_value_redaction() {
        let schema =
            ToolSchema::compile(&json!({"type": "string", "pattern": "^(a+)+(?=b)$"})).unwrap();
        let secret = format!("{}SECRET", "a".repeat(64));
        let error = schema.validate(&json!(secret)).unwrap_err().to_string();
        assert!(!error.contains("SECRET"));
        assert!(!error.contains(&"a".repeat(64)));
        // A deliberately tiny test-only engine budget exercises its failure
        // handling independently of whether the production budget is reached.
        let validator = jsonschema::options()
            .with_draft(Draft::Draft202012)
            .offline()
            .with_pattern_options(PatternOptions::fancy_regex().backtrack_limit(1))
            .build(&json!({"type": "string", "pattern": "(?<=ab)c"}))
            .unwrap();
        let input = json!("abc");
        let failure = validator.validate(&input).unwrap_err();
        assert!(matches!(
            failure.kind(),
            jsonschema::error::ValidationErrorKind::BacktrackLimitExceeded { .. }
        ));
        let schema = ToolSchema { validator };
        assert!(schema.validate(&input).is_err());
        let schema = ToolSchema::compile(&json!({"enum": ["allowed"]})).unwrap();
        assert!(!schema
            .validate(&json!("BEARER_SECRET"))
            .unwrap_err()
            .to_string()
            .contains("BEARER_SECRET"));
    }

    #[test]
    fn diagnostic_paths_are_bounded_and_single_line() {
        let path = format!("/{}\n", "é".repeat(200));
        let result = bounded_path(path);
        assert!(result.chars().count() <= PATH_CHARS + 3);
        assert!(!result.contains('\n'));
        assert_eq!(bounded_path(String::new()), "<root>");
    }
}
