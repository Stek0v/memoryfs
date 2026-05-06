//! Extraction contract — validates LLM extraction output against the memory
//! proposal schema and converts it into typed proposals.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::{MemoryFsError, Result};
use crate::ids::{MemoryId, ProposalId};

/// Raw proposal as returned by the LLM extraction prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawProposal {
    /// Memory type classification.
    pub memory_type: String,
    /// Scope level.
    pub scope: String,
    /// Scope identifier (e.g. `user:alice`).
    pub scope_id: String,
    /// Sensitivity classification.
    pub sensitivity: String,
    /// Confidence score [0..1].
    pub confidence: f64,
    /// Short title.
    pub title: String,
    /// Memory body text.
    pub body: String,
    /// Tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Entity references.
    #[serde(default)]
    pub entities: Vec<EntityRef>,
    /// Hint about which older memory this supersedes.
    #[serde(default)]
    pub supersedes_hint: Option<String>,
}

/// Entity reference in a proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityRef {
    /// Entity ID (may be empty if unknown).
    #[serde(default)]
    pub id: String,
    /// Role of the entity in the memory.
    pub role: String,
}

/// Validated proposal ready for the inbox/commit pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedProposal {
    /// Assigned proposal ID.
    pub proposal_id: String,
    /// Generated memory ID.
    pub memory_id: String,
    /// Memory type.
    pub memory_type: MemoryType,
    /// Scope level.
    pub scope: Scope,
    /// Scope identifier.
    pub scope_id: String,
    /// Sensitivity classification.
    pub sensitivity: Sensitivity,
    /// Confidence score [0..1].
    pub confidence: f64,
    /// Short title.
    pub title: String,
    /// Memory body text.
    pub body: String,
    /// Tags.
    pub tags: Vec<String>,
    /// Entity references.
    pub entities: Vec<EntityRef>,
    /// Supersedes hint.
    pub supersedes_hint: Option<String>,
    /// When extracted.
    pub extracted_at: String,
}

/// Memory type categories (from `memory.schema.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// User preference.
    Preference,
    /// Known fact.
    Fact,
    /// User goal.
    Goal,
    /// Learned skill.
    Skill,
    /// Relationship between entities.
    Relationship,
    /// Constraint or rule.
    Constraint,
    /// Episodic event.
    Episodic,
}

impl MemoryType {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "preference" => Ok(Self::Preference),
            "fact" => Ok(Self::Fact),
            "goal" => Ok(Self::Goal),
            "skill" => Ok(Self::Skill),
            "relationship" => Ok(Self::Relationship),
            "constraint" => Ok(Self::Constraint),
            "episodic" => Ok(Self::Episodic),
            _ => Err(MemoryFsError::Validation(format!(
                "invalid memory_type: {s:?}, expected one of: preference, fact, goal, skill, relationship, constraint, episodic"
            ))),
        }
    }
}

/// Scope levels (from `base.schema.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// User-level scope.
    User,
    /// Agent-level scope.
    Agent,
    /// Session-level scope.
    Session,
    /// Project-level scope.
    Project,
    /// Organization-level scope.
    Org,
}

impl Scope {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "user" => Ok(Self::User),
            "agent" => Ok(Self::Agent),
            "session" => Ok(Self::Session),
            "project" => Ok(Self::Project),
            "org" => Ok(Self::Org),
            _ => Err(MemoryFsError::Validation(format!(
                "invalid scope: {s:?}, expected one of: user, agent, session, project, org"
            ))),
        }
    }
}

/// Sensitivity levels (from `base.schema.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    /// Normal, non-sensitive.
    Normal,
    /// Internal use only.
    Internal,
    /// Personally identifiable information.
    Pii,
    /// Secret/credential.
    Secret,
    /// Medical information.
    Medical,
    /// Legal information.
    Legal,
    /// Financial information.
    Financial,
}

