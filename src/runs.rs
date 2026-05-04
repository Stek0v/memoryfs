//! Agent run tracking — create, finish, query agent runs.
//!
//! A run represents a single agent invocation: who started it, when, what it
//! produced, and how it ended. Runs are stored in-memory and can be persisted
//! to the workspace as `runs/<run_id>/index.md`.
//!
//! See `specs/schemas/v1/run.schema.json` and `specs/openapi.yaml`.

use std::collections::BTreeMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::{MemoryFsError, Result};
use crate::ids::RunId;

/// Trigger kind — what started this run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    /// Initiated by a user request.
    UserRequest,
    /// Scheduled (cron, timer).
    Scheduled,
    /// Called by another agent.
    AgentCall,
    /// Triggered by a webhook.
    Webhook,
    /// Test invocation.
    Test,
}

/// What initiated a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    /// What kind of trigger.
    pub kind: TriggerKind,
    /// Who triggered (author subject).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Reference to source (cron-id, parent-run-id, webhook-id).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ref")]
    pub trigger_ref: Option<String>,
}

/// Run status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Currently executing.
    Running,
    /// Completed successfully.
    Succeeded,
    /// Completed with error.
    Failed,
    /// Cancelled by user or system.
    Cancelled,
    /// Exceeded time limit.
    Timeout,
}

impl RunStatus {
    /// Whether this status is terminal (not running).
    pub fn is_terminal(&self) -> bool {
        !matches!(self, RunStatus::Running)
    }
}

/// Metrics collected during a run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunMetrics {
    /// Total duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Input tokens consumed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_input: Option<u64>,
    /// Output tokens generated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_output: Option<u64>,
    /// Number of tool calls made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<u64>,
    /// Memories proposed during this run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memories_proposed: Option<u64>,
    /// Memories committed during this run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memories_committed: Option<u64>,
    /// Estimated cost in USD.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// Artifact paths within `runs/<run_id>/`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Artifacts {
    /// Path to the prompt file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Path to tool calls log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<String>,
    /// Path to stdout capture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    /// Path to stderr capture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    /// Path to result file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Path to memory patch file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_patch: Option<String>,
}

/// A single agent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    /// Run ID (`run_` prefix).
    pub id: String,
    /// Agent identifier (`agent:<slug>`).
    pub agent: String,
    /// Optional session grouping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Current status.
    pub status: RunStatus,
    /// When the run started (RFC 3339).
    pub started_at: String,
    /// When the run finished (RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// What triggered this run.
    pub trigger: Trigger,
    /// Author subject (e.g. `user:alice`).
    pub author: String,
    /// LLM model used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tags for filtering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Artifact file paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Artifacts>,
    /// Run metrics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<RunMetrics>,
    /// Proposal IDs created during this run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed_memories: Vec<String>,
    /// Memory IDs consumed during this run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumed_memories: Vec<String>,
    /// File paths read during this run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumed_files: Vec<String>,
    /// Error message (for failed/timeout/cancelled).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Parameters to start a new run.
pub struct StartRunParams {
    /// Agent identifier.
    pub agent: String,
    /// What triggered the run.
    pub trigger: Trigger,
    /// Author subject.
    pub author: String,
    /// Optional session ID.
    pub session_id: Option<String>,
    /// LLM model.
    pub model: Option<String>,
    /// Tags.
    pub tags: Vec<String>,
}

/// Parameters to finish a run.
pub struct FinishRunParams {
    /// Terminal status.
    pub status: RunStatus,
    /// Override finish timestamp.
    pub finished_at: Option<String>,
    /// Artifact paths.
    pub artifacts: Option<Artifacts>,
    /// Run metrics.
    pub metrics: Option<RunMetrics>,
    /// Error message.
    pub error: Option<String>,
    /// Proposal IDs.
    pub proposed_memories: Vec<String>,
}

/// In-memory run store.
pub struct RunStore {
    runs: BTreeMap<String, Run>,
}

impl Default for RunStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RunStore {
    /// Create an empty run store.
    pub fn new() -> Self {
        Self {
            runs: BTreeMap::new(),
        }
    }

    /// Start a new run. Returns the created run.
    pub fn start(&mut self, params: StartRunParams) -> Run {
        let id = RunId::new().to_string();
        let run = Run {
            id: id.clone(),
            agent: params.agent,
            session_id: params.session_id,
            status: RunStatus::Running,
            started_at: Utc::now().to_rfc3339(),
            finished_at: None,
            trigger: params.trigger,
            author: params.author,
            model: params.model,
            tags: params.tags,
            artifacts: None,
            metrics: None,
            proposed_memories: Vec::new(),
            consumed_memories: Vec::new(),
            consumed_files: Vec::new(),
            error: None,
        };
        self.runs.insert(id, run.clone());
        run
    }

