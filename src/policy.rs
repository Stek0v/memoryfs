//! Workspace policy: global ACL rules, redaction config, review config.
//!
//! Loaded from `.memoryfs/policy.yaml`. Defines path-based allow/deny rules
//! and sensitivity-based review requirements.

use serde::{Deserialize, Serialize};

/// Top-level workspace policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Schema version (e.g. `memoryfs/v1`).
    pub schema_version: String,
    /// Global ACL rules.
    pub default_acl: AclPolicy,
    /// Redaction configuration.
    pub redaction: RedactionPolicy,
    /// Review requirements.
    pub review: ReviewPolicy,
    /// Indexing configuration.
    pub indexing: IndexingPolicy,
}

/// ACL section of the policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclPolicy {
    /// Must be true — deny by default.
    pub deny_by_default: bool,
    /// Allow rules (path + subjects + actions).
    #[serde(default)]
    pub allow: Vec<AclRule>,
    /// Deny rules (override allow).
    #[serde(default)]
    pub deny: Vec<AclDenyRule>,
}

/// A single allow rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclRule {
    /// Glob path pattern (e.g. `memory/user/**`).
    pub path: String,
    /// Subjects this rule applies to.
    pub subjects: Vec<String>,
    /// Allowed actions.
    pub actions: Vec<String>,
}

/// A single deny rule (overrides allow).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclDenyRule {
    /// Glob path pattern.
    pub path: String,
    /// Subjects to deny.
    pub subjects: Vec<String>,
    /// Actions to deny (if absent, all actions are denied).
    #[serde(default)]
    pub actions: Option<Vec<String>>,
}

/// Redaction configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionPolicy {
    /// Whether redaction failures should block writes.
    #[serde(default = "default_true")]
    pub fail_closed: bool,
}

/// Review configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewPolicy {
    /// Sensitivity levels that require review before commit.
    #[serde(default)]
    pub require_review_for: Vec<String>,
    /// Confidence below this threshold triggers review even for normal sensitivity.
    #[serde(default = "default_low_confidence_threshold")]
    pub low_confidence_threshold: f64,
    /// Whether org-scope proposals always require review.
    #[serde(default = "default_true")]
    pub scope_org_requires_review: bool,
    /// Hours before a pending review expires.
    #[serde(default = "default_review_ttl_hours")]
    pub review_ttl_hours: u64,
}

fn default_low_confidence_threshold() -> f64 {
    0.6
}

fn default_review_ttl_hours() -> u64 {
    168
}

/// Indexing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingPolicy {
    /// Whether to auto-index on commit.
    #[serde(default = "default_true")]
    pub auto_index: bool,
}

fn default_true() -> bool {
    true
}

impl Policy {
    /// Parse a policy from YAML string.
    pub fn from_yaml(yaml: &str) -> crate::Result<Self> {
        serde_yaml::from_str(yaml)
            .map_err(|e| crate::MemoryFsError::Validation(format!("invalid policy YAML: {e}")))
    }

    /// Permissive policy for single-user local MCP mode.
    ///
    /// In local mode the MCP process runs under the user's UID with the data
    /// dir inside their own project — there is no auth boundary to enforce.
    /// `Default::default()` denies everything because it's intended for the
    /// integrated multi-tenant server, but applying that here means the
    /// agent can't even read back what it just wrote. Grant the local
    /// subject full access on `**`; redaction still runs and review still
    /// fires on sensitive content.
    pub fn local_user(subject: &str) -> Self {
        let mut p = Self::default();
        p.default_acl.allow.push(AclRule {
            path: "**".to_string(),
            subjects: vec![subject.to_string()],
            actions: vec![
                "read".into(),
                "write".into(),
                "list".into(),
                "review".into(),
                "commit".into(),
                "revert".into(),
            ],
        });
        p
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            schema_version: "memoryfs/v1".to_string(),
            default_acl: AclPolicy {
                deny_by_default: true,
                allow: Vec::new(),
                deny: Vec::new(),
            },
            redaction: RedactionPolicy { fail_closed: true },
            review: ReviewPolicy {
                require_review_for: vec![
                    "pii".to_string(),
                    "secret".to_string(),
                    "medical".to_string(),
                    "legal".to_string(),
                    "financial".to_string(),
                ],
                low_confidence_threshold: 0.6,
                scope_org_requires_review: true,
                review_ttl_hours: 168,
            },
            indexing: IndexingPolicy { auto_index: true },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_deny_by_default() {
        let p = Policy::default();
        assert!(p.default_acl.deny_by_default);
        assert!(p.redaction.fail_closed);
    }

    #[test]
    fn local_user_grants_full_access_to_subject() {
        let p = Policy::local_user("user:alice");
        assert_eq!(p.default_acl.allow.len(), 1);
        let rule = &p.default_acl.allow[0];
        assert_eq!(rule.path, "**");
        assert_eq!(rule.subjects, vec!["user:alice"]);
        for action in ["read", "write", "list", "review", "commit", "revert"] {
            assert!(
                rule.actions.iter().any(|a| a == action),
                "missing action {action}"
            );
        }
        // Defaults preserved: redaction stays fail-closed, review still fires
        // on sensitive content.
        assert!(p.redaction.fail_closed);
        assert!(p.review.require_review_for.contains(&"secret".to_string()));
    }

    #[test]
    fn parse_minimal_policy() {
        let yaml = r#"
schema_version: memoryfs/v1
default_acl:
  deny_by_default: true
  allow:
    - path: "memory/**"
      subjects: ["owner"]
      actions: ["read", "write"]
redaction:
  fail_closed: true
review:
  require_review_for: ["pii", "secret"]
indexing:
  auto_index: true
"#;
        let p = Policy::from_yaml(yaml).unwrap();
        assert_eq!(p.default_acl.allow.len(), 1);
        assert_eq!(p.default_acl.allow[0].path, "memory/**");
    }
}
