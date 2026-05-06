//! Inbox — pending memory proposals awaiting review or auto-commit.
//!
//! Proposals enter the inbox via extraction. Based on policy, they are either
//! auto-committed or held for human review. Review actions (approve/reject)
//! move proposals to their final state.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::RwLock;

use crate::error::{MemoryFsError, Result};
use crate::extraction::ValidatedProposal;

/// Status of a proposal in the inbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    /// Awaiting human review.
    PendingReview,
    /// Approved and committed.
    Approved,
    /// Rejected by reviewer.
    Rejected,
    /// Expired (review TTL exceeded).
    Expired,
}

/// A proposal stored in the inbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxEntry {
    /// The validated proposal.
    pub proposal: ValidatedProposal,
    /// Current status.
    pub status: ProposalStatus,
    /// Why this proposal requires review.
    pub review_reasons: Vec<String>,
    /// When the proposal was added to the inbox.
    pub added_at: DateTime<Utc>,
    /// When the proposal expires (if review TTL is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Review decision details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_decision: Option<ReviewDecision>,
}

/// Review decision recorded on a proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewDecision {
    /// Who reviewed.
    pub reviewer: String,
    /// When reviewed.
    pub reviewed_at: DateTime<Utc>,
    /// Decision type.
    pub decision: ReviewAction,
    /// Optional reason/comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Possible review actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    /// Approve the proposal.
    Approved,
    /// Reject the proposal.
    Rejected,
}

/// In-memory inbox store for proposals.
pub struct Inbox {
    entries: RwLock<BTreeMap<String, InboxEntry>>,
}

impl Inbox {
    /// Create an empty inbox.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(BTreeMap::new()),
        }
    }

    /// Add a proposal to the inbox with pending_review status.
    pub fn add(
        &self,
        proposal: ValidatedProposal,
        review_reasons: Vec<String>,
        review_ttl_hours: Option<u64>,
    ) -> Result<String> {
        let now = Utc::now();
        let expires_at = review_ttl_hours.map(|h| now + chrono::Duration::hours(h as i64));
        let id = proposal.proposal_id.clone();

        let entry = InboxEntry {
            proposal,
            status: ProposalStatus::PendingReview,
            review_reasons,
            added_at: now,
            expires_at,
            review_decision: None,
        };

        let mut entries = self.entries.write().unwrap();
        entries.insert(id.clone(), entry);
        Ok(id)
    }

    /// Get a proposal by ID.
    pub fn get(&self, proposal_id: &str) -> Result<InboxEntry> {
        let entries = self.entries.read().unwrap();
        entries
            .get(proposal_id)
            .cloned()
            .ok_or_else(|| MemoryFsError::NotFound(format!("proposal {proposal_id}")))
    }

    /// List all proposals, optionally filtered by status.
    pub fn list(&self, status_filter: Option<ProposalStatus>) -> Vec<InboxEntry> {
        let entries = self.entries.read().unwrap();
        entries
            .values()
            .filter(|e| match &status_filter {
                Some(s) => &e.status == s,
                None => true,
            })
            .cloned()
            .collect()
    }

    /// Count proposals by status.
    pub fn count_by_status(&self) -> BTreeMap<String, usize> {
        let entries = self.entries.read().unwrap();
        let mut counts = BTreeMap::new();
        for entry in entries.values() {
            let status = serde_json::to_value(&entry.status)
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown".to_string());
            *counts.entry(status).or_insert(0) += 1;
        }
        counts
    }

    /// Review a proposal — approve or reject.
    pub fn review(
        &self,
        proposal_id: &str,
        reviewer: &str,
        action: ReviewAction,
        reason: Option<String>,
    ) -> Result<InboxEntry> {
        let mut entries = self.entries.write().unwrap();
        let entry = entries
            .get_mut(proposal_id)
            .ok_or_else(|| MemoryFsError::NotFound(format!("proposal {proposal_id}")))?;

        if entry.status != ProposalStatus::PendingReview {
            return Err(MemoryFsError::Conflict(format!(
                "proposal {proposal_id} is not pending review (status: {:?})",
                entry.status
            )));
        }

        let new_status = match action {
            ReviewAction::Approved => ProposalStatus::Approved,
            ReviewAction::Rejected => ProposalStatus::Rejected,
        };

        entry.status = new_status;
        entry.review_decision = Some(ReviewDecision {
            reviewer: reviewer.to_string(),
            reviewed_at: Utc::now(),
            decision: action,
            reason,
        });

        Ok(entry.clone())
    }

    /// Expire proposals that have passed their TTL.
    pub fn expire_stale(&self) -> Vec<String> {
        let now = Utc::now();
        let mut entries = self.entries.write().unwrap();
        let mut expired = Vec::new();

        for (id, entry) in entries.iter_mut() {
            if entry.status == ProposalStatus::PendingReview {
                if let Some(expires_at) = entry.expires_at {
                    if now >= expires_at {
                        entry.status = ProposalStatus::Expired;
                        expired.push(id.clone());
                    }
                }
            }
        }

        expired
    }

    /// Remove a proposal from the inbox.
    pub fn remove(&self, proposal_id: &str) -> Result<InboxEntry> {
        let mut entries = self.entries.write().unwrap();
        entries
            .remove(proposal_id)
            .ok_or_else(|| MemoryFsError::NotFound(format!("proposal {proposal_id}")))
    }

    /// Total number of entries.
    pub fn len(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    /// Whether the inbox is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.read().unwrap().is_empty()
    }
}

