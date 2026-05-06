//! Post-extraction secret scan — re-scans proposal content for secrets
//! that the LLM may have leaked or failed to redact.
//!
//! When secrets are found, the proposal is flagged with `sensitivity: secret`
//! and blocked from auto-commit regardless of policy.

use crate::error::Result;
use crate::extraction::{Sensitivity, ValidatedProposal};
use crate::redaction;

/// Result of scanning a proposal.
#[derive(Debug)]
pub struct ScanResult {
    /// Whether secrets were found.
    pub has_secrets: bool,
    /// Number of findings.
    pub finding_count: usize,
    /// Categories of findings.
    pub categories: Vec<String>,
    /// If secrets found, the redacted body text.
    pub redacted_body: Option<String>,
}

/// Scan a proposal's title and body for secrets.
pub fn scan_proposal(proposal: &ValidatedProposal) -> ScanResult {
    let combined = format!("{}\n{}", proposal.title, proposal.body);
    let result = redaction::scan(&combined);

    if result.has_secrets() {
        let categories: Vec<String> = result
            .findings
            .iter()
            .map(|f| f.category.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        let redacted = redaction::redact(&combined);

        ScanResult {
            has_secrets: true,
            finding_count: result.findings.len(),
            categories,
            redacted_body: Some(redacted),
        }
    } else {
        ScanResult {
            has_secrets: false,
            finding_count: 0,
            categories: vec![],
            redacted_body: None,
        }
    }
}

/// Scan and enforce — returns an error if secrets are found and fail_closed is true.
pub fn check_proposal(proposal: &ValidatedProposal, fail_closed: bool) -> Result<ScanResult> {
    let result = scan_proposal(proposal);

    if result.has_secrets && fail_closed {
        return Err(crate::MemoryFsError::Forbidden(format!(
            "proposal {} contains secrets ({} findings: {}), blocked by post-scan",
            proposal.proposal_id,
            result.finding_count,
            result.categories.join(", ")
        )));
    }

    Ok(result)
}

/// Upgrade a proposal's sensitivity to `secret` if secrets are found.
/// Returns the (possibly modified) proposal and the scan result.
pub fn scan_and_upgrade(mut proposal: ValidatedProposal) -> (ValidatedProposal, ScanResult) {
    let result = scan_proposal(&proposal);

    if result.has_secrets {
        proposal.sensitivity = Sensitivity::Secret;
    }

    (proposal, result)
}

/// Batch-scan multiple proposals.
pub fn scan_batch(proposals: &[ValidatedProposal]) -> Vec<(usize, ScanResult)> {
    proposals
        .iter()
        .enumerate()
        .map(|(i, p)| (i, scan_proposal(p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::{MemoryType, Scope, ValidatedProposal};

    fn clean_proposal() -> ValidatedProposal {
        ValidatedProposal {
            proposal_id: "prp_TEST".to_string(),
            memory_id: "mem_TEST".to_string(),
            memory_type: MemoryType::Fact,
            scope: Scope::User,
            scope_id: "user:test".to_string(),
            sensitivity: Sensitivity::Normal,
            confidence: 0.9,
            title: "User role".to_string(),
            body: "The user is a backend engineer.".to_string(),
            tags: vec![],
            entities: vec![],
            supersedes_hint: None,
            extracted_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn secret_proposal() -> ValidatedProposal {
        ValidatedProposal {
            proposal_id: "prp_SECRET".to_string(),
            memory_id: "mem_SECRET".to_string(),
            memory_type: MemoryType::Fact,
            scope: Scope::User,
            scope_id: "user:test".to_string(),
            sensitivity: Sensitivity::Normal,
            confidence: 0.9,
            title: "API configuration".to_string(),
            body: "The user's API key is sk-proj-abc123def456ghi789jklmnopqrstuvwxyz12345678901234"
                .to_string(),
            tags: vec![],
            entities: vec![],
            supersedes_hint: None,
            extracted_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn clean_proposal_passes() {
        let result = scan_proposal(&clean_proposal());
        assert!(!result.has_secrets);
        assert_eq!(result.finding_count, 0);
        assert!(result.categories.is_empty());
        assert!(result.redacted_body.is_none());
    }

    #[test]
    fn secret_proposal_detected() {
        let result = scan_proposal(&secret_proposal());
        assert!(result.has_secrets);
        assert!(result.finding_count > 0);
        assert!(!result.categories.is_empty());
        assert!(result.redacted_body.is_some());
    }

    #[test]
    fn check_clean_proposal_passes() {
        let result = check_proposal(&clean_proposal(), true).unwrap();
        assert!(!result.has_secrets);
    }

    #[test]
    fn check_secret_proposal_fail_closed() {
        let err = check_proposal(&secret_proposal(), true).unwrap_err();
        assert_eq!(err.api_code(), "FORBIDDEN");
    }

    #[test]
    fn check_secret_proposal_fail_open() {
        let result = check_proposal(&secret_proposal(), false).unwrap();
        assert!(result.has_secrets);
    }

    #[test]
    fn upgrade_sensitivity_on_secret() {
        let (upgraded, result) = scan_and_upgrade(secret_proposal());
        assert!(result.has_secrets);
        assert_eq!(upgraded.sensitivity, Sensitivity::Secret);
    }

    #[test]
    fn no_upgrade_on_clean() {
        let (proposal, result) = scan_and_upgrade(clean_proposal());
        assert!(!result.has_secrets);
        assert_eq!(proposal.sensitivity, Sensitivity::Normal);
    }

    #[test]
    fn batch_scan() {
        let proposals = vec![clean_proposal(), secret_proposal(), clean_proposal()];
        let results = scan_batch(&proposals);

        assert_eq!(results.len(), 3);
        assert!(!results[0].1.has_secrets);
        assert!(results[1].1.has_secrets);
        assert!(!results[2].1.has_secrets);
    }

    #[test]
    fn jwt_in_body_detected() {
        let mut p = clean_proposal();
        p.body = "Token: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U".to_string();
        let result = scan_proposal(&p);
        assert!(result.has_secrets);
    }

    #[test]
    fn ssh_key_in_body_detected() {
        let mut p = clean_proposal();
        p.body = "Key: -----BEGIN RSA PRIVATE KEY-----\nMIIE...".to_string();
        let result = scan_proposal(&p);
        assert!(result.has_secrets);
    }

    #[test]
    fn secret_in_title_detected() {
        let mut p = clean_proposal();
        p.title =
            "Config with key sk-proj-abc123def456ghi789jklmnopqrstuvwxyz12345678901234".to_string();
        p.body = "Some normal body text.".to_string();
        let result = scan_proposal(&p);
        assert!(result.has_secrets);
    }

    #[test]
    fn redacted_body_removes_secrets() {
        let result = scan_proposal(&secret_proposal());
        let redacted = result.redacted_body.unwrap();
        assert!(!redacted.contains("sk-proj-"));
        assert!(redacted.contains("[REDACTED"));
    }
}
