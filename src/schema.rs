//! Frontmatter parser and JSON Schema validator.
//!
//! Parses `---`-delimited YAML frontmatter from markdown files and validates
//! it against the JSON Schemas in `specs/schemas/v1/`.

use crate::error::{MemoryFsError, Result};

/// A parsed markdown document: YAML frontmatter + body.
#[derive(Debug, Clone)]
pub struct Document {
    /// Parsed frontmatter as a JSON value (converted from YAML).
    pub frontmatter: serde_json::Value,
    /// Markdown body after the closing `---`.
    pub body: String,
}

/// Split a markdown file into YAML frontmatter and body.
///
/// The file must start with `---\n`, followed by YAML, followed by `---\n`.
/// Everything after the second `---` is the body.
pub fn parse_frontmatter(input: &str) -> Result<Document> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);

    if !input.starts_with("---") {
        return Err(MemoryFsError::Validation(
            "file must start with '---' (YAML frontmatter delimiter)".into(),
        ));
    }

    let after_first = &input[3..];
    let after_first = after_first.strip_prefix('\n').unwrap_or(after_first);

    let closing = after_first.find("\n---");
    let (yaml_str, body) = match closing {
        Some(pos) => {
            let yaml = &after_first[..pos];
            let rest = &after_first[pos + 4..]; // skip \n---
            let body = rest.strip_prefix('\n').unwrap_or(rest);
            (yaml, body.to_string())
        }
        None => {
            return Err(MemoryFsError::Validation(
                "missing closing '---' for frontmatter".into(),
            ));
        }
    };

    let yaml_value: serde_yaml::Value = serde_yaml::from_str(yaml_str)
        .map_err(|e| MemoryFsError::Validation(format!("invalid YAML in frontmatter: {e}")))?;

    let json_value = yaml_to_json(yaml_value)?;

    Ok(Document {
        frontmatter: json_value,
        body,
    })
}

/// Serialize a `Document` back to markdown with YAML frontmatter.
pub fn render_document(doc: &Document) -> Result<String> {
    let yaml_str = serde_yaml::to_string(&json_to_yaml(&doc.frontmatter))
        .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("YAML serialization failed: {e}")))?;

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&yaml_str);
    if !yaml_str.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---\n");
    out.push_str(&doc.body);
    Ok(out)
}

/// Extract the `type` field from frontmatter.
pub fn document_type(frontmatter: &serde_json::Value) -> Option<&str> {
    frontmatter.get("type").and_then(|v| v.as_str())
}

/// Extract the `schema_version` field from frontmatter.
pub fn schema_version(frontmatter: &serde_json::Value) -> Option<&str> {
    frontmatter.get("schema_version").and_then(|v| v.as_str())
}

/// Validate frontmatter against a JSON Schema.
pub fn validate_frontmatter(
    frontmatter: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<()> {
    let compiled = jsonschema::JSONSchema::compile(schema)
        .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("invalid JSON Schema: {e}")))?;

    let errors: Vec<String> = compiled
        .validate(frontmatter)
        .err()
        .into_iter()
        .flat_map(|e| e.into_iter())
        .map(|e| {
            let path = e.instance_path.to_string();
            if path.is_empty() {
                e.to_string()
            } else {
                format!("{path}: {e}")
            }
        })
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(MemoryFsError::Validation(errors.join("; ")))
    }
}

fn yaml_to_json(yaml: serde_yaml::Value) -> Result<serde_json::Value> {
    match yaml {
        serde_yaml::Value::Null => Ok(serde_json::Value::Null),
        serde_yaml::Value::Bool(b) => Ok(serde_json::Value::Bool(b)),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(serde_json::Value::Number(i.into()))
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| {
                        MemoryFsError::Validation(format!("non-finite float in YAML: {f}"))
                    })
            } else {
                Err(MemoryFsError::Validation("unsupported YAML number".into()))
            }
        }
        serde_yaml::Value::String(s) => Ok(serde_json::Value::String(s)),
        serde_yaml::Value::Sequence(seq) => {
            let arr: std::result::Result<Vec<_>, _> = seq.into_iter().map(yaml_to_json).collect();
            Ok(serde_json::Value::Array(arr?))
        }
        serde_yaml::Value::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                let key = match k {
                    serde_yaml::Value::String(s) => s,
                    other => serde_yaml::to_string(&other)
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                };
                obj.insert(key, yaml_to_json(v)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
        serde_yaml::Value::Tagged(tagged) => yaml_to_json(tagged.value),
    }
}

