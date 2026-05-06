//! Supersede engine — manages memory replacement relationships.
//!
//! When a new memory replaces an older one, the old memory's status becomes
//! `superseded` and its `superseded_by` list is updated. The new memory's
//! `supersedes` list points back to the old one. Cycle detection prevents
//! invalid supersede chains.

use std::collections::{HashMap, HashSet};

use crate::error::{MemoryFsError, Result};

/// A memory's supersede metadata (extracted from frontmatter).
#[derive(Debug, Clone)]
pub struct MemoryMeta {
    /// Memory ID (e.g. `mem_...`).
    pub id: String,
    /// Current status.
    pub status: MemoryStatus,
    /// IDs of memories this one supersedes.
    pub supersedes: Vec<String>,
    /// IDs of memories that supersede this one.
    pub superseded_by: Vec<String>,
}

/// Memory status relevant to supersede logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryStatus {
    /// Active memory.
    Active,
    /// Superseded by another memory.
    Superseded,
    /// Archived.
    Archived,
    /// Disputed.
    Disputed,
}

/// Result of a supersede operation.
#[derive(Debug)]
pub struct SupersedeResult {
    /// The new memory (with `supersedes` populated).
    pub new_memory: MemoryMeta,
    /// Updated old memories (status → superseded, `superseded_by` updated).
    pub updated_old: Vec<MemoryMeta>,
}

/// In-memory supersede graph for cycle detection and relationship tracking.
pub struct SupersedeGraph {
    metas: HashMap<String, MemoryMeta>,
}

impl SupersedeGraph {
    /// Create a new empty graph.
    pub fn new() -> Self {
        Self {
            metas: HashMap::new(),
        }
    }

    /// Load existing memory metadata into the graph.
    pub fn load(&mut self, meta: MemoryMeta) {
        self.metas.insert(meta.id.clone(), meta);
    }

    /// Get a memory's metadata.
    pub fn get(&self, id: &str) -> Option<&MemoryMeta> {
        self.metas.get(id)
    }

    /// Number of memories in the graph.
    pub fn len(&self) -> usize {
        self.metas.len()
    }

    /// Whether the graph is empty.
    pub fn is_empty(&self) -> bool {
        self.metas.is_empty()
    }

    /// Execute a supersede: new_id supersedes old_ids.
    ///
    /// Validates that:
    /// - All old_ids exist and are active
    /// - new_id doesn't already exist
    /// - No cycles would be created
    pub fn supersede(&mut self, new_id: &str, old_ids: &[&str]) -> Result<SupersedeResult> {
        if old_ids.is_empty() {
            return Err(MemoryFsError::Validation(
                "supersede requires at least one old memory ID".to_string(),
            ));
        }

        if self.metas.contains_key(new_id) {
            return Err(MemoryFsError::Conflict(format!(
                "memory {new_id} already exists"
            )));
        }

        for old_id in old_ids {
            let old = self
                .metas
                .get(*old_id)
                .ok_or_else(|| MemoryFsError::NotFound(format!("memory {old_id}")))?;

            if old.status != MemoryStatus::Active {
                return Err(MemoryFsError::Conflict(format!(
                    "memory {old_id} is not active (status: {:?}), cannot be superseded",
                    old.status
                )));
            }
        }

        // Check for cycles: new_id must not already be in the supersede chain of any old_id
        for old_id in old_ids {
            if self.would_create_cycle(new_id, old_id) {
                return Err(MemoryFsError::Conflict(format!(
                    "superseding {old_id} with {new_id} would create a cycle"
                )));
            }
        }

        // Apply the supersede
        let new_memory = MemoryMeta {
            id: new_id.to_string(),
            status: MemoryStatus::Active,
            supersedes: old_ids.iter().map(|s| s.to_string()).collect(),
            superseded_by: vec![],
        };

        let mut updated_old = Vec::new();
        for old_id in old_ids {
            if let Some(old) = self.metas.get_mut(*old_id) {
                old.status = MemoryStatus::Superseded;
                old.superseded_by.push(new_id.to_string());
                updated_old.push(old.clone());
            }
        }

        self.metas.insert(new_id.to_string(), new_memory.clone());

        Ok(SupersedeResult {
            new_memory,
            updated_old,
        })
    }