impl Sensitivity {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "normal" => Ok(Self::Normal),
            "internal" => Ok(Self::Internal),
            "pii" => Ok(Self::Pii),
            "secret" => Ok(Self::Secret),
            "medical" => Ok(Self::Medical),
            "legal" => Ok(Self::Legal),
            "financial" => Ok(Self::Financial),
            _ => Err(MemoryFsError::Validation(format!(
                "invalid sensitivity: {s:?}, expected one of: normal, internal, pii, secret, medical, legal, financial"
            ))),
        }
    }

    /// Whether this sensitivity level requires review before commit.
    pub fn requires_review(&self) -> bool {
        !matches!(self, Self::Normal | Self::Internal)
    }
}

/// Errors from parsing LLM extraction output.
#[derive(Debug)]
pub struct ExtractionError {
    /// Index of the proposal in the array (if applicable).
    pub index: Option<usize>,
    /// Error message.
    pub message: String,
}

/// Parse and validate LLM extraction output.
///
/// The input should be a JSON array of `RawProposal`. Returns validated
/// proposals and any validation errors. Partial success is allowed — valid
/// proposals are returned even if some fail validation.
pub fn parse_extraction_output(json_str: &str) -> (Vec<ValidatedProposal>, Vec<ExtractionError>) {
    let raw_proposals: Vec<RawProposal> = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(e) => {
            return (
                Vec::new(),
                vec![ExtractionError {
                    index: None,
                    message: format!("failed to parse JSON array: {e}"),
                }],
            );
        }
    };

    let mut validated = Vec::new();
    let mut errors = Vec::new();

    for (i, raw) in raw_proposals.into_iter().enumerate() {
        match validate_proposal(raw) {
            Ok(v) => validated.push(v),
            Err(msg) => errors.push(ExtractionError {
                index: Some(i),
                message: msg,
            }),
        }
    }

    (validated, errors)
}

/// Strictly parse extraction output — all proposals must be valid.
pub fn parse_extraction_output_strict(json_str: &str) -> Result<Vec<ValidatedProposal>> {
    let (validated, errors) = parse_extraction_output(json_str);
    if !errors.is_empty() {
        let messages: Vec<String> = errors
            .iter()
            .map(|e| {
                if let Some(idx) = e.index {
                    format!("[{idx}]: {}", e.message)
                } else {
                    e.message.clone()
                }
            })
            .collect();
        return Err(MemoryFsError::Validation(format!(
            "extraction output validation failed: {}",
            messages.join("; ")
        )));
    }
    Ok(validated)
}

fn validate_proposal(raw: RawProposal) -> std::result::Result<ValidatedProposal, String> {
    let memory_type = MemoryType::parse(&raw.memory_type).map_err(|e| e.to_string())?;
    let scope = Scope::parse(&raw.scope).map_err(|e| e.to_string())?;
    let sensitivity = Sensitivity::parse(&raw.sensitivity).map_err(|e| e.to_string())?;

    if raw.confidence < 0.0 || raw.confidence > 1.0 {
        return Err(format!(
            "confidence must be in [0, 1], got {}",
            raw.confidence
        ));
    }

    let scope_prefix = match scope {
        Scope::User => "user:",
        Scope::Agent => "agent:",
        Scope::Session => "session:",
        Scope::Project => "project:",
        Scope::Org => "org:",
    };
    if !raw.scope_id.starts_with(scope_prefix) {
        return Err(format!(
            "scope_id {0:?} must start with {1:?} for scope {2:?}",
            raw.scope_id, scope_prefix, raw.scope
        ));
    }

    if raw.title.is_empty() {
        return Err("title must not be empty".to_string());
    }
    if raw.body.is_empty() {
        return Err("body must not be empty".to_string());
    }
    if raw.title.len() > 256 {
        return Err(format!(
            "title too long: {} chars (max 256)",
            raw.title.len()
        ));
    }

    for entity in &raw.entities {
        match entity.role.as_str() {
            "subject" | "object" | "context" => {}
            other => return Err(format!("invalid entity role: {other:?}")),
        }
    }

    Ok(ValidatedProposal {
        proposal_id: ProposalId::new().to_string(),
        memory_id: MemoryId::new().to_string(),
        memory_type,
        scope,
        scope_id: raw.scope_id,
        sensitivity,
        confidence: raw.confidence,
        title: raw.title,
        body: raw.body,
        tags: raw.tags,
        entities: raw.entities,
        supersedes_hint: raw.supersedes_hint,
        extracted_at: Utc::now().to_rfc3339(),
    })
}

