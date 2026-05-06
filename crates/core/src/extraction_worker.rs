//! Extraction worker — consumes events from the event log, calls the LLM
//! to extract memories, validates the output, and emits proposals.
//!
//! Designed for at-least-once delivery: idempotent via `event_id` dedup.
//! Retries on LLM failure with exponential backoff. Gracefully degrades
//! when the LLM is unavailable.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use crate::error::{MemoryFsError, Result};
use crate::event_log::{ConsumerOffset, EventKind, EventLog};
use crate::extraction::{self, ValidatedProposal};
use crate::llm::{LlmClient, Message, Role};

/// Configuration for the extraction worker.
pub struct WorkerConfig {
    /// Maximum retries per event on LLM failure.
    pub max_retries: u32,
    /// Base delay for exponential backoff (doubled each retry).
    pub retry_base_delay: Duration,
    /// Maximum batch size when polling events.
    pub batch_size: usize,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_base_delay: Duration::from_millis(500),
            batch_size: 10,
        }
    }
}

/// Result of processing a single event.
#[derive(Debug)]
pub enum ProcessResult {
    /// Successfully extracted proposals.
    Extracted(Vec<ValidatedProposal>),
    /// No proposals extracted (valid empty result).
    Empty,
    /// Event kind is not relevant for extraction — skipped.
    Skipped,
    /// LLM call failed after all retries.
    Failed(String),
}

/// Extraction worker that polls the event log and produces proposals.
pub struct ExtractionWorker {
    event_log: Arc<EventLog>,
    consumer: ConsumerOffset,
    llm: Arc<dyn LlmClient>,
    config: WorkerConfig,
    seen_events: HashSet<String>,
}

impl ExtractionWorker {
    /// Create a new extraction worker.
    pub fn new(
        event_log: Arc<EventLog>,
        consumer: ConsumerOffset,
        llm: Arc<dyn LlmClient>,
        config: WorkerConfig,
    ) -> Self {
        Self {
            event_log,
            consumer,
            llm,
            config,
            seen_events: HashSet::new(),
        }
    }

    /// Poll for new events, process them, and return proposals.
    /// Commits the consumer offset after successful processing.
    pub async fn poll(&mut self) -> Result<Vec<ValidatedProposal>> {
        let from_offset = self.consumer.get()?;
        let events = self.event_log.read_from(from_offset)?;

        if events.is_empty() {
            return Ok(Vec::new());
        }

        let batch: Vec<_> = events.into_iter().take(self.config.batch_size).collect();

        let mut all_proposals = Vec::new();
        let mut max_offset = from_offset;

        for event in &batch {
            if self.seen_events.contains(&event.event_id) {
                max_offset = max_offset.max(event.offset + 1);
                continue;
            }

            let result = self.process_event(event).await;
            self.seen_events.insert(event.event_id.clone());

            match result {
                ProcessResult::Extracted(proposals) => {
                    all_proposals.extend(proposals);
                }
                ProcessResult::Empty | ProcessResult::Skipped => {}
                ProcessResult::Failed(err) => {
                    tracing::warn!(
                        event_id = %event.event_id,
                        error = %err,
                        "extraction failed after retries"
                    );
                }
            }

            max_offset = max_offset.max(event.offset + 1);
        }

        self.consumer.commit(max_offset)?;

        Ok(all_proposals)
    }

    async fn process_event(&self, event: &crate::event_log::Event) -> ProcessResult {
        match event.kind {
            EventKind::CommitCreated | EventKind::RunFinished => {}
            _ => return ProcessResult::Skipped,
        }

        let conversation = match &event.payload {
            Some(payload) => {
                if let Some(conv) = payload.get("conversation").and_then(|v| v.as_str()) {
                    conv.to_string()
                } else {
                    return ProcessResult::Skipped;
                }
            }
            None => return ProcessResult::Skipped,
        };

        match self.call_llm_with_retry(&conversation).await {
            Ok(proposals) => {
                if proposals.is_empty() {
                    ProcessResult::Empty
                } else {
                    ProcessResult::Extracted(proposals)
                }
            }
            Err(e) => ProcessResult::Failed(e.to_string()),
        }
    }

    async fn call_llm_with_retry(&self, conversation: &str) -> Result<Vec<ValidatedProposal>> {
        let prompt = extraction::build_extraction_prompt(conversation);
        let messages = vec![
            Message {
                role: Role::System,
                content:
                    "You are a memory extraction system. Return a JSON array of memory proposals."
                        .to_string(),
            },
            Message {
                role: Role::User,
                content: prompt,
            },
        ];

        let mut last_error = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                let delay = self.config.retry_base_delay * 2u32.pow(attempt - 1);
                tokio::time::sleep(delay).await;
            }

            match self.llm.chat(&messages, None).await {
                Ok(response) => {
                    let cleaned = clean_llm_response(&response);
                    match extraction::parse_extraction_output_strict(&cleaned) {
                        Ok(proposals) => return Ok(proposals),
                        Err(e) => {
                            last_error = Some(e);
                            continue;
                        }
                    }
                }
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            MemoryFsError::Internal(anyhow::anyhow!("extraction failed with no error"))
        }))
    }
}