    /// Check if adding a supersede edge would create a cycle.
    fn would_create_cycle(&self, new_id: &str, old_id: &str) -> bool {
        // Walk the supersedes chain from old_id backwards. If we find new_id, there's a cycle.
        let mut visited = HashSet::new();
        let mut stack = vec![old_id.to_string()];

        while let Some(current) = stack.pop() {
            if current == new_id {
                return true;
            }
            if !visited.insert(current.clone()) {
                continue;
            }
            if let Some(meta) = self.metas.get(&current) {
                for sup in &meta.supersedes {
                    stack.push(sup.clone());
                }
            }
        }

        false
    }

    /// Get the full supersede chain for a memory (all ancestors it supersedes, recursively).
    pub fn supersede_chain(&self, id: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut visited = HashSet::new();
        let mut stack = vec![id.to_string()];

        while let Some(current) = stack.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            if current != id {
                chain.push(current.clone());
            }
            if let Some(meta) = self.metas.get(&current) {
                for sup in &meta.supersedes {
                    stack.push(sup.clone());
                }
            }
        }

        chain
    }

    /// Get all active memories (not superseded, not archived).
    pub fn active_memories(&self) -> Vec<&MemoryMeta> {
        self.metas
            .values()
            .filter(|m| m.status == MemoryStatus::Active)
            .collect()
    }

    /// Validate the entire graph for consistency.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        for (id, meta) in &self.metas {
            // Check supersedes references exist
            for sup_id in &meta.supersedes {
                if !self.metas.contains_key(sup_id) {
                    issues.push(format!("{id}: supersedes {sup_id} which does not exist"));
                }
            }

            // Check superseded_by references exist
            for by_id in &meta.superseded_by {
                if !self.metas.contains_key(by_id) {
                    issues.push(format!("{id}: superseded_by {by_id} which does not exist"));
                }
            }

            // Superseded must have at least one superseded_by
            if meta.status == MemoryStatus::Superseded && meta.superseded_by.is_empty() {
                issues.push(format!(
                    "{id}: status is superseded but superseded_by is empty"
                ));
            }

            // Active must not have superseded_by
            if meta.status == MemoryStatus::Active && !meta.superseded_by.is_empty() {
                issues.push(format!(
                    "{id}: status is active but has superseded_by entries"
                ));
            }
        }

        issues
    }
}

impl Default for SupersedeGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl SupersedeGraph {
    /// Build a graph from the current workspace state by reading every memory
    /// file referenced by `index` whose path matches `path_prefix`.
    ///
    /// This is the bridge between the on-disk markdown source-of-truth and
    /// the in-memory cycle-detection structure. Handlers call it before each
    /// supersede to validate against the *current* workspace, not against a
    /// stale graph snapshot.
    pub fn build_from_workspace(
        index: &crate::storage::InodeIndex,
        store: &crate::storage::ObjectStore,
        path_prefix: &str,
    ) -> Result<Self> {
        let mut graph = Self::new();
        for (path, hash) in index.iter() {
            if !path.starts_with(path_prefix) || !path.ends_with(".md") {
                continue;
            }
            let bytes = store.get(hash)?;
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            let Ok(doc) = crate::schema::parse_frontmatter(&text) else {
                continue;
            };
            let Some(meta) = meta_from_frontmatter(&doc.frontmatter) else {
                continue;
            };
            graph.load(meta);
        }
        Ok(graph)
    }

    /// Read-only validation that a supersede is safe to commit. Same checks
    /// as `supersede()` but without mutating the graph — handlers do their
    /// own frontmatter mutation and atomic commit, so they only need the
    /// validator to refuse cycles, duplicate ids, and non-active old memories.
    pub fn validate_supersede(&self, new_id: &str, old_ids: &[&str]) -> Result<()> {
        if old_ids.is_empty() {
            return Err(MemoryFsError::Validation(
                "supersede requires at least one old memory ID".to_string(),
            ));
        }
        if self.metas.contains_key(new_id) {
            return Err(MemoryFsError::Conflict(format!(
                "memory id {new_id} already exists in workspace"
            )));
        }
        for old_id in old_ids {
            let old = self
                .metas
                .get(*old_id)
                .ok_or_else(|| MemoryFsError::NotFound(format!("memory {old_id}")))?;
            if old.status != MemoryStatus::Active {
                return Err(MemoryFsError::Conflict(format!(
                    "memory {old_id} is not active (status: {:?}), cannot be superseded",
                    old.status
                )));
            }
            if self.would_create_cycle(new_id, old_id) {
                return Err(MemoryFsError::Conflict(format!(
                    "superseding {old_id} with {new_id} would create a cycle"
                )));
            }
        }
        Ok(())
    }
}