    /// Finish a running run. Returns error if not found or already terminal.
    pub fn finish(&mut self, run_id: &str, params: FinishRunParams) -> Result<Run> {
        let run = self
            .runs
            .get_mut(run_id)
            .ok_or_else(|| MemoryFsError::NotFound(format!("run {run_id}")))?;

        if run.status.is_terminal() {
            return Err(MemoryFsError::Validation(format!(
                "run {run_id} already finished with status {:?}",
                run.status
            )));
        }

        run.status = params.status;
        run.finished_at = Some(
            params
                .finished_at
                .unwrap_or_else(|| Utc::now().to_rfc3339()),
        );
        run.artifacts = params.artifacts;
        run.metrics = params.metrics;
        run.error = params.error;
        if !params.proposed_memories.is_empty() {
            run.proposed_memories = params.proposed_memories;
        }

        Ok(run.clone())
    }

    /// Get a run by ID.
    pub fn get(&self, run_id: &str) -> Result<&Run> {
        self.runs
            .get(run_id)
            .ok_or_else(|| MemoryFsError::NotFound(format!("run {run_id}")))
    }

    /// List all runs, most recent first.
    pub fn list(&self, limit: usize) -> Vec<&Run> {
        let mut runs: Vec<&Run> = self.runs.values().collect();
        runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        runs.truncate(limit);
        runs
    }

    /// List runs filtered by agent.
    pub fn list_by_agent(&self, agent: &str, limit: usize) -> Vec<&Run> {
        let mut runs: Vec<&Run> = self.runs.values().filter(|r| r.agent == agent).collect();
        runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        runs.truncate(limit);
        runs
    }

    /// Count of all runs.
    pub fn len(&self) -> usize {
        self.runs.len()
    }

    /// Whether the store has no runs.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Count of currently running runs.
    pub fn active_count(&self) -> usize {
        self.runs
            .values()
            .filter(|r| r.status == RunStatus::Running)
            .count()
    }

