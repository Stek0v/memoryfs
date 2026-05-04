//! Memory policy engine — decides whether a proposal should be auto-committed
//! or sent to review, based on sensitivity, confidence, scope, and workspace policy.

use crate::extraction::{Scope, ValidatedProposal};
use crate::policy::ReviewPolicy;

/// Reason(s) a proposal requires review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewReason {
    /// Sensitivity level requires review.
    Sensitivity(String),
    /// Confidence is below the threshold.
    LowConfidence {
        /// Actual confidence score.
        confidence: u64,
        /// Threshold (as integer percentage, e.g. 60).
        threshold: u64,
    },
    /// Org-scope proposals always require review per policy.
    OrgScope,
}

/// Decision for a proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    /// Proposal can be auto-committed.
    AutoCommit,
    /// Proposal requires human review.
    RequiresReview(Vec<ReviewReason>),
}

/// Evaluate a proposal against the review policy.
pub fn evaluate(proposal: &ValidatedProposal, policy: &ReviewPolicy) -> PolicyDecision {
    let mut reasons = Vec::new();

    let sensitivity_str = serde_json::to_value(proposal.sensitivity)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();

    if policy.require_review_for.contains(&sensitivity_str) {
        reasons.push(ReviewReason::Sensitivity(sensitivity_str));
    }

    if proposal.confidence < policy.low_confidence_threshold {
        reasons.push(ReviewReason::LowConfidence {
            confidence: (proposal.confidence * 100.0) as u64,
            threshold: (policy.low_confidence_threshold * 100.0) as u64,
        });
    }

    if policy.scope_org_requires_review && proposal.scope == Scope::Org {
        reasons.push(ReviewReason::OrgScope);
    }

    if reasons.is_empty() {
        PolicyDecision::AutoCommit
    } else {
        PolicyDecision::RequiresReview(reasons)
    }
}

/// Batch-evaluate multiple proposals.
pub fn evaluate_batch(
    proposals: &[ValidatedProposal],
    policy: &ReviewPolicy,
) -> Vec<(usize, PolicyDecision)> {
    proposals
        .iter()
        .enumerate()
        .map(|(i, p)| (i, evaluate(p, policy)))
        .collect()
}

