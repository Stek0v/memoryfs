//! Append-only event log with consumer offsets for at-least-once delivery.
//!
//! Events are durable (fsync per write), identified by monotonic offset.
//! Consumers track their own offset; replaying from a given offset is
//! idempotent by design (consumers deduplicate via `event_id`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{MemoryFsError, Result};
use crate::ids::EventId;

/// Event kinds matching the `event.schema.json` enum subset relevant for the worker bus.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum EventKind {
    CommitCreated,
    CommitReverted,
    FileStaged,
    MemoryProposed,
    MemoryAutoCommitted,
    MemoryReviewRequested,
    MemoryApproved,
    MemoryRejected,
    MemorySuperseded,
    RunStarted,
    RunFinished,
    RedactionApplied,
    PolicyChanged,
    IndexReindexStarted,
    IndexReindexCompleted,
}

/// A single event in the log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct Event {
    pub offset: u64,
    pub event_id: String,
    pub timestamp: DateTime<Utc>,
    pub kind: EventKind,
    pub workspace_id: String,
    pub subject: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

/// Append-only event log backed by an NDJSON file.
pub struct EventLog {
    path: PathBuf,
    file: Mutex<File>,
    next_offset: Mutex<u64>,
}

impl EventLog {
    /// Open or create an event log. Recovers the next offset from existing entries.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MemoryFsError::Internal(anyhow::anyhow!("failed to create event log dir: {e}"))
            })?;
        }

        let next_offset = if path.exists() {
            let entries = read_events(&path)?;
            entries.last().map(|e| e.offset + 1).unwrap_or(0)
        } else {
            0
        };

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| {
                MemoryFsError::Internal(anyhow::anyhow!("failed to open event log: {e}"))
            })?;

        Ok(Self {
            path,
            file: Mutex::new(file),
            next_offset: Mutex::new(next_offset),
        })
    }

    /// Append a new event. Returns the assigned offset.
    pub fn append(
        &self,
        kind: EventKind,
        workspace_id: &str,
        subject: &str,
        target: &str,
        payload: Option<serde_json::Value>,
    ) -> Result<u64> {
        let mut next = self.next_offset.lock().unwrap();
        let offset = *next;

        let event = Event {
            offset,
            event_id: EventId::new().to_string(),
            timestamp: Utc::now(),
            kind,
            workspace_id: workspace_id.to_string(),
            subject: subject.to_string(),
            target: target.to_string(),
            payload,
        };

        let line = serde_json::to_string(&event).map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!("failed to serialize event: {e}"))
        })?;

        let mut file = self.file.lock().unwrap();
        writeln!(file, "{line}")
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("failed to write event: {e}")))?;
        file.sync_all().map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!("failed to fsync event log: {e}"))
        })?;

        *next = offset + 1;
        Ok(offset)
    }

    /// Read events starting from a given offset (inclusive). This is the
    /// consumer pull mechanism for at-least-once delivery.
    pub fn read_from(&self, from_offset: u64) -> Result<Vec<Event>> {
        let events = read_events(&self.path)?;
        Ok(events
            .into_iter()
            .filter(|e| e.offset >= from_offset)
            .collect())
    }

    /// Read all events.
    pub fn read_all(&self) -> Result<Vec<Event>> {
        read_events(&self.path)
    }

    /// Current next offset (i.e. total events written).
    pub fn next_offset(&self) -> u64 {
        *self.next_offset.lock().unwrap()
    }

    /// Path to the event log file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Consumer offset tracker — persists a consumer's position in the event log.
pub struct ConsumerOffset {
    path: PathBuf,
}

impl ConsumerOffset {
    /// Create a consumer offset tracker. The file stores a single u64.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MemoryFsError::Internal(anyhow::anyhow!("failed to create offset dir: {e}"))
            })?;
        }
        Ok(Self { path })
    }

    /// Get the last committed offset, or 0 if none.
    pub fn get(&self) -> Result<u64> {
        if !self.path.exists() {
            return Ok(0);
        }
        let content = std::fs::read_to_string(&self.path).map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!("failed to read consumer offset: {e}"))
        })?;
        content
            .trim()
            .parse::<u64>()
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("invalid consumer offset: {e}")))
    }

    /// Commit a new offset (after successfully processing events up to this offset).
    pub fn commit(&self, offset: u64) -> Result<()> {
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, offset.to_string()).map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!("failed to write consumer offset: {e}"))
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!("failed to commit consumer offset: {e}"))
        })?;
        Ok(())
    }
}