impl Default for Inbox {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::{MemoryType, Scope, Sensitivity};

    fn make_proposal(id_suffix: &str) -> ValidatedProposal {
        ValidatedProposal {
            proposal_id: format!("prp_{id_suffix}"),
            memory_id: format!("mem_{id_suffix}"),
            memory_type: MemoryType::Fact,
            scope: Scope::User,
            scope_id: "user:test".to_string(),
            sensitivity: Sensitivity::Normal,
            confidence: 0.9,
            title: format!("Title {id_suffix}"),
            body: format!("Body {id_suffix}"),
            tags: vec![],
            entities: vec![],
            supersedes_hint: None,
            extracted_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn add_and_get() {
        let inbox = Inbox::new();
        let id = inbox
            .add(make_proposal("001"), vec!["sensitivity".to_string()], None)
            .unwrap();

        let entry = inbox.get(&id).unwrap();
        assert_eq!(entry.status, ProposalStatus::PendingReview);
        assert_eq!(entry.proposal.title, "Title 001");
        assert_eq!(entry.review_reasons, vec!["sensitivity"]);
    }

    #[test]
    fn get_nonexistent_returns_not_found() {
        let inbox = Inbox::new();
        let err = inbox.get("prp_nonexistent").unwrap_err();
        assert_eq!(err.api_code(), "NOT_FOUND");
    }

    #[test]
    fn list_all() {
        let inbox = Inbox::new();
        inbox.add(make_proposal("001"), vec![], None).unwrap();
        inbox.add(make_proposal("002"), vec![], None).unwrap();

        let all = inbox.list(None);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn list_filtered_by_status() {
        let inbox = Inbox::new();
        inbox.add(make_proposal("001"), vec![], None).unwrap();
        inbox.add(make_proposal("002"), vec![], None).unwrap();

        inbox
            .review("prp_001", "user:reviewer", ReviewAction::Approved, None)
            .unwrap();

        let pending = inbox.list(Some(ProposalStatus::PendingReview));
        assert_eq!(pending.len(), 1);

        let approved = inbox.list(Some(ProposalStatus::Approved));
        assert_eq!(approved.len(), 1);
    }

    #[test]
    fn approve_proposal() {
        let inbox = Inbox::new();
        inbox
            .add(make_proposal("001"), vec!["pii".to_string()], None)
            .unwrap();

        let entry = inbox
            .review(
                "prp_001",
                "user:alice",
                ReviewAction::Approved,
                Some("looks good".to_string()),
            )
            .unwrap();

        assert_eq!(entry.status, ProposalStatus::Approved);
        let decision = entry.review_decision.unwrap();
        assert_eq!(decision.reviewer, "user:alice");
        assert_eq!(decision.decision, ReviewAction::Approved);
        assert_eq!(decision.reason.as_deref(), Some("looks good"));
    }

    #[test]
    fn reject_proposal() {
        let inbox = Inbox::new();
        inbox.add(make_proposal("001"), vec![], None).unwrap();

        let entry = inbox
            .review(
                "prp_001",
                "user:bob",
                ReviewAction::Rejected,
                Some("not accurate".to_string()),
            )
            .unwrap();

        assert_eq!(entry.status, ProposalStatus::Rejected);
    }

    #[test]
    fn cannot_review_already_reviewed() {
        let inbox = Inbox::new();
        inbox.add(make_proposal("001"), vec![], None).unwrap();

        inbox
            .review("prp_001", "user:alice", ReviewAction::Approved, None)
            .unwrap();

        let err = inbox
            .review("prp_001", "user:bob", ReviewAction::Rejected, None)
            .unwrap_err();
        assert_eq!(err.api_code(), "CONFLICT");
    }

    #[test]
    fn count_by_status() {
        let inbox = Inbox::new();
        inbox.add(make_proposal("001"), vec![], None).unwrap();
        inbox.add(make_proposal("002"), vec![], None).unwrap();
        inbox.add(make_proposal("003"), vec![], None).unwrap();

        inbox
            .review("prp_001", "user:a", ReviewAction::Approved, None)
            .unwrap();
        inbox
            .review("prp_002", "user:a", ReviewAction::Rejected, None)
            .unwrap();

        let counts = inbox.count_by_status();
        assert_eq!(counts.get("approved"), Some(&1));
        assert_eq!(counts.get("rejected"), Some(&1));
        assert_eq!(counts.get("pending_review"), Some(&1));
    }

    #[test]
    fn expire_stale_proposals() {
        let inbox = Inbox::new();

        // Add with very short TTL
        let id = inbox
            .add(make_proposal("001"), vec!["pii".to_string()], Some(0))
            .unwrap();

        // The entry expires immediately (TTL=0 hours)
        let expired = inbox.expire_stale();
        assert_eq!(expired, vec![id]);

        let entry = inbox.get("prp_001").unwrap();
        assert_eq!(entry.status, ProposalStatus::Expired);
    }

    #[test]
    fn non_expired_not_affected() {
        let inbox = Inbox::new();
        inbox.add(make_proposal("001"), vec![], Some(168)).unwrap();

        let expired = inbox.expire_stale();
        assert!(expired.is_empty());

        let entry = inbox.get("prp_001").unwrap();
        assert_eq!(entry.status, ProposalStatus::PendingReview);
    }

    #[test]
    fn remove_proposal() {
        let inbox = Inbox::new();
        inbox.add(make_proposal("001"), vec![], None).unwrap();
        assert_eq!(inbox.len(), 1);

        let removed = inbox.remove("prp_001").unwrap();
        assert_eq!(removed.proposal.title, "Title 001");
        assert_eq!(inbox.len(), 0);
    }

    #[test]
    fn remove_nonexistent_returns_not_found() {
        let inbox = Inbox::new();
        let err = inbox.remove("prp_missing").unwrap_err();
        assert_eq!(err.api_code(), "NOT_FOUND");
    }

    #[test]
    fn is_empty_and_len() {
        let inbox = Inbox::new();
        assert!(inbox.is_empty());
        assert_eq!(inbox.len(), 0);

        inbox.add(make_proposal("001"), vec![], None).unwrap();
        assert!(!inbox.is_empty());
        assert_eq!(inbox.len(), 1);
    }

    #[test]
    fn proposals_with_expires_at() {
        let inbox = Inbox::new();
        inbox.add(make_proposal("001"), vec![], Some(24)).unwrap();

        let entry = inbox.get("prp_001").unwrap();
        assert!(entry.expires_at.is_some());
    }

    #[test]
    fn proposals_without_expires_at() {
        let inbox = Inbox::new();
        inbox.add(make_proposal("001"), vec![], None).unwrap();

        let entry = inbox.get("prp_001").unwrap();
        assert!(entry.expires_at.is_none());
    }

    #[test]
    fn approved_proposals_not_expired() {
        let inbox = Inbox::new();
        inbox.add(make_proposal("001"), vec![], Some(0)).unwrap();

        // Approve before expire runs
        inbox
            .review("prp_001", "user:a", ReviewAction::Approved, None)
            .unwrap();

        let expired = inbox.expire_stale();
        assert!(expired.is_empty());

        let entry = inbox.get("prp_001").unwrap();
        assert_eq!(entry.status, ProposalStatus::Approved);
    }

    #[test]
    fn review_reasons_preserved() {
        let inbox = Inbox::new();
        let reasons = vec![
            "sensitivity".to_string(),
            "low_confidence".to_string(),
            "org_scope".to_string(),
        ];
        inbox
            .add(make_proposal("001"), reasons.clone(), None)
            .unwrap();

        let entry = inbox.get("prp_001").unwrap();
        assert_eq!(entry.review_reasons, reasons);
    }
}