/// Build the extraction prompt with conversation content injected.
pub fn build_extraction_prompt(conversation: &str) -> String {
    let template = include_str!("../prompts/extraction/v1.md");
    format!(
        "{template}\n\n---\n\n## Conversation to extract from:\n\n{conversation}\n\n---\n\nReturn the JSON array of extracted memories now."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_extraction() {
        let json = r#"[
            {
                "memory_type": "preference",
                "scope": "user",
                "scope_id": "user:alice",
                "sensitivity": "normal",
                "confidence": 0.95,
                "title": "Prefers Rust",
                "body": "The user prefers Rust over Go.",
                "tags": ["language"],
                "entities": [],
                "supersedes_hint": null
            }
        ]"#;

        let (proposals, errors) = parse_extraction_output(json);
        assert!(errors.is_empty());
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].memory_type, MemoryType::Preference);
        assert_eq!(proposals[0].scope, Scope::User);
        assert_eq!(proposals[0].sensitivity, Sensitivity::Normal);
        assert_eq!(proposals[0].confidence, 0.95);
        assert!(proposals[0].proposal_id.starts_with("prp_"));
        assert!(proposals[0].memory_id.starts_with("mem_"));
    }

    #[test]
    fn parse_empty_array() {
        let (proposals, errors) = parse_extraction_output("[]");
        assert!(errors.is_empty());
        assert!(proposals.is_empty());
    }

    #[test]
    fn reject_invalid_json() {
        let (proposals, errors) = parse_extraction_output("not json");
        assert!(proposals.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("failed to parse JSON array"));
    }

    #[test]
    fn reject_invalid_memory_type() {
        let json = r#"[{
            "memory_type": "unknown_type",
            "scope": "user",
            "scope_id": "user:alice",
            "sensitivity": "normal",
            "confidence": 0.9,
            "title": "Test",
            "body": "Test body",
            "tags": [],
            "entities": []
        }]"#;

        let (proposals, errors) = parse_extraction_output(json);
        assert!(proposals.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("invalid memory_type"));
    }

    #[test]
    fn reject_invalid_scope() {
        let json = r#"[{
            "memory_type": "fact",
            "scope": "invalid",
            "scope_id": "invalid:x",
            "sensitivity": "normal",
            "confidence": 0.9,
            "title": "Test",
            "body": "Test body"
        }]"#;

        let (proposals, errors) = parse_extraction_output(json);
        assert!(proposals.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("invalid scope"));
    }

    #[test]
    fn reject_mismatched_scope_id() {
        let json = r#"[{
            "memory_type": "fact",
            "scope": "user",
            "scope_id": "org:acme",
            "sensitivity": "normal",
            "confidence": 0.9,
            "title": "Test",
            "body": "Test body"
        }]"#;

        let (proposals, errors) = parse_extraction_output(json);
        assert!(proposals.is_empty());
        assert!(errors[0].message.contains("scope_id"));
    }

    #[test]
    fn reject_out_of_range_confidence() {
        let json = r#"[{
            "memory_type": "fact",
            "scope": "user",
            "scope_id": "user:alice",
            "sensitivity": "normal",
            "confidence": 1.5,
            "title": "Test",
            "body": "Body"
        }]"#;

        let (proposals, errors) = parse_extraction_output(json);
        assert!(proposals.is_empty());
        assert!(errors[0].message.contains("confidence"));
    }

    #[test]
    fn reject_empty_title() {
        let json = r#"[{
            "memory_type": "fact",
            "scope": "user",
            "scope_id": "user:alice",
            "sensitivity": "normal",
            "confidence": 0.9,
            "title": "",
            "body": "Body"
        }]"#;

        let (proposals, errors) = parse_extraction_output(json);
        assert!(proposals.is_empty());
        assert!(errors[0].message.contains("title"));
    }

    #[test]
    fn reject_empty_body() {
        let json = r#"[{
            "memory_type": "fact",
            "scope": "user",
            "scope_id": "user:alice",
            "sensitivity": "normal",
            "confidence": 0.9,
            "title": "Title",
            "body": ""
        }]"#;

        let (proposals, errors) = parse_extraction_output(json);
        assert!(proposals.is_empty());
        assert!(errors[0].message.contains("body"));
    }

    #[test]
    fn reject_invalid_entity_role() {
        let json = r#"[{
            "memory_type": "fact",
            "scope": "user",
            "scope_id": "user:alice",
            "sensitivity": "normal",
            "confidence": 0.9,
            "title": "Title",
            "body": "Body",
            "entities": [{"id": "ent_X", "role": "invalid"}]
        }]"#;

        let (proposals, errors) = parse_extraction_output(json);
        assert!(proposals.is_empty());
        assert!(errors[0].message.contains("entity role"));
    }

    #[test]
    fn partial_success() {
        let json = r#"[
            {
                "memory_type": "fact",
                "scope": "user",
                "scope_id": "user:alice",
                "sensitivity": "normal",
                "confidence": 0.9,
                "title": "Valid",
                "body": "Valid body"
            },
            {
                "memory_type": "INVALID",
                "scope": "user",
                "scope_id": "user:alice",
                "sensitivity": "normal",
                "confidence": 0.9,
                "title": "Bad",
                "body": "Bad body"
            },
            {
                "memory_type": "preference",
                "scope": "project",
                "scope_id": "project:alpha",
                "sensitivity": "internal",
                "confidence": 0.8,
                "title": "Also valid",
                "body": "Also valid body"
            }
        ]"#;

        let (proposals, errors) = parse_extraction_output(json);
        assert_eq!(proposals.len(), 2);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].index, Some(1));
    }

    #[test]
    fn strict_mode_rejects_partial() {
        let json = r#"[
            {"memory_type": "fact", "scope": "user", "scope_id": "user:a", "sensitivity": "normal", "confidence": 0.9, "title": "Ok", "body": "Ok"},
            {"memory_type": "BAD", "scope": "user", "scope_id": "user:a", "sensitivity": "normal", "confidence": 0.9, "title": "Bad", "body": "Bad"}
        ]"#;

        let result = parse_extraction_output_strict(json);
        assert!(result.is_err());
    }

    #[test]
    fn all_sensitivity_levels() {
        for level in [
            "normal",
            "internal",
            "pii",
            "secret",
            "medical",
            "legal",
            "financial",
        ] {
            let json = format!(
                r#"[{{"memory_type": "fact", "scope": "user", "scope_id": "user:a", "sensitivity": "{level}", "confidence": 0.9, "title": "T", "body": "B"}}]"#
            );
            let (proposals, errors) = parse_extraction_output(&json);
            assert!(errors.is_empty(), "failed for sensitivity {level}");
            assert_eq!(proposals.len(), 1);
        }
    }

    #[test]
    fn all_memory_types() {
        for mt in [
            "preference",
            "fact",
            "goal",
            "skill",
            "relationship",
            "constraint",
            "episodic",
        ] {
            let json = format!(
                r#"[{{"memory_type": "{mt}", "scope": "user", "scope_id": "user:a", "sensitivity": "normal", "confidence": 0.9, "title": "T", "body": "B"}}]"#
            );
            let (proposals, errors) = parse_extraction_output(&json);
            assert!(errors.is_empty(), "failed for memory_type {mt}");
            assert_eq!(proposals.len(), 1);
        }
    }

    #[test]
    fn all_scopes() {
        for (scope, prefix) in [
            ("user", "user:"),
            ("agent", "agent:"),
            ("session", "session:"),
            ("project", "project:"),
            ("org", "org:"),
        ] {
            let json = format!(
                r#"[{{"memory_type": "fact", "scope": "{scope}", "scope_id": "{prefix}test", "sensitivity": "normal", "confidence": 0.9, "title": "T", "body": "B"}}]"#
            );
            let (proposals, errors) = parse_extraction_output(&json);
            assert!(errors.is_empty(), "failed for scope {scope}");
            assert_eq!(proposals.len(), 1);
        }
    }

    #[test]
    fn sensitivity_requires_review() {
        assert!(!Sensitivity::Normal.requires_review());
        assert!(!Sensitivity::Internal.requires_review());
        assert!(Sensitivity::Pii.requires_review());
        assert!(Sensitivity::Secret.requires_review());
        assert!(Sensitivity::Medical.requires_review());
        assert!(Sensitivity::Legal.requires_review());
        assert!(Sensitivity::Financial.requires_review());
    }

    #[test]
    fn build_prompt_includes_conversation() {
        let prompt = build_extraction_prompt("user: hello\nassistant: hi");
        assert!(prompt.contains("user: hello"));
        assert!(prompt.contains("Extraction Prompt v1"));
        assert!(prompt.contains("Return the JSON array"));
    }

    #[test]
    fn supersedes_hint_roundtrip() {
        let json = r#"[{
            "memory_type": "preference",
            "scope": "user",
            "scope_id": "user:alice",
            "sensitivity": "normal",
            "confidence": 0.95,
            "title": "Prefers Go now",
            "body": "Changed preference from Rust to Go.",
            "supersedes_hint": "Previously preferred Rust over Go"
        }]"#;

        let (proposals, errors) = parse_extraction_output(json);
        assert!(errors.is_empty());
        assert_eq!(
            proposals[0].supersedes_hint.as_deref(),
            Some("Previously preferred Rust over Go")
        );
    }

    #[test]
    fn golden_conversation_extraction() {
        let json = r#"[
            {
                "memory_type": "fact",
                "scope": "user",
                "scope_id": "user:current",
                "sensitivity": "normal",
                "confidence": 0.95,
                "title": "Backend engineer on payments service",
                "body": "The user is a backend engineer working on the payments service.",
                "tags": ["role", "payments"],
                "entities": [],
                "supersedes_hint": null
            },
            {
                "memory_type": "preference",
                "scope": "user",
                "scope_id": "user:current",
                "sensitivity": "normal",
                "confidence": 0.95,
                "title": "Prefers Rust over Go for new microservices",
                "body": "The user prefers Rust over Go when building new microservices.",
                "tags": ["language-preference", "rust", "go"],
                "entities": [],
                "supersedes_hint": null
            },
            {
                "memory_type": "fact",
                "scope": "project",
                "scope_id": "project:payments",
                "sensitivity": "internal",
                "confidence": 0.9,
                "title": "Team standup schedule",
                "body": "Team standup is at 9:30 AM EST every weekday.",
                "tags": ["schedule", "standup"],
                "entities": [],
                "supersedes_hint": null
            }
        ]"#;

        let proposals = parse_extraction_output_strict(json).unwrap();
        assert_eq!(proposals.len(), 3);
        assert_eq!(proposals[0].memory_type, MemoryType::Fact);
        assert_eq!(proposals[1].memory_type, MemoryType::Preference);
        assert_eq!(proposals[2].scope, Scope::Project);
        assert_eq!(proposals[2].sensitivity, Sensitivity::Internal);
    }
}