/// Read all events from an NDJSON event log file.
pub fn read_events(path: &Path) -> Result<Vec<Event>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(MemoryFsError::Internal(anyhow::anyhow!(
                "failed to open event log: {e}"
            )))
        }
    };

    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for (i, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!(
                "failed to read event log line {}: {e}",
                i + 1
            ))
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let event: Event = serde_json::from_str(&line).map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!(
                "failed to parse event log line {}: {e}",
                i + 1
            ))
        })?;
        events.push(event);
    }

    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_event_log() -> (tempfile::TempDir, EventLog) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.ndjson");
        let log = EventLog::open(&path).unwrap();
        (dir, log)
    }

    #[test]
    fn append_and_read_all() {
        let (_dir, log) = temp_event_log();

        let off0 = log
            .append(
                EventKind::CommitCreated,
                "ws_TEST",
                "user:alice",
                "commit_abc",
                None,
            )
            .unwrap();
        let off1 = log
            .append(
                EventKind::FileStaged,
                "ws_TEST",
                "user:bob",
                "memory/a.md",
                None,
            )
            .unwrap();

        assert_eq!(off0, 0);
        assert_eq!(off1, 1);
        assert_eq!(log.next_offset(), 2);

        let events = log.read_all().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, EventKind::CommitCreated);
        assert_eq!(events[0].subject, "user:alice");
        assert_eq!(events[1].kind, EventKind::FileStaged);
    }

    #[test]
    fn read_from_offset() {
        let (_dir, log) = temp_event_log();

        for i in 0..5 {
            log.append(
                EventKind::CommitCreated,
                "ws_TEST",
                "user:alice",
                &format!("commit_{i}"),
                None,
            )
            .unwrap();
        }

        let from_3 = log.read_from(3).unwrap();
        assert_eq!(from_3.len(), 2);
        assert_eq!(from_3[0].offset, 3);
        assert_eq!(from_3[1].offset, 4);

        let from_0 = log.read_from(0).unwrap();
        assert_eq!(from_0.len(), 5);

        let from_end = log.read_from(5).unwrap();
        assert!(from_end.is_empty());
    }

    #[test]
    fn ndjson_format() {
        let (_dir, log) = temp_event_log();
        log.append(
            EventKind::MemoryProposed,
            "ws_TEST",
            "agent:bot",
            "memory/m.md",
            None,
        )
        .unwrap();

        let content = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(content.lines().count(), 1);
        let parsed: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["kind"], "memory_proposed");
        assert_eq!(parsed["offset"], 0);
    }

    #[test]
    fn event_ids_are_unique() {
        let (_dir, log) = temp_event_log();
        log.append(EventKind::CommitCreated, "ws_TEST", "user:a", "c1", None)
            .unwrap();
        log.append(EventKind::CommitCreated, "ws_TEST", "user:a", "c2", None)
            .unwrap();

        let events = log.read_all().unwrap();
        assert_ne!(events[0].event_id, events[1].event_id);
    }

    #[test]
    fn payload_roundtrip() {
        let (_dir, log) = temp_event_log();
        let payload = serde_json::json!({"commit_hash": "abc123", "files": 3});

        log.append(
            EventKind::CommitCreated,
            "ws_TEST",
            "user:alice",
            "commit",
            Some(payload.clone()),
        )
        .unwrap();

        let events = log.read_all().unwrap();
        assert_eq!(events[0].payload.as_ref().unwrap()["commit_hash"], "abc123");
        assert_eq!(events[0].payload.as_ref().unwrap()["files"], 3);
    }

    #[test]
    fn recovery_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.ndjson");

        {
            let log = EventLog::open(&path).unwrap();
            log.append(EventKind::CommitCreated, "ws_TEST", "user:a", "c1", None)
                .unwrap();
            log.append(EventKind::FileStaged, "ws_TEST", "user:b", "f1", None)
                .unwrap();
            assert_eq!(log.next_offset(), 2);
        }

        {
            let log = EventLog::open(&path).unwrap();
            assert_eq!(log.next_offset(), 2);

            log.append(
                EventKind::MemoryProposed,
                "ws_TEST",
                "agent:bot",
                "m1",
                None,
            )
            .unwrap();
            assert_eq!(log.next_offset(), 3);

            let events = log.read_all().unwrap();
            assert_eq!(events.len(), 3);
            assert_eq!(events[2].offset, 2);
        }
    }

    #[test]
    fn empty_log_reads_ok() {
        let (_dir, log) = temp_event_log();
        let events = log.read_all().unwrap();
        assert!(events.is_empty());
        assert_eq!(log.next_offset(), 0);
    }

    #[test]
    fn consumer_offset_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("offsets/extraction.offset");

        let consumer = ConsumerOffset::open(&path).unwrap();
        assert_eq!(consumer.get().unwrap(), 0);

        consumer.commit(5).unwrap();
        assert_eq!(consumer.get().unwrap(), 5);

        consumer.commit(10).unwrap();
        assert_eq!(consumer.get().unwrap(), 10);

        // Reopen
        let consumer2 = ConsumerOffset::open(&path).unwrap();
        assert_eq!(consumer2.get().unwrap(), 10);
    }

    #[test]
    fn consumer_reads_from_offset() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("events.ndjson");
        let offset_path = dir.path().join("consumer.offset");

        let log = EventLog::open(&log_path).unwrap();
        for i in 0..10 {
            log.append(
                EventKind::CommitCreated,
                "ws_TEST",
                "user:a",
                &format!("c{i}"),
                None,
            )
            .unwrap();
        }

        let consumer = ConsumerOffset::open(&offset_path).unwrap();
        let offset = consumer.get().unwrap();
        let batch = log.read_from(offset).unwrap();
        assert_eq!(batch.len(), 10);

        // Process first 5, commit offset
        consumer.commit(5).unwrap();

        let batch2 = log.read_from(consumer.get().unwrap()).unwrap();
        assert_eq!(batch2.len(), 5);
        assert_eq!(batch2[0].offset, 5);
    }

    #[test]
    fn idempotent_redelivery() {
        let (_dir, log) = temp_event_log();
        log.append(EventKind::CommitCreated, "ws_TEST", "user:a", "c1", None)
            .unwrap();
        log.append(EventKind::CommitCreated, "ws_TEST", "user:a", "c2", None)
            .unwrap();

        let events = log.read_all().unwrap();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut processed = 0;

        // Simulate at-least-once: process twice
        for _ in 0..2 {
            for event in &events {
                if seen.insert(event.event_id.clone()) {
                    processed += 1;
                }
            }
        }

        assert_eq!(processed, 2);
    }

    #[test]
    fn concurrent_appends_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.ndjson");
        let log = std::sync::Arc::new(EventLog::open(&path).unwrap());

        let mut handles = Vec::new();
        for i in 0..20 {
            let log = log.clone();
            handles.push(std::thread::spawn(move || {
                log.append(
                    EventKind::FileStaged,
                    "ws_TEST",
                    &format!("user:t{i}"),
                    &format!("file{i}.md"),
                    None,
                )
                .unwrap();
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let events = log.read_all().unwrap();
        assert_eq!(events.len(), 20);

        // Offsets should be unique
        let mut offsets: Vec<u64> = events.iter().map(|e| e.offset).collect();
        offsets.sort();
        offsets.dedup();
        assert_eq!(offsets.len(), 20);
    }

    #[test]
    fn all_event_kinds_serialize() {
        let (_dir, log) = temp_event_log();
        let kinds = vec![
            EventKind::CommitCreated,
            EventKind::CommitReverted,
            EventKind::FileStaged,
            EventKind::MemoryProposed,
            EventKind::MemoryAutoCommitted,
            EventKind::MemoryReviewRequested,
            EventKind::MemoryApproved,
            EventKind::MemoryRejected,
            EventKind::MemorySuperseded,
            EventKind::RunStarted,
            EventKind::RunFinished,
            EventKind::RedactionApplied,
            EventKind::PolicyChanged,
            EventKind::IndexReindexStarted,
            EventKind::IndexReindexCompleted,
        ];

        for kind in &kinds {
            log.append(kind.clone(), "ws_TEST", "user:test", "target", None)
                .unwrap();
        }

        let events = log.read_all().unwrap();
        assert_eq!(events.len(), kinds.len());
    }

    #[test]
    fn fsync_durability() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.ndjson");

        let log = EventLog::open(&path).unwrap();
        log.append(EventKind::CommitCreated, "ws_TEST", "user:a", "c1", None)
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.is_empty());
        assert!(content.contains("commit_created"));
    }
}