    /// Serialize the run to frontmatter markdown for persistence.
    pub fn to_markdown(run: &Run) -> Result<String> {
        let yaml = serde_yaml::to_string(run)
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("serialize run: {e}")))?;
        Ok(format!("---\n{yaml}---\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_trigger() -> Trigger {
        Trigger {
            kind: TriggerKind::Test,
            by: None,
            trigger_ref: None,
        }
    }

    fn test_params() -> StartRunParams {
        StartRunParams {
            agent: "agent:test".into(),
            trigger: test_trigger(),
            author: "user:alice".into(),
            session_id: None,
            model: None,
            tags: vec![],
        }
    }

    #[test]
    fn start_run() {
        let mut store = RunStore::new();
        let run = store.start(test_params());
        assert!(run.id.starts_with("run_"));
        assert_eq!(run.agent, "agent:test");
        assert_eq!(run.status, RunStatus::Running);
        assert_eq!(store.len(), 1);
        assert_eq!(store.active_count(), 1);
    }

    #[test]
    fn finish_run_success() {
        let mut store = RunStore::new();
        let run = store.start(test_params());

        let finished = store
            .finish(
                &run.id,
                FinishRunParams {
                    status: RunStatus::Succeeded,
                    finished_at: None,
                    artifacts: None,
                    metrics: Some(RunMetrics {
                        duration_ms: Some(1234),
                        tokens_input: Some(500),
                        ..Default::default()
                    }),
                    error: None,
                    proposed_memories: vec![],
                },
            )
            .unwrap();

        assert_eq!(finished.status, RunStatus::Succeeded);
        assert!(finished.finished_at.is_some());
        assert_eq!(finished.metrics.as_ref().unwrap().duration_ms, Some(1234));
        assert_eq!(store.active_count(), 0);
    }

    #[test]
    fn finish_run_failed_with_error() {
        let mut store = RunStore::new();
        let run = store.start(test_params());

        let finished = store
            .finish(
                &run.id,
                FinishRunParams {
                    status: RunStatus::Failed,
                    finished_at: None,
                    artifacts: None,
                    metrics: None,
                    error: Some("LLM timeout".into()),
                    proposed_memories: vec![],
                },
            )
            .unwrap();

        assert_eq!(finished.status, RunStatus::Failed);
        assert_eq!(finished.error.as_deref(), Some("LLM timeout"));
    }

    #[test]
    fn cannot_finish_already_terminal() {
        let mut store = RunStore::new();
        let run = store.start(test_params());

        store
            .finish(
                &run.id,
                FinishRunParams {
                    status: RunStatus::Succeeded,
                    finished_at: None,
                    artifacts: None,
                    metrics: None,
                    error: None,
                    proposed_memories: vec![],
                },
            )
            .unwrap();

        let result = store.finish(
            &run.id,
            FinishRunParams {
                status: RunStatus::Failed,
                finished_at: None,
                artifacts: None,
                metrics: None,
                error: None,
                proposed_memories: vec![],
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn finish_nonexistent_run() {
        let mut store = RunStore::new();
        let result = store.finish(
            "run_nonexistent",
            FinishRunParams {
                status: RunStatus::Succeeded,
                finished_at: None,
                artifacts: None,
                metrics: None,
                error: None,
                proposed_memories: vec![],
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn get_run() {
        let mut store = RunStore::new();
        let run = store.start(test_params());
        let fetched = store.get(&run.id).unwrap();
        assert_eq!(fetched.agent, "agent:test");
    }

    #[test]
    fn get_nonexistent() {
        let store = RunStore::new();
        assert!(store.get("run_nope").is_err());
    }

    #[test]
    fn list_runs() {
        let mut store = RunStore::new();
        store.start(test_params());
        store.start(StartRunParams {
            agent: "agent:other".into(),
            ..test_params()
        });
        store.start(test_params());

        assert_eq!(store.list(10).len(), 3);
        assert_eq!(store.list(2).len(), 2);
    }

    #[test]
    fn list_by_agent() {
        let mut store = RunStore::new();
        store.start(test_params());
        store.start(StartRunParams {
            agent: "agent:special".into(),
            ..test_params()
        });
        store.start(test_params());

        let by_test = store.list_by_agent("agent:test", 10);
        assert_eq!(by_test.len(), 2);

        let by_special = store.list_by_agent("agent:special", 10);
        assert_eq!(by_special.len(), 1);
    }

    #[test]
    fn run_to_markdown() {
        let mut store = RunStore::new();
        let run = store.start(test_params());
        let md = RunStore::to_markdown(&run).unwrap();
        assert!(md.starts_with("---\n"));
        assert!(
            md.contains("agent: 'agent:test'")
                || md.contains("agent: \"agent:test\"")
                || md.contains("agent: agent:test")
        );
        assert!(md.contains("status: running") || md.contains("status: Running"));
    }

    #[test]
    fn run_with_all_fields() {
        let mut store = RunStore::new();
        let run = store.start(StartRunParams {
            agent: "agent:claude".into(),
            trigger: Trigger {
                kind: TriggerKind::UserRequest,
                by: Some("user:bob".into()),
                trigger_ref: Some("session_123".into()),
            },
            author: "user:bob".into(),
            session_id: Some("sess_abc".into()),
            model: Some("anthropic/claude-opus-4-7".into()),
            tags: vec!["production".into(), "critical".into()],
        });

        let finished = store
            .finish(
                &run.id,
                FinishRunParams {
                    status: RunStatus::Succeeded,
                    finished_at: None,
                    artifacts: Some(Artifacts {
                        prompt: Some(format!("runs/{}/prompt.md", run.id)),
                        result: Some(format!("runs/{}/result.md", run.id)),
                        ..Default::default()
                    }),
                    metrics: Some(RunMetrics {
                        duration_ms: Some(5000),
                        tokens_input: Some(2000),
                        tokens_output: Some(800),
                        tool_calls: Some(3),
                        memories_proposed: Some(2),
                        memories_committed: Some(1),
                        cost_usd: Some(0.05),
                    }),
                    error: None,
                    proposed_memories: vec!["prop_abc".into()],
                },
            )
            .unwrap();

        assert_eq!(finished.model.as_deref(), Some("anthropic/claude-opus-4-7"));
        assert_eq!(finished.tags.len(), 2);
        assert!(finished.artifacts.is_some());
        assert_eq!(finished.proposed_memories.len(), 1);
    }

    #[test]
    fn trigger_kinds_serialize() {
        let kinds = vec![
            TriggerKind::UserRequest,
            TriggerKind::Scheduled,
            TriggerKind::AgentCall,
            TriggerKind::Webhook,
            TriggerKind::Test,
        ];
        for kind in kinds {
            let json = serde_json::to_string(&kind).unwrap();
            let restored: TriggerKind = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, kind);
        }
    }

    #[test]
    fn status_is_terminal() {
        assert!(!RunStatus::Running.is_terminal());
        assert!(RunStatus::Succeeded.is_terminal());
        assert!(RunStatus::Failed.is_terminal());
        assert!(RunStatus::Cancelled.is_terminal());
        assert!(RunStatus::Timeout.is_terminal());
    }
}
