//! Append-only audit log — NDJSON format with optional hash chain.
//!
//! Every write, commit, revert, and review event is recorded. Entries are
//! fsynced individually to survive kill -9. When `tamper_evident` is enabled,
//! each entry includes the SHA-256 hash of the previous entry, forming a
//! hash chain that detects tampering.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{MemoryFsError, Result};

/// A single audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// ISO-8601 timestamp.
    pub timestamp: DateTime<Utc>,
    /// Who performed the action (e.g. `user:alice`, `agent:bot`).
    pub subject: String,
    /// What action was performed.
    pub action: AuditAction,
    /// Target path or resource identifier.
    pub target: String,
    /// Additional details (commit hash, reason, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// SHA-256 hash of the previous entry (when tamper_evident is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_hash: Option<String>,
    /// SHA-256 hash of this entry (set after serialization).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_hash: Option<String>,
}

/// Actions recorded in the audit log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// File written to staging.
    FileWrite,
    /// File read.
    FileRead,
    /// Commit created.
    Commit,
    /// Revert performed.
    Revert,
    /// Review decision made.
    Review,
    /// Memory proposed.
    MemoryPropose,
    /// Memory superseded.
    MemorySupersede,
    /// ACL check denied.
    AclDenied,
    /// Redaction triggered.
    RedactionTriggered,
}

/// Append-only audit log writer.
pub struct AuditLog {
    path: PathBuf,
    file: Mutex<File>,
    tamper_evident: bool,
    last_hash: Mutex<Option<String>>,
    fsync_per_event: bool,
}

impl AuditLog {
    /// Open or create an audit log file.
    pub fn open(
        path: impl Into<PathBuf>,
        tamper_evident: bool,
        fsync_per_event: bool,
    ) -> Result<Self> {
        let path = path.into();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MemoryFsError::Internal(anyhow::anyhow!("failed to create audit log dir: {e}"))
            })?;
        }

        let last_hash = if tamper_evident && path.exists() {
            recover_last_hash(&path)?
        } else {
            None
        };

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| {
                MemoryFsError::Internal(anyhow::anyhow!("failed to open audit log: {e}"))
            })?;

        Ok(Self {
            path,
            file: Mutex::new(file),
            tamper_evident,
            last_hash: Mutex::new(last_hash),
            fsync_per_event,
        })
    }

    /// Append an event to the log.
    pub fn record(
        &self,
        subject: &str,
        action: AuditAction,
        target: &str,
        details: Option<serde_json::Value>,
    ) -> Result<()> {
        let mut entry = AuditEntry {
            timestamp: Utc::now(),
            subject: subject.to_string(),
            action,
            target: target.to_string(),
            details,
            prev_hash: None,
            entry_hash: None,
        };

        if self.tamper_evident {
            let mut last = self.last_hash.lock().unwrap();
            entry.prev_hash = last.clone();

            let hash = compute_entry_hash(&entry);
            entry.entry_hash = Some(hash.clone());
            *last = Some(hash);
        }

        let line = serde_json::to_string(&entry).map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!("failed to serialize audit entry: {e}"))
        })?;

        let mut file = self.file.lock().unwrap();
        writeln!(file, "{line}").map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!("failed to write audit entry: {e}"))
        })?;

        if self.fsync_per_event {
            file.sync_all().map_err(|e| {
                MemoryFsError::Internal(anyhow::anyhow!("failed to fsync audit log: {e}"))
            })?;
        }

        Ok(())
    }

    /// Read all entries from the log.
    pub fn read_all(&self) -> Result<Vec<AuditEntry>> {
        read_entries(&self.path)
    }

    /// Verify the hash chain integrity (tamper_evident mode only).
    pub fn verify_chain(&self) -> Result<()> {
        let entries = self.read_all()?;
        verify_hash_chain(&entries)
    }

    /// Path to the audit log file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Read entries from an audit log file.
pub fn read_entries(path: &Path) -> Result<Vec<AuditEntry>> {
    let file = File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => MemoryFsError::NotFound("audit log".into()),
        _ => MemoryFsError::Internal(anyhow::anyhow!("failed to open audit log: {e}")),
    })?;

    let reader = BufReader::new(file);
    let mut entries = Vec::new();

    for (i, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!(
                "failed to read audit log line {}: {e}",
                i + 1
            ))
        })?;

        if line.trim().is_empty() {
            continue;
        }

        let entry: AuditEntry = serde_json::from_str(&line).map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!(
                "failed to parse audit log line {}: {e}",
                i + 1
            ))
        })?;
        entries.push(entry);
    }

    Ok(entries)
}