/// Strip markdown code fences and leading/trailing whitespace from LLM output.
fn clean_llm_response(response: &str) -> String {
    let trimmed = response.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        if let Some(content) = rest.strip_suffix("```") {
            return content.trim().to_string();
        }
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        if let Some(content) = rest.strip_suffix("```") {
            return content.trim().to_string();
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_log::EventKind;
    use std::sync::Mutex;

    struct MockLlm {
        responses: Mutex<Vec<std::result::Result<String, String>>>,
        call_count: Mutex<u32>,
    }

    impl MockLlm {
        fn new(responses: Vec<std::result::Result<String, String>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                call_count: Mutex::new(0),
            }
        }

        fn call_count(&self) -> u32 {
            *self.call_count.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for MockLlm {
        async fn chat(
            &self,
            _messages: &[Message],
            _json_schema: Option<&serde_json::Value>,
        ) -> Result<String> {
            let mut count = self.call_count.lock().unwrap();
            *count += 1;

            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                return Err(MemoryFsError::Internal(anyhow::anyhow!(
                    "no more responses"
                )));
            }
            match responses.remove(0) {
                Ok(s) => Ok(s),
                Err(e) => Err(MemoryFsError::Internal(anyhow::anyhow!("{e}"))),
            }
        }

        fn model_id(&self) -> &str {
            "mock-model"
        }
    }

    fn setup() -> (tempfile::TempDir, Arc<EventLog>, ConsumerOffset) {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(EventLog::open(dir.path().join("events.ndjson")).unwrap());
        let consumer = ConsumerOffset::open(dir.path().join("consumer.offset")).unwrap();
        (dir, log, consumer)
    }

    fn valid_json_response() -> String {
        r#"[{"memory_type":"fact","scope":"user","scope_id":"user:alice","sensitivity":"normal","confidence":0.9,"title":"Test","body":"Test body"}]"#.to_string()
    }

    #[tokio::test]
    async fn extracts_from_commit_event() {
        let (_dir, log, consumer) = setup();
        let llm = Arc::new(MockLlm::new(vec![Ok(valid_json_response())]));

        log.append(
            EventKind::CommitCreated,
            "ws_TEST",
            "user:alice",
            "commit",
            Some(serde_json::json!({"conversation": "user: I like Rust\nassistant: noted!"})),
        )
        .unwrap();

        let mut worker = ExtractionWorker::new(log, consumer, llm.clone(), WorkerConfig::default());
        let proposals = worker.poll().await.unwrap();

        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].title, "Test");
        assert_eq!(llm.call_count(), 1);
    }

    #[tokio::test]
    async fn skips_irrelevant_events() {
        let (_dir, log, consumer) = setup();
        let llm = Arc::new(MockLlm::new(vec![]));

        log.append(EventKind::FileStaged, "ws_TEST", "user:a", "f.md", None)
            .unwrap();
        log.append(
            EventKind::PolicyChanged,
            "ws_TEST",
            "system:core",
            "policy",
            None,
        )
        .unwrap();

        let mut worker = ExtractionWorker::new(log, consumer, llm.clone(), WorkerConfig::default());
        let proposals = worker.poll().await.unwrap();

        assert!(proposals.is_empty());
        assert_eq!(llm.call_count(), 0);
    }

    #[tokio::test]
    async fn skips_events_without_conversation() {
        let (_dir, log, consumer) = setup();
        let llm = Arc::new(MockLlm::new(vec![]));

        log.append(
            EventKind::CommitCreated,
            "ws_TEST",
            "user:a",
            "commit",
            Some(serde_json::json!({"files": 3})),
        )
        .unwrap();

        let mut worker = ExtractionWorker::new(log, consumer, llm.clone(), WorkerConfig::default());
        let proposals = worker.poll().await.unwrap();

        assert!(proposals.is_empty());
        assert_eq!(llm.call_count(), 0);
    }

    #[tokio::test]
    async fn retries_on_llm_failure() {
        let (_dir, log, consumer) = setup();
        let llm = Arc::new(MockLlm::new(vec![
            Err("timeout".to_string()),
            Err("rate limited".to_string()),
            Ok(valid_json_response()),
        ]));

        log.append(
            EventKind::CommitCreated,
            "ws_TEST",
            "user:a",
            "c",
            Some(serde_json::json!({"conversation": "user: hi"})),
        )
        .unwrap();

        let config = WorkerConfig {
            max_retries: 3,
            retry_base_delay: Duration::from_millis(1),
            batch_size: 10,
        };

        let mut worker = ExtractionWorker::new(log, consumer, llm.clone(), config);
        let proposals = worker.poll().await.unwrap();

        assert_eq!(proposals.len(), 1);
        assert_eq!(llm.call_count(), 3);
    }

    #[tokio::test]
    async fn retries_on_invalid_json_then_succeeds() {
        let (_dir, log, consumer) = setup();
        let llm = Arc::new(MockLlm::new(vec![
            Ok("not json".to_string()),
            Ok(valid_json_response()),
        ]));

        log.append(
            EventKind::CommitCreated,
            "ws_TEST",
            "user:a",
            "c",
            Some(serde_json::json!({"conversation": "user: test"})),
        )
        .unwrap();

        let config = WorkerConfig {
            max_retries: 3,
            retry_base_delay: Duration::from_millis(1),
            batch_size: 10,
        };

        let mut worker = ExtractionWorker::new(log, consumer, llm.clone(), config);
        let proposals = worker.poll().await.unwrap();

        assert_eq!(proposals.len(), 1);
        assert_eq!(llm.call_count(), 2);
    }

    #[tokio::test]
    async fn fails_after_max_retries() {
        let (_dir, log, consumer) = setup();
        let llm = Arc::new(MockLlm::new(vec![
            Err("fail1".to_string()),
            Err("fail2".to_string()),
            Err("fail3".to_string()),
            Err("fail4".to_string()),
        ]));

        log.append(
            EventKind::CommitCreated,
            "ws_TEST",
            "user:a",
            "c",
            Some(serde_json::json!({"conversation": "user: test"})),
        )
        .unwrap();

        let config = WorkerConfig {
            max_retries: 3,
            retry_base_delay: Duration::from_millis(1),
            batch_size: 10,
        };

        let mut worker = ExtractionWorker::new(log, consumer, llm.clone(), config);
        let proposals = worker.poll().await.unwrap();

        assert!(proposals.is_empty());
        assert_eq!(llm.call_count(), 4);
    }

    #[tokio::test]
    async fn deduplicates_events() {
        let (_dir, log, consumer) = setup();

        log.append(
            EventKind::CommitCreated,
            "ws_TEST",
            "user:a",
            "c",
            Some(serde_json::json!({"conversation": "user: test"})),
        )
        .unwrap();

        let llm = Arc::new(MockLlm::new(vec![
            Ok(valid_json_response()),
            Ok(valid_json_response()),
        ]));

        let config = WorkerConfig {
            max_retries: 0,
            retry_base_delay: Duration::from_millis(1),
            batch_size: 10,
        };

        let mut worker = ExtractionWorker::new(log.clone(), consumer, llm.clone(), config);

        let proposals1 = worker.poll().await.unwrap();
        assert_eq!(proposals1.len(), 1);

        // Reset consumer offset to replay
        worker.consumer.commit(0).unwrap();
        let proposals2 = worker.poll().await.unwrap();
        assert!(proposals2.is_empty());

        // Only called once due to dedup
        assert_eq!(llm.call_count(), 1);
    }

    #[tokio::test]
    async fn consumer_offset_advances() {
        let (_dir, log, consumer) = setup();

        for i in 0..5 {
            log.append(
                EventKind::CommitCreated,
                "ws_TEST",
                "user:a",
                &format!("c{i}"),
                Some(serde_json::json!({"conversation": format!("user: msg {i}")})),
            )
            .unwrap();
        }

        let responses: Vec<_> = (0..5).map(|_| Ok(valid_json_response())).collect();
        let llm = Arc::new(MockLlm::new(responses));

        let config = WorkerConfig {
            max_retries: 0,
            retry_base_delay: Duration::from_millis(1),
            batch_size: 3,
        };

        let mut worker = ExtractionWorker::new(log, consumer, llm, config);

        let batch1 = worker.poll().await.unwrap();
        assert_eq!(batch1.len(), 3);
        assert_eq!(worker.consumer.get().unwrap(), 3);

        let batch2 = worker.poll().await.unwrap();
        assert_eq!(batch2.len(), 2);
        assert_eq!(worker.consumer.get().unwrap(), 5);

        let batch3 = worker.poll().await.unwrap();
        assert!(batch3.is_empty());
    }

    #[tokio::test]
    async fn empty_log_returns_empty() {
        let (_dir, log, consumer) = setup();
        let llm = Arc::new(MockLlm::new(vec![]));

        let mut worker = ExtractionWorker::new(log, consumer, llm, WorkerConfig::default());
        let proposals = worker.poll().await.unwrap();
        assert!(proposals.is_empty());
    }

    #[test]
    fn clean_response_strips_code_fences() {
        assert_eq!(clean_llm_response("```json\n[]\n```"), "[]");
        assert_eq!(clean_llm_response("```\n[]\n```"), "[]");
        assert_eq!(clean_llm_response("  []  "), "[]");
        assert_eq!(clean_llm_response("[]"), "[]");
    }

    #[test]
    fn clean_response_handles_nested_content() {
        let input = r#"```json
[{"memory_type": "fact"}]
```"#;
        let cleaned = clean_llm_response(input);
        assert!(cleaned.starts_with('['));
        assert!(cleaned.ends_with(']'));
    }
}