/// Validate that a policy configuration is well-formed.
pub fn validate_policy(policy: &ReviewPolicy) -> crate::Result<()> {
    let valid_sensitivities = [
        "normal",
        "internal",
        "pii",
        "secret",
        "medical",
        "legal",
        "financial",
    ];

    for s in &policy.require_review_for {
        if !valid_sensitivities.contains(&s.as_str()) {
            return Err(crate::MemoryFsError::Validation(format!(
                "invalid sensitivity level in review policy: {s:?}"
            )));
        }
    }

    if policy.low_confidence_threshold < 0.0 || policy.low_confidence_threshold > 1.0 {
        return Err(crate::MemoryFsError::Validation(format!(
            "low_confidence_threshold must be in [0, 1], got {}",
            policy.low_confidence_threshold
        )));
    }

    if policy.review_ttl_hours == 0 {
        return Err(crate::MemoryFsError::Validation(
            "review_ttl_hours must be > 0".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::{MemoryType, Sensitivity, ValidatedProposal};
    use crate::policy::ReviewPolicy;

    fn default_review_policy() -> ReviewPolicy {
        ReviewPolicy {
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
        }
    }

    fn make_proposal(sensitivity: Sensitivity, confidence: f64, scope: Scope) -> ValidatedProposal {
        ValidatedProposal {
            proposal_id: "prp_TEST".to_string(),
            memory_id: "mem_TEST".to_string(),
            memory_type: MemoryType::Fact,
            scope,
            scope_id: match scope {
                Scope::User => "user:test".to_string(),
                Scope::Agent => "agent:test".to_string(),
                Scope::Session => "session:test".to_string(),
                Scope::Project => "project:test".to_string(),
                Scope::Org => "org:test".to_string(),
            },
            sensitivity,
            confidence,
            title: "Test".to_string(),
            body: "Test body".to_string(),
            tags: vec![],
            entities: vec![],
            supersedes_hint: None,
            extracted_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn auto_commit_normal_high_confidence() {
        let policy = default_review_policy();
        let proposal = make_proposal(Sensitivity::Normal, 0.9, Scope::User);
        assert_eq!(evaluate(&proposal, &policy), PolicyDecision::AutoCommit);
    }

    #[test]
    fn auto_commit_internal_high_confidence() {
        let policy = default_review_policy();
        let proposal = make_proposal(Sensitivity::Internal, 0.8, Scope::Project);
        assert_eq!(evaluate(&proposal, &policy), PolicyDecision::AutoCommit);
    }

    #[test]
    fn review_for_pii() {
        let policy = default_review_policy();
        let proposal = make_proposal(Sensitivity::Pii, 0.9, Scope::User);
        match evaluate(&proposal, &policy) {
            PolicyDecision::RequiresReview(reasons) => {
                assert!(reasons
                    .iter()
                    .any(|r| matches!(r, ReviewReason::Sensitivity(_))));
            }
            other => panic!("expected RequiresReview, got {:?}", other),
        }
    }

    #[test]
    fn review_for_secret() {
        let policy = default_review_policy();
        let proposal = make_proposal(Sensitivity::Secret, 0.95, Scope::User);
        match evaluate(&proposal, &policy) {
            PolicyDecision::RequiresReview(reasons) => {
                assert!(reasons
                    .iter()
                    .any(|r| matches!(r, ReviewReason::Sensitivity(_))));
            }
            other => panic!("expected RequiresReview, got {:?}", other),
        }
    }

    #[test]
    fn review_for_medical() {
        let policy = default_review_policy();
        let proposal = make_proposal(Sensitivity::Medical, 0.9, Scope::User);
        match evaluate(&proposal, &policy) {
            PolicyDecision::RequiresReview(reasons) => {
                assert!(reasons
                    .iter()
                    .any(|r| matches!(r, ReviewReason::Sensitivity(_))));
            }
            other => panic!("expected RequiresReview, got {:?}", other),
        }
    }

    #[test]
    fn review_for_legal() {
        let policy = default_review_policy();
        let proposal = make_proposal(Sensitivity::Legal, 0.9, Scope::User);
        match evaluate(&proposal, &policy) {
            PolicyDecision::RequiresReview(reasons) => {
                assert!(reasons
                    .iter()
                    .any(|r| matches!(r, ReviewReason::Sensitivity(_))));
            }
            other => panic!("expected RequiresReview, got {:?}", other),
        }
    }

    #[test]
    fn review_for_financial() {
        let policy = default_review_policy();
        let proposal = make_proposal(Sensitivity::Financial, 0.9, Scope::User);
        match evaluate(&proposal, &policy) {
            PolicyDecision::RequiresReview(reasons) => {
                assert!(reasons
                    .iter()
                    .any(|r| matches!(r, ReviewReason::Sensitivity(_))));
            }
            other => panic!("expected RequiresReview, got {:?}", other),
        }
    }

    #[test]
    fn review_for_low_confidence() {
        let policy = default_review_policy();
        let proposal = make_proposal(Sensitivity::Normal, 0.4, Scope::User);
        match evaluate(&proposal, &policy) {
            PolicyDecision::RequiresReview(reasons) => {
                assert!(reasons
                    .iter()
                    .any(|r| matches!(r, ReviewReason::LowConfidence { .. })));
            }
            other => panic!("expected RequiresReview, got {:?}", other),
        }
    }

    #[test]
    fn review_for_org_scope() {
        let policy = default_review_policy();
        let proposal = make_proposal(Sensitivity::Normal, 0.9, Scope::Org);
        match evaluate(&proposal, &policy) {
            PolicyDecision::RequiresReview(reasons) => {
                assert!(reasons.iter().any(|r| matches!(r, ReviewReason::OrgScope)));
            }
            other => panic!("expected RequiresReview, got {:?}", other),
        }
    }

    #[test]
    fn multiple_reasons() {
        let policy = default_review_policy();
        let proposal = make_proposal(Sensitivity::Pii, 0.3, Scope::Org);
        match evaluate(&proposal, &policy) {
            PolicyDecision::RequiresReview(reasons) => {
                assert_eq!(reasons.len(), 3);
            }
            other => panic!("expected RequiresReview with 3 reasons, got {:?}", other),
        }
    }

    #[test]
    fn org_review_disabled() {
        let mut policy = default_review_policy();
        policy.scope_org_requires_review = false;
        let proposal = make_proposal(Sensitivity::Normal, 0.9, Scope::Org);
        assert_eq!(evaluate(&proposal, &policy), PolicyDecision::AutoCommit);
    }

    #[test]
    fn custom_confidence_threshold() {
        let mut policy = default_review_policy();
        policy.low_confidence_threshold = 0.8;
        let proposal = make_proposal(Sensitivity::Normal, 0.75, Scope::User);
        match evaluate(&proposal, &policy) {
            PolicyDecision::RequiresReview(reasons) => {
                assert!(reasons
                    .iter()
                    .any(|r| matches!(r, ReviewReason::LowConfidence { .. })));
            }
            other => panic!("expected RequiresReview, got {:?}", other),
        }
    }

    #[test]
    fn exact_threshold_does_not_trigger() {
        let mut policy = default_review_policy();
        policy.low_confidence_threshold = 0.6;
        let proposal = make_proposal(Sensitivity::Normal, 0.6, Scope::User);
        assert_eq!(evaluate(&proposal, &policy), PolicyDecision::AutoCommit);
    }

    #[test]
    fn batch_evaluation() {
        let policy = default_review_policy();
        let proposals = vec![
            make_proposal(Sensitivity::Normal, 0.9, Scope::User),
            make_proposal(Sensitivity::Pii, 0.9, Scope::User),
            make_proposal(Sensitivity::Normal, 0.3, Scope::User),
        ];

        let results = evaluate_batch(&proposals, &policy);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].1, PolicyDecision::AutoCommit);
        assert!(matches!(results[1].1, PolicyDecision::RequiresReview(_)));
        assert!(matches!(results[2].1, PolicyDecision::RequiresReview(_)));
    }

    #[test]
    fn empty_review_policy_auto_commits_all() {
        let policy = ReviewPolicy {
            require_review_for: vec![],
            low_confidence_threshold: 0.0,
            scope_org_requires_review: false,
            review_ttl_hours: 168,
        };

        let proposal = make_proposal(Sensitivity::Pii, 0.1, Scope::Org);
        assert_eq!(evaluate(&proposal, &policy), PolicyDecision::AutoCommit);
    }

    #[test]
    fn validate_valid_policy() {
        let policy = default_review_policy();
        validate_policy(&policy).unwrap();
    }

    #[test]
    fn validate_rejects_invalid_sensitivity() {
        let policy = ReviewPolicy {
            require_review_for: vec!["invalid_level".to_string()],
            low_confidence_threshold: 0.6,
            scope_org_requires_review: true,
            review_ttl_hours: 168,
        };
        assert!(validate_policy(&policy).is_err());
    }

    #[test]
    fn validate_rejects_invalid_threshold() {
        let policy = ReviewPolicy {
            require_review_for: vec![],
            low_confidence_threshold: 1.5,
            scope_org_requires_review: true,
            review_ttl_hours: 168,
        };
        assert!(validate_policy(&policy).is_err());
    }

    #[test]
    fn validate_rejects_zero_ttl() {
        let policy = ReviewPolicy {
            require_review_for: vec![],
            low_confidence_threshold: 0.6,
            scope_org_requires_review: true,
            review_ttl_hours: 0,
        };
        assert!(validate_policy(&policy).is_err());
    }

    #[test]
    fn all_sensitivity_levels_handled() {
        let policy = default_review_policy();
        let sensitivities = [
            (Sensitivity::Normal, PolicyDecision::AutoCommit),
            (Sensitivity::Internal, PolicyDecision::AutoCommit),
        ];

        for (sensitivity, expected) in &sensitivities {
            let proposal = make_proposal(*sensitivity, 0.9, Scope::User);
            assert_eq!(evaluate(&proposal, &policy), *expected);
        }

        let review_sensitivities = [
            Sensitivity::Pii,
            Sensitivity::Secret,
            Sensitivity::Medical,
            Sensitivity::Legal,
            Sensitivity::Financial,
        ];

        for sensitivity in &review_sensitivities {
            let proposal = make_proposal(*sensitivity, 0.9, Scope::User);
            assert!(matches!(
                evaluate(&proposal, &policy),
                PolicyDecision::RequiresReview(_)
            ));
        }
    }
}
