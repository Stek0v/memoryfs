//! Git-style commit graph without a git dependency.
//!
//! Each commit records:
//! - SHA-256 hash (computed from parent + author + timestamp + file snapshot)
//! - Parent commit hash (None for the initial commit)
//! - Author (subject string)
//! - Timestamp
//! - Message
//! - Snapshot of the inode index at that point
//!
//! The commit graph supports `log`, `diff`, and `revert`.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use crate::error::{MemoryFsError, Result};

/// A single commit in the history.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Commit {
    /// SHA-256 hex hash of this commit.
    pub hash: String,
    /// Parent commit hash (None for the initial commit).
    pub parent: Option<String>,
    /// Who made this commit (e.g. `user:alice`, `agent:bot`).
    pub author: String,
    /// When the commit was created.
    pub timestamp: DateTime<Utc>,
    /// Human-readable commit message.
    pub message: String,
    /// Snapshot: path → object hash at this commit.
    pub snapshot: BTreeMap<String, String>,
}

impl Commit {
    fn compute_hash(
        parent: Option<&str>,
        author: &str,
        timestamp: &DateTime<Utc>,
        message: &str,
        snapshot: &BTreeMap<String, String>,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(parent.unwrap_or("null").as_bytes());
        hasher.update(b"\0");
        hasher.update(author.as_bytes());
        hasher.update(b"\0");
        hasher.update(timestamp.to_rfc3339().as_bytes());
        hasher.update(b"\0");
        hasher.update(message.as_bytes());
        hasher.update(b"\0");
        for (path, hash) in snapshot {
            hasher.update(path.as_bytes());
            hasher.update(b":");
            hasher.update(hash.as_bytes());
            hasher.update(b"\n");
        }
        hex::encode(hasher.finalize())
    }
}

/// A diff entry between two commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffEntry {
    /// File was added.
    Added {
        /// Workspace-relative path.
        path: String,
        /// Object hash of the new file.
        hash: String,
    },
    /// File was removed.
    Removed {
        /// Workspace-relative path.
        path: String,
        /// Object hash of the removed file.
        hash: String,
    },
    /// File was modified.
    Modified {
        /// Workspace-relative path.
        path: String,
        /// Object hash before the change.
        old_hash: String,
        /// Object hash after the change.
        new_hash: String,
    },
}

/// Linear commit graph stored as a Vec (most recent last).
pub struct CommitGraph {
    commits: Vec<Commit>,
}

impl CommitGraph {
    /// Create an empty commit graph.
    pub fn new() -> Self {
        Self {
            commits: Vec::new(),
        }
    }

    /// Load from a serialized JSON array.
    pub fn from_json(json: &str) -> Result<Self> {
        let commits: Vec<Commit> = serde_json::from_str(json).map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!("failed to parse commit graph: {e}"))
        })?;
        Ok(Self { commits })
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.commits).map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!("failed to serialize commit graph: {e}"))
        })
    }

    /// Create a new commit from the current inode index state.
    ///
    /// Returns `PreconditionFailed` if `expected_parent` doesn't match HEAD.
    pub fn commit(
        &mut self,
        author: &str,
        message: &str,
        snapshot: BTreeMap<String, String>,
        expected_parent: Option<&str>,
    ) -> Result<&Commit> {
        let actual_parent = self.head().map(|c| c.hash.as_str());

        match (expected_parent, actual_parent) {
            (Some(exp), Some(act)) if exp != act => {
                return Err(MemoryFsError::PreconditionFailed);
            }
            (Some(_), None) => {
                return Err(MemoryFsError::PreconditionFailed);
            }
            (None, Some(_)) => {
                return Err(MemoryFsError::PreconditionFailed);
            }
            _ => {}
        }

        let timestamp = Utc::now();
        let parent_str = actual_parent.map(|s| s.to_string());
        let hash = Commit::compute_hash(
            parent_str.as_deref(),
            author,
            &timestamp,
            message,
            &snapshot,
        );

        let commit = Commit {
            hash,
            parent: parent_str,
            author: author.to_string(),
            timestamp,
            message: message.to_string(),
            snapshot,
        };

        self.commits.push(commit);
        Ok(self.commits.last().unwrap())
    }

    /// The most recent commit, if any.
    pub fn head(&self) -> Option<&Commit> {
        self.commits.last()
    }

    /// Find a commit by hash.
    pub fn get(&self, hash: &str) -> Option<&Commit> {
        self.commits.iter().find(|c| c.hash == hash)
    }

    /// Return the log of commits in reverse chronological order.
    pub fn log(&self, limit: Option<usize>) -> Vec<&Commit> {
        let iter = self.commits.iter().rev();
        match limit {
            Some(n) => iter.take(n).collect(),
            None => iter.collect(),
        }
    }

    /// Compute the diff between two commits (or from empty to a commit).
    pub fn diff(&self, from: Option<&str>, to: &str) -> Result<Vec<DiffEntry>> {
        let to_commit = self
            .get(to)
            .ok_or_else(|| MemoryFsError::NotFound(format!("commit {to}")))?;

        let old_snap: &BTreeMap<String, String> = match from {
            Some(hash) => {
                let c = self
                    .get(hash)
                    .ok_or_else(|| MemoryFsError::NotFound(format!("commit {hash}")))?;
                &c.snapshot
            }
            None => &BTreeMap::new(),
        };

        Ok(compute_diff(old_snap, &to_commit.snapshot))
    }

    /// Create a revert commit that restores the state of `target_hash`.
    pub fn revert(&mut self, target_hash: &str, author: &str) -> Result<&Commit> {
        let target = self
            .get(target_hash)
            .ok_or_else(|| MemoryFsError::NotFound(format!("commit {target_hash}")))?
            .clone();

        let head_hash = self
            .head()
            .ok_or_else(|| MemoryFsError::Conflict("cannot revert: no commits".into()))?
            .hash
            .clone();

        let message = format!("revert to {}", &target_hash[..12]);

        self.commit(author, &message, target.snapshot.clone(), Some(&head_hash))
    }

    /// Number of commits.
    pub fn len(&self) -> usize {
        self.commits.len()
    }

    /// Whether the graph is empty.
    pub fn is_empty(&self) -> bool {
        self.commits.is_empty()
    }
}

