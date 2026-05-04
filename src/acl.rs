//! ACL guard — enforces deny-by-default access control.
//!
//! Evaluates policy rules against (subject, action, path) triples.
//! Deny rules always override allow rules.

use crate::error::{MemoryFsError, Result};
use crate::policy::{AclDenyRule, AclRule, Policy};

/// Actions that can be checked against the ACL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Read a file or memory.
    Read,
    /// Write (create/update) a file or memory.
    Write,
    /// Review a proposed memory.
    Review,
    /// Commit staged changes.
    Commit,
    /// Revert to a prior commit.
    Revert,
    /// List files/memories in a path.
    List,
}

impl Action {
    fn as_str(self) -> &'static str {
        match self {
            Action::Read => "read",
            Action::Write => "write",
            Action::Review => "review",
            Action::Commit => "commit",
            Action::Revert => "revert",
            Action::List => "list",
        }
    }

    /// Parse from a string.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "read" => Ok(Action::Read),
            "write" => Ok(Action::Write),
            "review" => Ok(Action::Review),
            "commit" => Ok(Action::Commit),
            "revert" => Ok(Action::Revert),
            "list" => Ok(Action::List),
            _ => Err(MemoryFsError::Validation(format!("unknown action: {s:?}"))),
        }
    }
}

/// Check whether `subject` may perform `action` on `path` under the given policy.
///
/// Returns `Ok(())` if allowed, `Err(Forbidden)` if denied.
pub fn check(subject: &str, action: Action, path: &str, policy: &Policy) -> Result<()> {
    let normalized = normalize_path(path)?;

    if !policy.default_acl.deny_by_default {
        return Err(MemoryFsError::Validation(
            "policy must have deny_by_default: true".into(),
        ));
    }

    if is_denied(subject, action, &normalized, &policy.default_acl.deny) {
        return Err(MemoryFsError::Forbidden(format!(
            "{subject} denied {action} on {normalized}",
            action = action.as_str(),
        )));
    }

    if is_allowed(subject, action, &normalized, &policy.default_acl.allow) {
        return Ok(());
    }

    Err(MemoryFsError::Forbidden(format!(
        "{subject} has no allow rule for {action} on {normalized}",
        action = action.as_str(),
    )))
}

/// Check whether `subject` may perform `action` on `path`, also considering
/// per-file permissions that must not exceed the policy-level grant.
pub fn check_with_file_permissions(
    subject: &str,
    action: Action,
    path: &str,
    policy: &Policy,
    file_read: &[String],
    file_write: &[String],
) -> Result<()> {
    check(subject, action, path, policy)?;

    let file_subjects = match action {
        Action::Read | Action::List => file_read,
        Action::Write | Action::Review | Action::Commit | Action::Revert => file_write,
    };

    if file_subjects.is_empty() {
        return Ok(());
    }

    if !file_subjects.iter().any(|s| subject_matches(subject, s)) {
        return Err(MemoryFsError::Forbidden(format!(
            "{subject} not in file-level permissions for {action}",
            action = action.as_str(),
        )));
    }

    Ok(())
}

fn normalize_path(path: &str) -> Result<String> {
    if path.is_empty() {
        return Err(MemoryFsError::Validation("path must not be empty".into()));
    }

    if path.contains('\0') {
        return Err(MemoryFsError::Validation(
            "path must not contain null bytes".into(),
        ));
    }

    let segments: Vec<&str> = path.split('/').collect();
    let mut normalized: Vec<&str> = Vec::new();

    for seg in &segments {
        if *seg == ".." {
            return Err(MemoryFsError::Forbidden(
                "path traversal (..) is forbidden".into(),
            ));
        }
        if *seg == "." || seg.is_empty() {
            continue;
        }
        normalized.push(seg);
    }

    if normalized.is_empty() {
        return Err(MemoryFsError::Validation(
            "path resolved to empty after normalization".into(),
        ));
    }

    Ok(normalized.join("/"))
}

fn is_denied(subject: &str, action: Action, path: &str, deny_rules: &[AclDenyRule]) -> bool {
    for rule in deny_rules {
        if !glob_match(&rule.path, path) {
            continue;
        }
        if !rule.subjects.iter().any(|s| subject_matches(subject, s)) {
            continue;
        }
        if let Some(actions) = &rule.actions {
            if !actions.iter().any(|a| a == action.as_str()) {
                continue;
            }
        }
        return true;
    }
    false
}