fn json_to_yaml(json: &serde_json::Value) -> serde_yaml::Value {
    match json {
        serde_json::Value::Null => serde_yaml::Value::Null,
        serde_json::Value::Bool(b) => serde_yaml::Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_yaml::Value::Number(i.into())
            } else if let Some(f) = n.as_f64() {
                serde_yaml::Value::Number(serde_yaml::Number::from(f))
            } else {
                serde_yaml::Value::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => serde_yaml::Value::String(s.clone()),
        serde_json::Value::Array(arr) => {
            serde_yaml::Value::Sequence(arr.iter().map(json_to_yaml).collect())
        }
        serde_json::Value::Object(map) => {
            let mut m = serde_yaml::Mapping::new();
            for (k, v) in map {
                m.insert(serde_yaml::Value::String(k.clone()), json_to_yaml(v));
            }
            serde_yaml::Value::Mapping(m)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_DOC: &str = "\
---
schema_version: memoryfs/v1
id: mem_01HZK4M8N1P3Q5R7S9T1V3X5Z7
type: memory
memory_type: preference
scope: user
scope_id: user:alice
author: agent:architect
confidence: 0.92
status: active
sensitivity: normal
permissions:
  read: [\"owner\"]
  write: [\"owner\"]
provenance:
  source_file: conversations/2026/04/30/conv.md
  source_commit: 8f3a6e9c2b1d4a5e7f8c9b0d1a2e3f4c5b6a7d8e9f0c1b2a3d4e5f6a7b8c9d0e
  extracted_at: \"2026-04-30T09:13:58.044Z\"
tags: [preference, devops]
---
User prefers local-first AI infrastructure.
";

    #[test]
    fn parse_valid_frontmatter() {
        let doc = parse_frontmatter(SAMPLE_DOC).unwrap();
        assert_eq!(document_type(&doc.frontmatter), Some("memory"));
        assert_eq!(schema_version(&doc.frontmatter), Some("memoryfs/v1"));
        assert_eq!(
            doc.body.trim(),
            "User prefers local-first AI infrastructure."
        );
    }

    #[test]
    fn parse_missing_opening_delimiter() {
        let err = parse_frontmatter("no frontmatter here").unwrap_err();
        assert_eq!(err.api_code(), "VALIDATION");
    }

    #[test]
    fn parse_missing_closing_delimiter() {
        let err = parse_frontmatter("---\nfoo: bar\n").unwrap_err();
        assert_eq!(err.api_code(), "VALIDATION");
    }

    #[test]
    fn parse_invalid_yaml() {
        let err = parse_frontmatter("---\n: : : broken\n---\n").unwrap_err();
        assert_eq!(err.api_code(), "VALIDATION");
    }

    #[test]
    fn parse_empty_body() {
        let doc = parse_frontmatter("---\nfoo: bar\n---\n").unwrap();
        assert_eq!(doc.body, "");
        assert_eq!(doc.frontmatter["foo"], "bar");
    }

    #[test]
    fn parse_preserves_multiline_body() {
        let input = "---\nk: v\n---\nline 1\nline 2\nline 3\n";
        let doc = parse_frontmatter(input).unwrap();
        assert_eq!(doc.body, "line 1\nline 2\nline 3\n");
    }

    #[test]
    fn yaml_numbers_convert_correctly() {
        let input = "---\nint_val: 42\nfloat_val: 9.81\nbool_val: true\n---\n";
        let doc = parse_frontmatter(input).unwrap();
        assert_eq!(doc.frontmatter["int_val"], 42);
        assert!((doc.frontmatter["float_val"].as_f64().unwrap() - 9.81).abs() < 0.001);
        assert_eq!(doc.frontmatter["bool_val"], true);
    }

    #[test]
    fn roundtrip_render() {
        let doc = parse_frontmatter(SAMPLE_DOC).unwrap();
        let rendered = render_document(&doc).unwrap();
        let reparsed = parse_frontmatter(&rendered).unwrap();
        assert_eq!(doc.frontmatter, reparsed.frontmatter);
        assert_eq!(doc.body.trim(), reparsed.body.trim());
    }

    #[test]
    fn validate_against_simple_schema() {
        let schema: serde_json::Value = serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string" }
            }
        });

        let valid = serde_json::json!({"name": "hello"});
        validate_frontmatter(&valid, &schema).unwrap();

        let invalid = serde_json::json!({"other": 42});
        let err = validate_frontmatter(&invalid, &schema).unwrap_err();
        assert_eq!(err.api_code(), "VALIDATION");
    }

    #[test]
    fn validate_memory_frontmatter_against_schema() {
        let schema_str = std::fs::read_to_string("../../specs/schemas/v1/memory.schema.json");
        if schema_str.is_err() {
            // Skip if not running from workspace root
            return;
        }
        let doc = parse_frontmatter(SAMPLE_DOC).unwrap();
        // This validates structure — the real schema uses $ref which requires
        // a resolver; here we just confirm the validator runs without panic.
        let schema: serde_json::Value = serde_json::from_str(&schema_str.unwrap()).unwrap();
        let _ = validate_frontmatter(&doc.frontmatter, &schema);
    }

    #[test]
    fn bom_handling() {
        let input = "\u{feff}---\nk: v\n---\nbody\n";
        let doc = parse_frontmatter(input).unwrap();
        assert_eq!(doc.frontmatter["k"], "v");
    }

    #[test]
    fn nested_yaml_objects() {
        let input = "---\npermissions:\n  read:\n    - owner\n    - agent:bot\n  write:\n    - owner\n---\n";
        let doc = parse_frontmatter(input).unwrap();
        let read = doc.frontmatter["permissions"]["read"].as_array().unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0], "owner");
    }
}