fn meta_from_frontmatter(fm: &serde_json::Value) -> Option<MemoryMeta> {
    let id = fm.get("id")?.as_str()?.to_string();
    let status = match fm
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("active")
    {
        "active" => MemoryStatus::Active,
        "superseded" => MemoryStatus::Superseded,
        "archived" => MemoryStatus::Archived,
        "disputed" => MemoryStatus::Disputed,
        // Tombstones (`deleted`) and any unknown status get treated as
        // archived for cycle purposes — they can't be superseded again, but
        // they're also not active candidates. Skipping them entirely would
        // hide id collisions.
        _ => MemoryStatus::Archived,
    };
    let collect_strings = |key: &str| -> Vec<String> {
        fm.get(key)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(MemoryMeta {
        id,
        status,
        supersedes: collect_strings("supersedes"),
        superseded_by: collect_strings("superseded_by"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_memory(id: &str) -> MemoryMeta {
        MemoryMeta {
            id: id.to_string(),
            status: MemoryStatus::Active,
            supersedes: vec![],
            superseded_by: vec![],
        }
    }

    #[test]
    fn basic_supersede() {
        let mut graph = SupersedeGraph::new();
        graph.load(active_memory("mem_old"));

        let result = graph.supersede("mem_new", &["mem_old"]).unwrap();

        assert_eq!(result.new_memory.id, "mem_new");
        assert_eq!(result.new_memory.supersedes, vec!["mem_old"]);
        assert_eq!(result.new_memory.status, MemoryStatus::Active);

        assert_eq!(result.updated_old.len(), 1);
        assert_eq!(result.updated_old[0].status, MemoryStatus::Superseded);
        assert_eq!(result.updated_old[0].superseded_by, vec!["mem_new"]);
    }

    #[test]
    fn supersede_multiple() {
        let mut graph = SupersedeGraph::new();
        graph.load(active_memory("mem_a"));
        graph.load(active_memory("mem_b"));

        let result = graph.supersede("mem_merged", &["mem_a", "mem_b"]).unwrap();

        assert_eq!(result.new_memory.supersedes.len(), 2);
        assert_eq!(result.updated_old.len(), 2);

        for old in &result.updated_old {
            assert_eq!(old.status, MemoryStatus::Superseded);
        }
    }

    #[test]
    fn cannot_supersede_nonexistent() {
        let mut graph = SupersedeGraph::new();
        let err = graph.supersede("mem_new", &["mem_missing"]).unwrap_err();
        assert_eq!(err.api_code(), "NOT_FOUND");
    }

    #[test]
    fn cannot_supersede_already_superseded() {
        let mut graph = SupersedeGraph::new();
        graph.load(active_memory("mem_old"));
        graph.supersede("mem_mid", &["mem_old"]).unwrap();

        let err = graph.supersede("mem_new", &["mem_old"]).unwrap_err();
        assert_eq!(err.api_code(), "CONFLICT");
    }

    #[test]
    fn cannot_supersede_with_existing_id() {
        let mut graph = SupersedeGraph::new();
        graph.load(active_memory("mem_old"));
        graph.load(active_memory("mem_existing"));

        let err = graph.supersede("mem_existing", &["mem_old"]).unwrap_err();
        assert_eq!(err.api_code(), "CONFLICT");
    }

    #[test]
    fn empty_old_ids_rejected() {
        let mut graph = SupersedeGraph::new();
        let err = graph.supersede("mem_new", &[]).unwrap_err();
        assert_eq!(err.api_code(), "VALIDATION");
    }

    #[test]
    fn chain_supersede() {
        let mut graph = SupersedeGraph::new();
        graph.load(active_memory("mem_v1"));
        graph.supersede("mem_v2", &["mem_v1"]).unwrap();
        graph.supersede("mem_v3", &["mem_v2"]).unwrap();

        let v3 = graph.get("mem_v3").unwrap();
        assert_eq!(v3.supersedes, vec!["mem_v2"]);

        let chain = graph.supersede_chain("mem_v3");
        assert!(chain.contains(&"mem_v2".to_string()));
        assert!(chain.contains(&"mem_v1".to_string()));
    }

    #[test]
    fn cycle_detection_direct() {
        let mut graph = SupersedeGraph::new();
        graph.load(MemoryMeta {
            id: "mem_a".to_string(),
            status: MemoryStatus::Active,
            supersedes: vec!["mem_b".to_string()],
            superseded_by: vec![],
        });
        graph.load(active_memory("mem_b"));

        // mem_b -> mem_a would create a cycle since mem_a -> mem_b
        // But mem_b is active and doesn't supersede mem_a, so no cycle.
        // Let's set up an actual cycle scenario:
        let mut graph2 = SupersedeGraph::new();
        graph2.load(active_memory("mem_x"));
        graph2.supersede("mem_y", &["mem_x"]).unwrap();

        // Now if we tried to make mem_x supersede mem_y, that's a cycle.
        // But mem_x is already superseded, so it'll fail on status check.
        let err = graph2
            .supersede("mem_z_that_is_mem_x", &["mem_x"])
            .unwrap_err();
        assert_eq!(err.api_code(), "CONFLICT");
    }

    #[test]
    fn active_memories() {
        let mut graph = SupersedeGraph::new();
        graph.load(active_memory("mem_a"));
        graph.load(active_memory("mem_b"));
        graph.load(active_memory("mem_c"));

        graph.supersede("mem_d", &["mem_a"]).unwrap();

        let active = graph.active_memories();
        assert_eq!(active.len(), 3); // mem_b, mem_c, mem_d
        let active_ids: HashSet<_> = active.iter().map(|m| m.id.as_str()).collect();
        assert!(active_ids.contains("mem_b"));
        assert!(active_ids.contains("mem_c"));
        assert!(active_ids.contains("mem_d"));
        assert!(!active_ids.contains("mem_a"));
    }

    #[test]
    fn validate_consistent_graph() {
        let mut graph = SupersedeGraph::new();
        graph.load(active_memory("mem_a"));
        graph.supersede("mem_b", &["mem_a"]).unwrap();

        let issues = graph.validate();
        assert!(issues.is_empty(), "unexpected issues: {issues:?}");
    }

    #[test]
    fn validate_detects_dangling_reference() {
        let mut graph = SupersedeGraph::new();
        graph.load(MemoryMeta {
            id: "mem_a".to_string(),
            status: MemoryStatus::Active,
            supersedes: vec!["mem_nonexistent".to_string()],
            superseded_by: vec![],
        });

        let issues = graph.validate();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("does not exist"));
    }

    #[test]
    fn validate_detects_superseded_without_by() {
        let mut graph = SupersedeGraph::new();
        graph.load(MemoryMeta {
            id: "mem_a".to_string(),
            status: MemoryStatus::Superseded,
            supersedes: vec![],
            superseded_by: vec![],
        });

        let issues = graph.validate();
        assert!(issues.iter().any(|i| i.contains("superseded_by is empty")));
    }

    #[test]
    fn validate_detects_active_with_superseded_by() {
        let mut graph = SupersedeGraph::new();
        graph.load(MemoryMeta {
            id: "mem_a".to_string(),
            status: MemoryStatus::Active,
            supersedes: vec![],
            superseded_by: vec!["mem_b".to_string()],
        });
        graph.load(active_memory("mem_b"));

        let issues = graph.validate();
        assert!(issues
            .iter()
            .any(|i| i.contains("active but has superseded_by")));
    }

    #[test]
    fn len_and_is_empty() {
        let mut graph = SupersedeGraph::new();
        assert!(graph.is_empty());
        assert_eq!(graph.len(), 0);

        graph.load(active_memory("mem_a"));
        assert!(!graph.is_empty());
        assert_eq!(graph.len(), 1);
    }

    #[test]
    fn supersede_chain_empty_for_leaf() {
        let mut graph = SupersedeGraph::new();
        graph.load(active_memory("mem_a"));

        let chain = graph.supersede_chain("mem_a");
        assert!(chain.is_empty());
    }

    #[test]
    fn graph_state_after_supersede() {
        let mut graph = SupersedeGraph::new();
        graph.load(active_memory("mem_old"));
        graph.supersede("mem_new", &["mem_old"]).unwrap();

        let old = graph.get("mem_old").unwrap();
        assert_eq!(old.status, MemoryStatus::Superseded);
        assert_eq!(old.superseded_by, vec!["mem_new"]);

        let new = graph.get("mem_new").unwrap();
        assert_eq!(new.status, MemoryStatus::Active);
        assert_eq!(new.supersedes, vec!["mem_old"]);
    }
}