/// Verify the hash chain of entries.
pub fn verify_hash_chain(entries: &[AuditEntry]) -> Result<()> {
    let mut expected_prev: Option<String> = None;

    for (i, entry) in entries.iter().enumerate() {
        if entry.entry_hash.is_none() {
            continue;
        }

        if entry.prev_hash != expected_prev {
            return Err(MemoryFsError::Conflict(format!(
                "audit log hash chain broken at entry {}: expected prev_hash {:?}, got {:?}",
                i, expected_prev, entry.prev_hash
            )));
        }

        let computed = compute_entry_hash(entry);
        if entry.entry_hash.as_deref() != Some(&computed) {
            return Err(MemoryFsError::Conflict(format!(
                "audit log entry {} hash mismatch: stored {:?}, computed {computed}",
                i, entry.entry_hash
            )));
        }

        expected_prev = entry.entry_hash.clone();
    }

    Ok(())
}

fn compute_entry_hash(entry: &AuditEntry) -> String {
    let mut hasher = Sha256::new();
    hasher.update(entry.timestamp.to_rfc3339().as_bytes());
    hasher.update(b"\0");
    hasher.update(entry.subject.as_bytes());
    hasher.update(b"\0");
    hasher.update(
        serde_json::to_string(&entry.action)
            .unwrap_or_default()
            .as_bytes(),
    );
    hasher.update(b"\0");
    hasher.update(entry.target.as_bytes());
    hasher.update(b"\0");
    if let Some(ref details) = entry.details {
        hasher.update(details.to_string().as_bytes());
    }
    hasher.update(b"\0");
    if let Some(ref prev) = entry.prev_hash {
        hasher.update(prev.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn recover_last_hash(path: &Path) -> Result<Option<String>> {
    let entries = read_entries(path)?;
    Ok(entries.last().and_then(|e| e.entry_hash.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log(tamper_evident: bool) -> (tempfile::TempDir, AuditLog) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.ndjson");
        let log = AuditLog::open(&path, tamper_evident, true).unwrap();
        (dir, log)
    }

    #[test]
    fn record_and_read_back() {
        let (_dir, log) = temp_log(false);
        log.record("user:alice", AuditAction::Commit, "memory/a.md", None)
            .unwrap();
        log.record(
            "agent:bot",
            AuditAction::FileWrite,
            "runs/r.md",
            Some(serde_json::json!({"bytes": 1024})),
        )
        .unwrap();

        let entries = log.read_all().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].subject, "user:alice");
        assert_eq!(entries[0].action, AuditAction::Commit);
        assert_eq!(entries[1].action, AuditAction::FileWrite);
        assert!(entries[1].details.is_some());
    }

    #[test]
    fn ndjson_format() {
        let (_dir, log) = temp_log(false);
        log.record("user:alice", AuditAction::Revert, "ws", None)
            .unwrap();

        let content = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(content.lines().count(), 1);
        let parsed: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["action"], "revert");
    }

    #[test]
    fn tamper_evident_hash_chain() {
        let (_dir, log) = temp_log(true);
        log.record("user:alice", AuditAction::Commit, "a.md", None)
            .unwrap();
        log.record("user:bob", AuditAction::FileWrite, "b.md", None)
            .unwrap();
        log.record("agent:bot", AuditAction::Review, "c.md", None)
            .unwrap();

        let entries = log.read_all().unwrap();
        assert_eq!(entries.len(), 3);

        assert!(entries[0].prev_hash.is_none());
        assert!(entries[0].entry_hash.is_some());

        assert_eq!(entries[1].prev_hash, entries[0].entry_hash);
        assert_eq!(entries[2].prev_hash, entries[1].entry_hash);

        log.verify_chain().unwrap();
    }

    #[test]
    fn tamper_detection() {
        let (_dir, log) = temp_log(true);
        log.record("user:alice", AuditAction::Commit, "a.md", None)
            .unwrap();
        log.record("user:bob", AuditAction::Commit, "b.md", None)
            .unwrap();

        // Tamper with the file
        let content = std::fs::read_to_string(log.path()).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        let mut entry: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        entry["subject"] = serde_json::Value::String("user:evil".into());
        let tampered = format!("{}\n{}\n", entry, lines[1]);
        std::fs::write(log.path(), tampered).unwrap();

        let entries = read_entries(log.path()).unwrap();
        let err = verify_hash_chain(&entries).unwrap_err();
        assert_eq!(err.api_code(), "CONFLICT");
    }

    #[test]
    fn recovery_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.ndjson");

        {
            let log = AuditLog::open(&path, true, true).unwrap();
            log.record("user:alice", AuditAction::Commit, "a.md", None)
                .unwrap();
            log.record("user:bob", AuditAction::FileWrite, "b.md", None)
                .unwrap();
        }

        {
            let log = AuditLog::open(&path, true, true).unwrap();
            log.record("user:charlie", AuditAction::Revert, "c.md", None)
                .unwrap();
            log.verify_chain().unwrap();

            let entries = log.read_all().unwrap();
            assert_eq!(entries.len(), 3);
            assert_eq!(entries[2].prev_hash, entries[1].entry_hash);
        }
    }

    #[test]
    fn empty_log_reads_ok() {
        let (_dir, log) = temp_log(false);
        let entries = log.read_all().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn all_action_types() {
        let (_dir, log) = temp_log(false);
        let actions = vec![
            AuditAction::FileWrite,
            AuditAction::FileRead,
            AuditAction::Commit,
            AuditAction::Revert,
            AuditAction::Review,
            AuditAction::MemoryPropose,
            AuditAction::MemorySupersede,
            AuditAction::AclDenied,
            AuditAction::RedactionTriggered,
        ];

        for action in &actions {
            log.record("user:test", action.clone(), "target", None)
                .unwrap();
        }

        let entries = log.read_all().unwrap();
        assert_eq!(entries.len(), 9);
    }

    #[test]
    fn details_roundtrip() {
        let (_dir, log) = temp_log(false);
        let details = serde_json::json!({
            "commit_hash": "abc123",
            "files_changed": 3,
            "reason": "rollback due to bug"
        });
        log.record(
            "user:alice",
            AuditAction::Revert,
            "workspace",
            Some(details.clone()),
        )
        .unwrap();

        let entries = log.read_all().unwrap();
        assert_eq!(
            entries[0].details.as_ref().unwrap()["commit_hash"],
            "abc123"
        );
        assert_eq!(entries[0].details.as_ref().unwrap()["files_changed"], 3);
    }

    #[test]
    fn fsync_creates_durable_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.ndjson");

        let log = AuditLog::open(&path, false, true).unwrap();
        log.record("user:alice", AuditAction::Commit, "a.md", None)
            .unwrap();

        // File should exist and have content immediately (fsynced)
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.is_empty());
        assert!(content.contains("commit"));
    }

    #[test]
    fn concurrent_writes_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.ndjson");
        let log = std::sync::Arc::new(AuditLog::open(&path, false, true).unwrap());

        let mut handles = Vec::new();
        for i in 0..10 {
            let log = log.clone();
            handles.push(std::thread::spawn(move || {
                log.record(
                    &format!("user:thread{i}"),
                    AuditAction::FileWrite,
                    &format!("file{i}.md"),
                    None,
                )
                .unwrap();
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let entries = log.read_all().unwrap();
        assert_eq!(entries.len(), 10);
    }

    #[test]
    fn verify_empty_chain_ok() {
        verify_hash_chain(&[]).unwrap();
    }

    #[test]
    fn non_tamper_evident_entries_skip_verification() {
        let (_dir, log) = temp_log(false);
        log.record("user:alice", AuditAction::Commit, "a.md", None)
            .unwrap();

        let entries = log.read_all().unwrap();
        assert!(entries[0].entry_hash.is_none());

        // verify_chain should succeed (skips entries without hashes)
        verify_hash_chain(&entries).unwrap();
    }

    #[test]
    fn hash_chain_broken_in_middle() {
        let (_dir, log) = temp_log(true);
        log.record("user:a", AuditAction::Commit, "a.md", None)
            .unwrap();
        log.record("user:b", AuditAction::Commit, "b.md", None)
            .unwrap();
        log.record("user:c", AuditAction::Commit, "c.md", None)
            .unwrap();

        // Remove the middle entry
        let content = std::fs::read_to_string(log.path()).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        let tampered = format!("{}\n{}\n", lines[0], lines[2]);
        std::fs::write(log.path(), tampered).unwrap();

        let entries = read_entries(log.path()).unwrap();
        let err = verify_hash_chain(&entries).unwrap_err();
        assert_eq!(err.api_code(), "CONFLICT");
    }
}