impl Default for CommitGraph {
    fn default() -> Self {
        Self::new()
    }
}

fn compute_diff(old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) -> Vec<DiffEntry> {
    let mut entries = Vec::new();

    for (path, new_hash) in new {
        match old.get(path) {
            None => entries.push(DiffEntry::Added {
                path: path.clone(),
                hash: new_hash.clone(),
            }),
            Some(old_hash) if old_hash != new_hash => entries.push(DiffEntry::Modified {
                path: path.clone(),
                old_hash: old_hash.clone(),
                new_hash: new_hash.clone(),
            }),
            _ => {}
        }
    }

    for (path, old_hash) in old {
        if !new.contains_key(path) {
            entries.push(DiffEntry::Removed {
                path: path.clone(),
                hash: old_hash.clone(),
            });
        }
    }

    entries.sort_by(|a, b| {
        let path_a = match a {
            DiffEntry::Added { path, .. }
            | DiffEntry::Removed { path, .. }
            | DiffEntry::Modified { path, .. } => path,
        };
        let path_b = match b {
            DiffEntry::Added { path, .. }
            | DiffEntry::Removed { path, .. }
            | DiffEntry::Modified { path, .. } => path,
        };
        path_a.cmp(path_b)
    });

    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn initial_commit() {
        let mut g = CommitGraph::new();
        assert!(g.is_empty());
        let snapshot = snap(&[("a.md", "hash_a")]);
        let c = g.commit("user:alice", "init", snapshot, None).unwrap();
        assert!(c.parent.is_none());
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn second_commit_requires_parent() {
        let mut g = CommitGraph::new();
        g.commit("user:alice", "init", snap(&[]), None).unwrap();
        let err = g
            .commit("user:alice", "second", snap(&[]), None)
            .unwrap_err();
        assert_eq!(err.api_code(), "PRECONDITION_FAILED");
    }

    #[test]
    fn second_commit_with_correct_parent() {
        let mut g = CommitGraph::new();
        let c1 = g
            .commit("user:alice", "init", snap(&[("a.md", "h1")]), None)
            .unwrap();
        let parent = c1.hash.clone();
        let c2 = g
            .commit(
                "user:alice",
                "update",
                snap(&[("a.md", "h2")]),
                Some(&parent),
            )
            .unwrap();
        assert_eq!(c2.parent.as_deref(), Some(parent.as_str()));
        assert_eq!(g.len(), 2);
    }

    #[test]
    fn concurrent_commit_conflict() {
        let mut g = CommitGraph::new();
        let c1 = g.commit("user:alice", "init", snap(&[]), None).unwrap();
        let parent = c1.hash.clone();
        g.commit(
            "user:bob",
            "bob's change",
            snap(&[("b.md", "hb")]),
            Some(&parent),
        )
        .unwrap();
        // Alice tries to commit with stale parent
        let err = g
            .commit(
                "user:alice",
                "alice's change",
                snap(&[("a.md", "ha")]),
                Some(&parent),
            )
            .unwrap_err();
        assert_eq!(err.api_code(), "PRECONDITION_FAILED");
    }

    #[test]
    fn log_returns_reverse_order() {
        let mut g = CommitGraph::new();
        let c1 = g.commit("user:a", "first", snap(&[]), None).unwrap();
        let h1 = c1.hash.clone();
        let c2 = g.commit("user:a", "second", snap(&[]), Some(&h1)).unwrap();
        let h2 = c2.hash.clone();
        g.commit("user:a", "third", snap(&[]), Some(&h2)).unwrap();

        let log = g.log(None);
        assert_eq!(log.len(), 3);
        assert_eq!(log[0].message, "third");
        assert_eq!(log[1].message, "second");
        assert_eq!(log[2].message, "first");
    }

    #[test]
    fn log_with_limit() {
        let mut g = CommitGraph::new();
        let c = g.commit("user:a", "first", snap(&[]), None).unwrap();
        let h = c.hash.clone();
        g.commit("user:a", "second", snap(&[]), Some(&h)).unwrap();

        let log = g.log(Some(1));
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].message, "second");
    }

    #[test]
    fn diff_added_files() {
        let mut g = CommitGraph::new();
        let c = g
            .commit(
                "user:a",
                "init",
                snap(&[("a.md", "ha"), ("b.md", "hb")]),
                None,
            )
            .unwrap();
        let hash = c.hash.clone();
        let diff = g.diff(None, &hash).unwrap();
        assert_eq!(diff.len(), 2);
        assert!(matches!(&diff[0], DiffEntry::Added { path, .. } if path == "a.md"));
        assert!(matches!(&diff[1], DiffEntry::Added { path, .. } if path == "b.md"));
    }

    #[test]
    fn diff_modified_and_removed() {
        let mut g = CommitGraph::new();
        let c1 = g
            .commit(
                "user:a",
                "init",
                snap(&[("a.md", "ha1"), ("b.md", "hb"), ("c.md", "hc")]),
                None,
            )
            .unwrap();
        let h1 = c1.hash.clone();
        let c2 = g
            .commit(
                "user:a",
                "change",
                snap(&[("a.md", "ha2"), ("c.md", "hc")]),
                Some(&h1),
            )
            .unwrap();

        let h2 = c2.hash.clone();
        let diff = g.diff(Some(&h1), &h2).unwrap();
        assert_eq!(diff.len(), 2);
        assert!(matches!(&diff[0], DiffEntry::Modified { path, .. } if path == "a.md"));
        assert!(matches!(&diff[1], DiffEntry::Removed { path, .. } if path == "b.md"));
    }

    #[test]
    fn revert_restores_state() {
        let mut g = CommitGraph::new();
        let snap1 = snap(&[("a.md", "v1")]);
        let c1 = g.commit("user:a", "v1", snap1.clone(), None).unwrap();
        let h1 = c1.hash.clone();

        let snap2 = snap(&[("a.md", "v2"), ("b.md", "new")]);
        let c2 = g.commit("user:a", "v2", snap2, Some(&h1)).unwrap();
        let _h2 = c2.hash.clone();

        let reverted = g.revert(&h1, "user:a").unwrap();
        assert_eq!(reverted.snapshot, snap1);
        assert!(reverted.message.contains("revert"));
        assert_eq!(g.len(), 3);
    }

    #[test]
    fn revert_nonexistent_commit() {
        let mut g = CommitGraph::new();
        g.commit("user:a", "init", snap(&[]), None).unwrap();
        let err = g.revert("deadbeef", "user:a").unwrap_err();
        assert_eq!(err.api_code(), "NOT_FOUND");
    }

    #[test]
    fn commit_hash_is_deterministic() {
        let snap = snap(&[("a.md", "ha")]);
        let ts = Utc::now();
        let h1 = Commit::compute_hash(None, "user:a", &ts, "msg", &snap);
        let h2 = Commit::compute_hash(None, "user:a", &ts, "msg", &snap);
        assert_eq!(h1, h2);
    }

    #[test]
    fn different_content_different_commit_hash() {
        let ts = Utc::now();
        let h1 = Commit::compute_hash(None, "user:a", &ts, "msg", &snap(&[("a.md", "h1")]));
        let h2 = Commit::compute_hash(None, "user:a", &ts, "msg", &snap(&[("a.md", "h2")]));
        assert_ne!(h1, h2);
    }

    #[test]
    fn json_roundtrip() {
        let mut g = CommitGraph::new();
        let c = g
            .commit("user:a", "init", snap(&[("f.md", "hf")]), None)
            .unwrap();
        let h = c.hash.clone();
        g.commit("user:a", "update", snap(&[("f.md", "hf2")]), Some(&h))
            .unwrap();

        let json = g.to_json().unwrap();
        let g2 = CommitGraph::from_json(&json).unwrap();
        assert_eq!(g2.len(), 2);
        assert_eq!(g2.head().unwrap().message, "update");
    }

    #[test]
    fn idempotent_revert() {
        let mut g = CommitGraph::new();
        let snap_v1 = snap(&[("a.md", "v1")]);
        let c1 = g.commit("user:a", "v1", snap_v1.clone(), None).unwrap();
        let h1 = c1.hash.clone();
        let c2 = g
            .commit("user:a", "v2", snap(&[("a.md", "v2")]), Some(&h1))
            .unwrap();
        let _h2 = c2.hash.clone();

        // Revert to v1
        let r1 = g.revert(&h1, "user:a").unwrap();
        assert_eq!(r1.snapshot, snap_v1);

        // Revert again to v1 (idempotent — snapshot same, but new commit)
        let r1_hash = r1.hash.clone();
        // Need the target to be h1 again
        let r2 = g.revert(&h1, "user:a").unwrap();
        assert_eq!(r2.snapshot, snap_v1);
        assert_ne!(r2.hash, r1_hash); // different commit (different parent)
    }
}