fn is_allowed(subject: &str, action: Action, path: &str, allow_rules: &[AclRule]) -> bool {
    for rule in allow_rules {
        if !glob_match(&rule.path, path) {
            continue;
        }
        if !rule.subjects.iter().any(|s| subject_matches(subject, s)) {
            continue;
        }
        if !rule.actions.iter().any(|a| a == action.as_str()) {
            continue;
        }
        return true;
    }
    false
}

/// Match a subject against a subject pattern.
///
/// Patterns: `"*"` matches all, `"owner"` matches literal, `"agent:*"` matches
/// any agent, `"user:alice"` matches exact.
fn subject_matches(subject: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern.ends_with(":*") {
        let prefix = &pattern[..pattern.len() - 1];
        return subject.starts_with(prefix);
    }
    subject == pattern
}

/// Simple glob matcher for workspace paths.
///
/// Supports:
/// - `*` — matches any single path segment (no `/`)
/// - `**` — matches zero or more path segments (including `/`)
/// - literal segments — exact match
fn glob_match(pattern: &str, path: &str) -> bool {
    let pat_segments: Vec<&str> = pattern.split('/').collect();
    let path_segments: Vec<&str> = path.split('/').collect();
    glob_match_segments(&pat_segments, &path_segments)
}

fn glob_match_segments(pat: &[&str], path: &[&str]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }

    if pat[0] == "**" {
        if pat.len() == 1 {
            return true;
        }
        for i in 0..=path.len() {
            if glob_match_segments(&pat[1..], &path[i..]) {
                return true;
            }
        }
        return false;
    }

    if path.is_empty() {
        return false;
    }

    if pat[0] == "*" || pat[0] == path[0] {
        return glob_match_segments(&pat[1..], &path[1..]);
    }

    false
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::*;

    fn base_policy() -> Policy {
        Policy {
            schema_version: "memoryfs/v1".to_string(),
            default_acl: AclPolicy {
                deny_by_default: true,
                allow: vec![],
                deny: vec![],
            },
            redaction: RedactionPolicy { fail_closed: true },
            review: ReviewPolicy {
                require_review_for: vec![],
                low_confidence_threshold: 0.6,
                scope_org_requires_review: true,
                review_ttl_hours: 168,
            },
            indexing: IndexingPolicy { auto_index: true },
        }
    }

    fn policy_with_allow(rules: Vec<AclRule>) -> Policy {
        let mut p = base_policy();
        p.default_acl.allow = rules;
        p
    }

    fn policy_with_allow_deny(allow: Vec<AclRule>, deny: Vec<AclDenyRule>) -> Policy {
        let mut p = base_policy();
        p.default_acl.allow = allow;
        p.default_acl.deny = deny;
        p
    }

    fn allow_rule(path: &str, subjects: &[&str], actions: &[&str]) -> AclRule {
        AclRule {
            path: path.to_string(),
            subjects: subjects.iter().map(|s| s.to_string()).collect(),
            actions: actions.iter().map(|a| a.to_string()).collect(),
        }
    }

    fn deny_rule(path: &str, subjects: &[&str]) -> AclDenyRule {
        AclDenyRule {
            path: path.to_string(),
            subjects: subjects.iter().map(|s| s.to_string()).collect(),
            actions: None,
        }
    }

    fn deny_rule_actions(path: &str, subjects: &[&str], actions: &[&str]) -> AclDenyRule {
        AclDenyRule {
            path: path.to_string(),
            subjects: subjects.iter().map(|s| s.to_string()).collect(),
            actions: Some(actions.iter().map(|a| a.to_string()).collect()),
        }
    }

    // ── deny by default ──

    #[test]
    fn deny_by_default_no_rules() {
        let p = base_policy();
        let err = check("user:alice", Action::Read, "memory/user/prefs.md", &p).unwrap_err();
        assert_eq!(err.api_code(), "FORBIDDEN");
    }

    #[test]
    fn deny_by_default_wrong_subject() {
        let p = policy_with_allow(vec![allow_rule("memory/**", &["user:bob"], &["read"])]);
        let err = check("user:alice", Action::Read, "memory/user/prefs.md", &p).unwrap_err();
        assert_eq!(err.api_code(), "FORBIDDEN");
    }

    #[test]
    fn deny_by_default_wrong_action() {
        let p = policy_with_allow(vec![allow_rule("memory/**", &["user:alice"], &["read"])]);
        let err = check("user:alice", Action::Write, "memory/user/prefs.md", &p).unwrap_err();
        assert_eq!(err.api_code(), "FORBIDDEN");
    }

    #[test]
    fn deny_by_default_wrong_path() {
        let p = policy_with_allow(vec![allow_rule(
            "memory/user/**",
            &["user:alice"],
            &["read"],
        )]);
        let err = check("user:alice", Action::Read, "conversations/c.md", &p).unwrap_err();
        assert_eq!(err.api_code(), "FORBIDDEN");
    }

    // ── basic allow ──

    #[test]
    fn allow_exact_path() {
        let p = policy_with_allow(vec![allow_rule(
            "memory/user/prefs.md",
            &["user:alice"],
            &["read"],
        )]);
        check("user:alice", Action::Read, "memory/user/prefs.md", &p).unwrap();
    }

    #[test]
    fn allow_glob_star() {
        let p = policy_with_allow(vec![allow_rule(
            "memory/user/*",
            &["user:alice"],
            &["read"],
        )]);
        check("user:alice", Action::Read, "memory/user/prefs.md", &p).unwrap();
    }

    #[test]
    fn star_does_not_cross_directories() {
        let p = policy_with_allow(vec![allow_rule("memory/*", &["user:alice"], &["read"])]);
        let err = check("user:alice", Action::Read, "memory/user/prefs.md", &p).unwrap_err();
        assert_eq!(err.api_code(), "FORBIDDEN");
    }

    #[test]
    fn allow_glob_doublestar() {
        let p = policy_with_allow(vec![allow_rule("memory/**", &["user:alice"], &["read"])]);
        check("user:alice", Action::Read, "memory/user/prefs.md", &p).unwrap();
        check("user:alice", Action::Read, "memory/deep/nested/file.md", &p).unwrap();
    }

    #[test]
    fn doublestar_at_root() {
        let p = policy_with_allow(vec![allow_rule("**", &["owner"], &["read", "write"])]);
        check("owner", Action::Read, "any/path/file.md", &p).unwrap();
        check("owner", Action::Write, "root.md", &p).unwrap();
    }

    #[test]
    fn allow_multiple_actions() {
        let p = policy_with_allow(vec![allow_rule(
            "memory/**",
            &["user:alice"],
            &["read", "write", "list"],
        )]);
        check("user:alice", Action::Read, "memory/x.md", &p).unwrap();
        check("user:alice", Action::Write, "memory/x.md", &p).unwrap();
        check("user:alice", Action::List, "memory/x.md", &p).unwrap();
        check("user:alice", Action::Commit, "memory/x.md", &p).unwrap_err();
    }

    #[test]
    fn allow_wildcard_subject() {
        let p = policy_with_allow(vec![allow_rule("public/**", &["*"], &["read"])]);
        check("user:anyone", Action::Read, "public/readme.md", &p).unwrap();
        check("agent:bot", Action::Read, "public/readme.md", &p).unwrap();
    }

    #[test]
    fn allow_agent_wildcard() {
        let p = policy_with_allow(vec![allow_rule(
            "runs/**",
            &["agent:*"],
            &["read", "write"],
        )]);
        check("agent:extractor", Action::Read, "runs/r1.md", &p).unwrap();
        check("agent:reviewer", Action::Write, "runs/r2.md", &p).unwrap();
        check("user:alice", Action::Read, "runs/r1.md", &p).unwrap_err();
    }

    #[test]
    fn allow_multiple_rules_first_match() {
        let p = policy_with_allow(vec![
            allow_rule("memory/user/**", &["user:alice"], &["read"]),
            allow_rule("memory/org/**", &["user:alice"], &["read", "write"]),
        ]);
        check("user:alice", Action::Read, "memory/user/a.md", &p).unwrap();
        check("user:alice", Action::Write, "memory/org/b.md", &p).unwrap();
        check("user:alice", Action::Write, "memory/user/a.md", &p).unwrap_err();
    }

    // ── deny overrides allow ──

    #[test]
    fn deny_overrides_allow() {
        let p = policy_with_allow_deny(
            vec![allow_rule("memory/**", &["user:alice"], &["read", "write"])],
            vec![deny_rule("memory/secrets/**", &["user:alice"])],
        );
        check("user:alice", Action::Read, "memory/user/a.md", &p).unwrap();
        let err = check("user:alice", Action::Read, "memory/secrets/key.md", &p).unwrap_err();
        assert_eq!(err.api_code(), "FORBIDDEN");
    }

    #[test]
    fn deny_with_specific_actions() {
        let p = policy_with_allow_deny(
            vec![allow_rule("memory/**", &["user:alice"], &["read", "write"])],
            vec![deny_rule_actions(
                "memory/readonly/**",
                &["user:alice"],
                &["write"],
            )],
        );
        check("user:alice", Action::Read, "memory/readonly/a.md", &p).unwrap();
        check("user:alice", Action::Write, "memory/readonly/a.md", &p).unwrap_err();
    }

    #[test]
    fn deny_all_subjects() {
        let p = policy_with_allow_deny(
            vec![allow_rule("**", &["*"], &["read", "write"])],
            vec![deny_rule("restricted/**", &["*"])],
        );
        check("user:alice", Action::Read, "restricted/x.md", &p).unwrap_err();
        check("agent:bot", Action::Read, "restricted/x.md", &p).unwrap_err();
        check("user:alice", Action::Read, "public/x.md", &p).unwrap();
    }

    #[test]
    fn deny_agent_wildcard() {
        let p = policy_with_allow_deny(
            vec![allow_rule("**", &["*"], &["read", "write"])],
            vec![deny_rule("admin/**", &["agent:*"])],
        );
        check("agent:bot", Action::Read, "admin/config.md", &p).unwrap_err();
        check("user:admin", Action::Read, "admin/config.md", &p).unwrap();
    }

    // ── path normalization ──

    #[test]
    fn path_traversal_blocked() {
        let p = policy_with_allow(vec![allow_rule("**", &["*"], &["read"])]);
        let err = check("user:alice", Action::Read, "memory/../etc/passwd", &p).unwrap_err();
        assert_eq!(err.api_code(), "FORBIDDEN");
    }

    #[test]
    fn path_traversal_at_start() {
        let p = policy_with_allow(vec![allow_rule("**", &["*"], &["read"])]);
        let err = check("user:alice", Action::Read, "../outside", &p).unwrap_err();
        assert_eq!(err.api_code(), "FORBIDDEN");
    }

    #[test]
    fn path_traversal_middle() {
        let p = policy_with_allow(vec![allow_rule("**", &["*"], &["read"])]);
        let err = check("user:alice", Action::Read, "a/b/../../c", &p).unwrap_err();
        assert_eq!(err.api_code(), "FORBIDDEN");
    }

    #[test]
    fn dot_segments_stripped() {
        let p = policy_with_allow(vec![allow_rule("memory/**", &["user:alice"], &["read"])]);
        check("user:alice", Action::Read, "./memory/./user/prefs.md", &p).unwrap();
    }

    #[test]
    fn double_slashes_normalized() {
        let p = policy_with_allow(vec![allow_rule("memory/**", &["user:alice"], &["read"])]);
        check("user:alice", Action::Read, "memory//user//prefs.md", &p).unwrap();
    }

    #[test]
    fn null_byte_rejected() {
        let p = base_policy();
        let err = check("user:alice", Action::Read, "memory/\0evil.md", &p).unwrap_err();
        assert_eq!(err.api_code(), "VALIDATION");
    }

    #[test]
    fn empty_path_rejected() {
        let p = base_policy();
        let err = check("user:alice", Action::Read, "", &p).unwrap_err();
        assert_eq!(err.api_code(), "VALIDATION");
    }

    // ── unicode normalization attacks ──

    #[test]
    fn unicode_path_literal_match() {
        let p = policy_with_allow(vec![allow_rule(
            "memory/café/**",
            &["user:alice"],
            &["read"],
        )]);
        check("user:alice", Action::Read, "memory/café/file.md", &p).unwrap();
    }

    #[test]
    fn unicode_nfd_vs_nfc_mismatch() {
        let p = policy_with_allow(vec![allow_rule(
            "memory/cafe\u{0301}/**",
            &["user:alice"],
            &["read"],
        )]);
        check("user:alice", Action::Read, "memory/caf\u{00e9}/file.md", &p).unwrap_err();
    }

    // ── per-file permissions ──

    #[test]
    fn file_perms_allow_owner() {
        let p = policy_with_allow(vec![allow_rule(
            "memory/**",
            &["user:alice"],
            &["read", "write"],
        )]);
        check_with_file_permissions(
            "user:alice",
            Action::Read,
            "memory/m.md",
            &p,
            &["owner".to_string()],
            &["owner".to_string()],
        )
        .unwrap_err();
    }

    #[test]
    fn file_perms_match_subject() {
        let p = policy_with_allow(vec![allow_rule(
            "memory/**",
            &["user:alice"],
            &["read", "write"],
        )]);
        check_with_file_permissions(
            "user:alice",
            Action::Read,
            "memory/m.md",
            &p,
            &["user:alice".to_string()],
            &["user:alice".to_string()],
        )
        .unwrap();
    }

    #[test]
    fn file_perms_empty_means_no_restriction() {
        let p = policy_with_allow(vec![allow_rule("memory/**", &["user:alice"], &["read"])]);
        check_with_file_permissions("user:alice", Action::Read, "memory/m.md", &p, &[], &[])
            .unwrap();
    }

    #[test]
    fn file_perms_write_checked_for_write_action() {
        let p = policy_with_allow(vec![allow_rule(
            "memory/**",
            &["user:alice"],
            &["read", "write"],
        )]);
        check_with_file_permissions(
            "user:alice",
            Action::Write,
            "memory/m.md",
            &p,
            &["user:alice".to_string()],
            &[],
        )
        .unwrap();

        check_with_file_permissions(
            "user:alice",
            Action::Write,
            "memory/m.md",
            &p,
            &["user:alice".to_string()],
            &["user:bob".to_string()],
        )
        .unwrap_err();
    }

    #[test]
    fn file_perms_wildcard_subject() {
        let p = policy_with_allow(vec![allow_rule("memory/**", &["*"], &["read"])]);
        check_with_file_permissions(
            "user:alice",
            Action::Read,
            "memory/m.md",
            &p,
            &["*".to_string()],
            &[],
        )
        .unwrap();
    }

    // ── policy validation ──

    #[test]
    fn deny_by_default_must_be_true() {
        let mut p = base_policy();
        p.default_acl.deny_by_default = false;
        let err = check("user:alice", Action::Read, "x.md", &p).unwrap_err();
        assert_eq!(err.api_code(), "VALIDATION");
    }

    // ── action parsing ──

    #[test]
    fn action_parse_all() {
        assert_eq!(Action::parse("read").unwrap(), Action::Read);
        assert_eq!(Action::parse("write").unwrap(), Action::Write);
        assert_eq!(Action::parse("review").unwrap(), Action::Review);
        assert_eq!(Action::parse("commit").unwrap(), Action::Commit);
        assert_eq!(Action::parse("revert").unwrap(), Action::Revert);
        assert_eq!(Action::parse("list").unwrap(), Action::List);
    }

    #[test]
    fn action_parse_unknown() {
        assert!(Action::parse("delete").is_err());
    }

    #[test]
    fn action_display() {
        assert_eq!(format!("{}", Action::Read), "read");
        assert_eq!(format!("{}", Action::Write), "write");
    }

    // ── glob matching edge cases ──

    #[test]
    fn glob_exact_single_segment() {
        assert!(glob_match("file.md", "file.md"));
        assert!(!glob_match("file.md", "other.md"));
    }

    #[test]
    fn glob_exact_multi_segment() {
        assert!(glob_match("a/b/c", "a/b/c"));
        assert!(!glob_match("a/b/c", "a/b/d"));
    }

    #[test]
    fn glob_star_single() {
        assert!(glob_match("a/*/c", "a/b/c"));
        assert!(glob_match("a/*/c", "a/x/c"));
        assert!(!glob_match("a/*/c", "a/b/d/c"));
    }

    #[test]
    fn glob_doublestar_middle() {
        assert!(glob_match("a/**/c", "a/c"));
        assert!(glob_match("a/**/c", "a/b/c"));
        assert!(glob_match("a/**/c", "a/b/d/c"));
    }

    #[test]
    fn glob_doublestar_only() {
        assert!(glob_match("**", "a"));
        assert!(glob_match("**", "a/b/c"));
    }

    #[test]
    fn glob_empty_pattern_matches_empty() {
        assert!(glob_match_segments(&[], &[]));
        assert!(!glob_match_segments(&[], &["a"]));
    }

    #[test]
    fn glob_trailing_doublestar() {
        assert!(glob_match("a/**", "a/b"));
        assert!(glob_match("a/**", "a/b/c/d"));
        assert!(!glob_match("a/**", "b/c"));
    }

    // ── subject matching ──

    #[test]
    fn subject_exact_match() {
        assert!(subject_matches("user:alice", "user:alice"));
        assert!(!subject_matches("user:alice", "user:bob"));
    }

    #[test]
    fn subject_wildcard_all() {
        assert!(subject_matches("user:alice", "*"));
        assert!(subject_matches("agent:bot", "*"));
        assert!(subject_matches("owner", "*"));
    }

    #[test]
    fn subject_prefix_wildcard() {
        assert!(subject_matches("agent:extractor", "agent:*"));
        assert!(subject_matches("agent:reviewer", "agent:*"));
        assert!(!subject_matches("user:alice", "agent:*"));
    }

    #[test]
    fn subject_user_prefix_wildcard() {
        assert!(subject_matches("user:alice", "user:*"));
        assert!(!subject_matches("agent:bot", "user:*"));
    }

    // ── complex scenarios ──

    #[test]
    fn owner_full_access_agents_restricted() {
        let p = policy_with_allow_deny(
            vec![
                allow_rule(
                    "**",
                    &["owner"],
                    &["read", "write", "commit", "revert", "list", "review"],
                ),
                allow_rule("memory/**", &["agent:*"], &["read", "write"]),
                allow_rule("runs/**", &["agent:*"], &["read", "write"]),
            ],
            vec![deny_rule("memory/secrets/**", &["agent:*"])],
        );

        check("owner", Action::Write, "memory/secrets/key.md", &p).unwrap();
        check("agent:extractor", Action::Write, "memory/user/a.md", &p).unwrap();
        check(
            "agent:extractor",
            Action::Write,
            "memory/secrets/key.md",
            &p,
        )
        .unwrap_err();
        check("agent:extractor", Action::Commit, "memory/user/a.md", &p).unwrap_err();
    }

    #[test]
    fn multi_subject_rule() {
        let p = policy_with_allow(vec![allow_rule(
            "shared/**",
            &["user:alice", "user:bob"],
            &["read", "write"],
        )]);
        check("user:alice", Action::Read, "shared/doc.md", &p).unwrap();
        check("user:bob", Action::Write, "shared/doc.md", &p).unwrap();
        check("user:charlie", Action::Read, "shared/doc.md", &p).unwrap_err();
    }

    #[test]
    fn all_six_actions() {
        let p = policy_with_allow(vec![allow_rule(
            "**",
            &["owner"],
            &["read", "write", "review", "commit", "revert", "list"],
        )]);
        check("owner", Action::Read, "x.md", &p).unwrap();
        check("owner", Action::Write, "x.md", &p).unwrap();
        check("owner", Action::Review, "x.md", &p).unwrap();
        check("owner", Action::Commit, "x.md", &p).unwrap();
        check("owner", Action::Revert, "x.md", &p).unwrap();
        check("owner", Action::List, "x.md", &p).unwrap();
    }

    #[test]
    fn deeply_nested_path_with_doublestar() {
        let p = policy_with_allow(vec![allow_rule("workspace/**", &["user:alice"], &["read"])]);
        check(
            "user:alice",
            Action::Read,
            "workspace/a/b/c/d/e/f/g/h.md",
            &p,
        )
        .unwrap();
    }

    #[test]
    fn multiple_deny_rules() {
        let p = policy_with_allow_deny(
            vec![allow_rule("**", &["*"], &["read", "write"])],
            vec![
                deny_rule("secrets/**", &["agent:*"]),
                deny_rule("admin/**", &["agent:*"]),
                deny_rule_actions("readonly/**", &["*"], &["write"]),
            ],
        );

        check("agent:bot", Action::Read, "secrets/x.md", &p).unwrap_err();
        check("agent:bot", Action::Read, "admin/x.md", &p).unwrap_err();
        check("user:alice", Action::Write, "readonly/x.md", &p).unwrap_err();
        check("user:alice", Action::Read, "readonly/x.md", &p).unwrap();
        check("user:alice", Action::Read, "secrets/x.md", &p).unwrap();
    }
}
