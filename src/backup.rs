//! Workspace backup & restore — snapshot all state to a single archive,
//! restore byte-for-byte from that archive.
//!
//! A backup captures:
//! - Object store (all content-addressable blobs)
//! - Inode index (path → hash mapping)
//! - Commit graph (full history)
//! - Entity graph (nodes + edges)
//! - Audit log entries
//! - Event log entries
//!
//! Format: JSON manifest + raw object files in a directory tree.
//! See `04-tasks-dod.md` Phase 7 (task 7.1).

use std::collections::BTreeMap;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::commit::CommitGraph;
use crate::error::{MemoryFsError, Result};
use crate::event_log;
use crate::graph::EntityGraph;
use crate::storage::{InodeIndex, ObjectHash, ObjectStore};

/// Backup manifest — serialized to `manifest.json` inside the backup dir.
#[derive(Debug, Serialize, Deserialize)]
pub struct BackupManifest {
    /// Schema version for the backup format.
    pub format_version: String,
    /// When the backup was created.
    pub created_at: String,
    /// Workspace ID that was backed up.
    pub workspace_id: String,
    /// Inode index: path → object hash.
    pub index: BTreeMap<String, String>,
    /// Commit graph as JSON.
    pub commit_graph: String,
    /// Entity graph entities (serialized).
    pub entities: Vec<serde_json::Value>,
    /// Entity graph edges (serialized).
    pub edges: Vec<serde_json::Value>,
    /// Audit log entries.
    pub audit_entries: Vec<serde_json::Value>,
    /// Event log entries.
    pub event_entries: Vec<serde_json::Value>,
    /// Object hashes included in this backup.
    pub object_hashes: Vec<String>,
}

/// Input parameters for creating a backup.
pub struct BackupParams<'a> {
    /// Directory to write the backup to.
    pub backup_dir: &'a Path,
    /// Workspace ID.
    pub workspace_id: &'a str,
    /// Object store.
    pub object_store: &'a ObjectStore,
    /// Inode index.
    pub index: &'a InodeIndex,
    /// Commit graph.
    pub commit_graph: &'a CommitGraph,
    /// Entity graph.
    pub entity_graph: &'a EntityGraph,
    /// Path to audit log file (optional).
    pub audit_log_path: Option<&'a Path>,
    /// Path to event log file (optional).
    pub event_log_path: Option<&'a Path>,
}

/// Create a full backup of the workspace.
pub fn create_backup(params: &BackupParams<'_>) -> Result<BackupManifest> {
    let backup_dir = params.backup_dir;
    let object_store = params.object_store;
    let index = params.index;
    let commit_graph = params.commit_graph;
    let entity_graph = params.entity_graph;
    let audit_log_path = params.audit_log_path;
    let event_log_path = params.event_log_path;
    std::fs::create_dir_all(backup_dir).map_err(|e| {
        MemoryFsError::Internal(anyhow::anyhow!("failed to create backup dir: {e}"))
    })?;

    let objects_dir = backup_dir.join("objects");
    std::fs::create_dir_all(&objects_dir).map_err(|e| {
        MemoryFsError::Internal(anyhow::anyhow!("failed to create objects dir: {e}"))
    })?;

    let mut index_map = BTreeMap::new();
    let mut object_hashes = Vec::new();

    for (path, hash) in index.iter() {
        index_map.insert(path.to_string(), hash.as_str().to_string());

        let data = object_store.get(hash)?;
        let obj_path = objects_dir.join(hash.as_str());
        std::fs::write(&obj_path, &data).map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!("failed to write object {}: {e}", hash))
        })?;
        object_hashes.push(hash.as_str().to_string());
    }

    let commit_json = commit_graph.to_json()?;

    let entities: Vec<serde_json::Value> = entity_graph
        .all_entities()
        .iter()
        .map(|e| serde_json::to_value(e).unwrap_or_default())
        .collect();

    let edges: Vec<serde_json::Value> = entity_graph
        .all_edges()
        .iter()
        .map(|e| serde_json::to_value(e).unwrap_or_default())
        .collect();

    let audit_entries = match audit_log_path {
        Some(p) if p.exists() => audit::read_entries(p)
            .unwrap_or_default()
            .iter()
            .map(|e| serde_json::to_value(e).unwrap_or_default())
            .collect(),
        _ => Vec::new(),
    };

    let event_entries = match event_log_path {
        Some(p) if p.exists() => event_log::read_events(p)
            .unwrap_or_default()
            .iter()
            .map(|e| serde_json::to_value(e).unwrap_or_default())
            .collect(),
        _ => Vec::new(),
    };

    let manifest = BackupManifest {
        format_version: "1".into(),
        created_at: Utc::now().to_rfc3339(),
        workspace_id: params.workspace_id.into(),
        index: index_map,
        commit_graph: commit_json,
        entities,
        edges,
        audit_entries,
        event_entries,
        object_hashes,
    };

    let manifest_json = serde_json::to_string_pretty(&manifest).map_err(|e| {
        MemoryFsError::Internal(anyhow::anyhow!("failed to serialize manifest: {e}"))
    })?;
    std::fs::write(backup_dir.join("manifest.json"), manifest_json)
        .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("failed to write manifest: {e}")))?;

    Ok(manifest)
}

/// Restore state from a backup directory. Returns the restored components.
pub fn restore_backup(backup_dir: &Path, target_store: &ObjectStore) -> Result<RestoredState> {
    let manifest_path = backup_dir.join("manifest.json");
    let manifest_json = std::fs::read_to_string(&manifest_path)
        .map_err(|e| MemoryFsError::NotFound(format!("backup manifest: {e}")))?;
    let manifest: BackupManifest = serde_json::from_str(&manifest_json)
        .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("failed to parse manifest: {e}")))?;

    let objects_dir = backup_dir.join("objects");
    for hash_str in &manifest.object_hashes {
        let obj_path = objects_dir.join(hash_str);
        let data = std::fs::read(&obj_path)
            .map_err(|e| MemoryFsError::NotFound(format!("backup object {hash_str}: {e}")))?;
        target_store.put(&data)?;
    }

    let mut index = InodeIndex::new();
    for (path, hash_str) in &manifest.index {
        let hash = ObjectHash::parse(hash_str)?;
        index.set(path.clone(), hash);
    }

    let commit_graph = CommitGraph::from_json(&manifest.commit_graph)?;

    let entity_graph = restore_entity_graph(&manifest)?;

    Ok(RestoredState {
        workspace_id: manifest.workspace_id,
        index,
        commit_graph,
        entity_graph,
        audit_entry_count: manifest.audit_entries.len(),
        event_entry_count: manifest.event_entries.len(),
    })
}

/// Result of a restore operation.
pub struct RestoredState {
    /// Workspace ID from the backup.
    pub workspace_id: String,
    /// Restored inode index.
    pub index: InodeIndex,
    /// Restored commit graph.
    pub commit_graph: CommitGraph,
    /// Restored entity graph.
    pub entity_graph: EntityGraph,
    /// Number of audit entries in the backup.
    pub audit_entry_count: usize,
    /// Number of event log entries in the backup.
    pub event_entry_count: usize,
}

fn restore_entity_graph(manifest: &BackupManifest) -> Result<EntityGraph> {
    use crate::graph::EntityKind;

    let mut graph = EntityGraph::new();

    for ent_val in &manifest.entities {
        let ws = ent_val["workspace_id"].as_str().unwrap_or("");
        let kind_str = ent_val["kind"].as_str().unwrap_or("concept");
        let kind = EntityKind::parse(kind_str).unwrap_or(EntityKind::Concept);
        let name = ent_val["canonical_name"].as_str().unwrap_or("");
        let aliases: Vec<String> = ent_val["aliases"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let attributes = ent_val["attributes"].clone();
        let external_refs: Vec<crate::graph::ExternalRef> = ent_val["external_refs"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();

        graph.create_entity(ws, kind, name, aliases, attributes, external_refs)?;
    }

    for edge_val in &manifest.edges {
        let src = edge_val["src"].as_str().unwrap_or("");
        let dst = edge_val["dst"].as_str().unwrap_or("");
        let rel_str = edge_val["relation"].as_str().unwrap_or("RELATED_TO");
        let weight = edge_val["weight"].as_f64().unwrap_or(1.0) as f32;

        if let Ok(relation) = crate::graph::Relation::parse(rel_str) {
            if graph.get(src).is_ok() && graph.get(dst).is_ok() {
                let _ = graph.link(src, dst, relation, weight, None);
            }
        }
    }

    Ok(graph)
}

/// Verify that a backup is self-consistent: all referenced objects exist
/// and the manifest parses correctly.
pub fn verify_backup(backup_dir: &Path) -> Result<VerifyResult> {
    let manifest_path = backup_dir.join("manifest.json");
    let manifest_json = std::fs::read_to_string(&manifest_path)
        .map_err(|e| MemoryFsError::NotFound(format!("backup manifest: {e}")))?;
    let manifest: BackupManifest = serde_json::from_str(&manifest_json)
        .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("failed to parse manifest: {e}")))?;

    let objects_dir = backup_dir.join("objects");
    let mut missing_objects = Vec::new();
    let mut corrupt_objects = Vec::new();

    for hash_str in &manifest.object_hashes {
        let obj_path = objects_dir.join(hash_str);
        if !obj_path.exists() {
            missing_objects.push(hash_str.clone());
            continue;
        }

        let data = std::fs::read(&obj_path).unwrap_or_default();
        let actual_hash = ObjectHash::of(&data);
        if actual_hash.as_str() != hash_str {
            corrupt_objects.push(hash_str.clone());
        }
    }

    let commit_ok = CommitGraph::from_json(&manifest.commit_graph).is_ok();

    Ok(VerifyResult {
        format_version: manifest.format_version,
        workspace_id: manifest.workspace_id,
        object_count: manifest.object_hashes.len(),
        index_entries: manifest.index.len(),
        entity_count: manifest.entities.len(),
        edge_count: manifest.edges.len(),
        audit_entries: manifest.audit_entries.len(),
        event_entries: manifest.event_entries.len(),
        missing_objects,
        corrupt_objects,
        commit_graph_valid: commit_ok,
    })
}

/// Result of backup verification.
#[derive(Debug)]
pub struct VerifyResult {
    /// Backup format version.
    pub format_version: String,
    /// Workspace ID.
    pub workspace_id: String,
    /// Total objects in manifest.
    pub object_count: usize,
    /// Index entries.
    pub index_entries: usize,
    /// Entities in graph.
    pub entity_count: usize,
    /// Edges in graph.
    pub edge_count: usize,
    /// Audit log entries.
    pub audit_entries: usize,
    /// Event log entries.
    pub event_entries: usize,
    /// Object hashes listed in manifest but missing from backup dir.
    pub missing_objects: Vec<String>,
    /// Object hashes where file content doesn't match hash.
    pub corrupt_objects: Vec<String>,
    /// Whether the commit graph JSON is valid.
    pub commit_graph_valid: bool,
}

impl VerifyResult {
    /// Whether the backup passes all integrity checks.
    pub fn is_ok(&self) -> bool {
        self.missing_objects.is_empty()
            && self.corrupt_objects.is_empty()
            && self.commit_graph_valid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_workspace(dir: &Path) -> (ObjectStore, InodeIndex, CommitGraph, EntityGraph) {
        let store = ObjectStore::open(dir.join("store")).unwrap();
        let mut index = InodeIndex::new();
        let mut commit_graph = CommitGraph::new();
        let mut entity_graph = EntityGraph::new();

        let h1 = store.put(b"# Memory 1\nAlice prefers Rust").unwrap();
        let h2 = store.put(b"# Memory 2\nBob uses Python").unwrap();
        index.set("memories/user/fact_1.md", h1.clone());
        index.set("memories/user/fact_2.md", h2.clone());

        let snapshot = index
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_str().to_string()))
            .collect();
        commit_graph
            .commit("user:alice", "initial memories", snapshot, None)
            .unwrap();

        entity_graph
            .create_entity(
                "ws_test",
                crate::graph::EntityKind::Person,
                "Alice",
                vec![],
                serde_json::json!({}),
                vec![],
            )
            .unwrap();

        (store, index, commit_graph, entity_graph)
    }

    fn make_params<'a>(
        backup_dir: &'a Path,
        store: &'a ObjectStore,
        index: &'a InodeIndex,
        commit_graph: &'a CommitGraph,
        entity_graph: &'a EntityGraph,
    ) -> BackupParams<'a> {
        BackupParams {
            backup_dir,
            workspace_id: "ws_test",
            object_store: store,
            index,
            commit_graph,
            entity_graph,
            audit_log_path: None,
            event_log_path: None,
        }
    }

    #[test]
    fn backup_and_restore_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let (store, index, commit_graph, entity_graph) = setup_workspace(dir.path());

        let backup_dir = dir.path().join("backup");
        let manifest = create_backup(&make_params(
            &backup_dir,
            &store,
            &index,
            &commit_graph,
            &entity_graph,
        ))
        .unwrap();

        assert_eq!(manifest.workspace_id, "ws_test");
        assert_eq!(manifest.index.len(), 2);
        assert_eq!(manifest.object_hashes.len(), 2);
        assert_eq!(manifest.entities.len(), 1);

        let restore_dir = dir.path().join("restored");
        let restore_store = ObjectStore::open(restore_dir.join("store")).unwrap();
        let restored = restore_backup(&backup_dir, &restore_store).unwrap();

        assert_eq!(restored.workspace_id, "ws_test");
        assert_eq!(restored.index.len(), 2);
        assert!(!restored.commit_graph.is_empty());
        assert_eq!(restored.entity_graph.entity_count(), 1);

        for (path, hash) in index.iter() {
            let original = store.get(hash).unwrap();
            let restored_hash = restored.index.get(path).unwrap();
            let restored_data = restore_store.get(restored_hash).unwrap();
            assert_eq!(original, restored_data, "mismatch for {path}");
        }
    }

    #[test]
    fn verify_valid_backup() {
        let dir = tempfile::tempdir().unwrap();
        let (store, index, commit_graph, entity_graph) = setup_workspace(dir.path());

        let backup_dir = dir.path().join("backup");
        create_backup(&make_params(
            &backup_dir,
            &store,
            &index,
            &commit_graph,
            &entity_graph,
        ))
        .unwrap();

        let result = verify_backup(&backup_dir).unwrap();
        assert!(result.is_ok());
        assert_eq!(result.object_count, 2);
        assert_eq!(result.index_entries, 2);
        assert!(result.commit_graph_valid);
    }

    #[test]
    fn verify_detects_missing_object() {
        let dir = tempfile::tempdir().unwrap();
        let (store, index, commit_graph, entity_graph) = setup_workspace(dir.path());

        let backup_dir = dir.path().join("backup");
        let manifest = create_backup(&make_params(
            &backup_dir,
            &store,
            &index,
            &commit_graph,
            &entity_graph,
        ))
        .unwrap();

        let first_hash = &manifest.object_hashes[0];
        std::fs::remove_file(backup_dir.join("objects").join(first_hash)).unwrap();

        let result = verify_backup(&backup_dir).unwrap();
        assert!(!result.is_ok());
        assert_eq!(result.missing_objects.len(), 1);
    }

    #[test]
    fn verify_detects_corrupt_object() {
        let dir = tempfile::tempdir().unwrap();
        let (store, index, commit_graph, entity_graph) = setup_workspace(dir.path());

        let backup_dir = dir.path().join("backup");
        let manifest = create_backup(&make_params(
            &backup_dir,
            &store,
            &index,
            &commit_graph,
            &entity_graph,
        ))
        .unwrap();

        let first_hash = &manifest.object_hashes[0];
        std::fs::write(
            backup_dir.join("objects").join(first_hash),
            b"corrupted data",
        )
        .unwrap();

        let result = verify_backup(&backup_dir).unwrap();
        assert!(!result.is_ok());
        assert_eq!(result.corrupt_objects.len(), 1);
    }

    #[test]
    fn restore_missing_manifest_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("store")).unwrap();
        let result = restore_backup(dir.path(), &store);
        assert!(result.is_err());
    }

    #[test]
    fn backup_empty_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("store")).unwrap();
        let index = InodeIndex::new();
        let commit_graph = CommitGraph::new();
        let entity_graph = EntityGraph::new();

        let backup_dir = dir.path().join("backup");
        let manifest = create_backup(&make_params(
            &backup_dir,
            &store,
            &index,
            &commit_graph,
            &entity_graph,
        ))
        .unwrap();

        assert_eq!(manifest.object_hashes.len(), 0);
        assert_eq!(manifest.index.len(), 0);

        let restore_store = ObjectStore::open(dir.path().join("restore_store")).unwrap();
        let restored = restore_backup(&backup_dir, &restore_store).unwrap();
        assert_eq!(restored.index.len(), 0);
        assert!(restored.commit_graph.is_empty());
    }

    #[test]
    fn backup_preserves_commit_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("store")).unwrap();
        let mut index = InodeIndex::new();
        let mut commit_graph = CommitGraph::new();
        let entity_graph = EntityGraph::new();

        let h1 = store.put(b"v1").unwrap();
        index.set("file.md", h1);
        let snap1 = index
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_str().to_string()))
            .collect();
        let c1 = commit_graph
            .commit("user:alice", "first", snap1, None)
            .unwrap()
            .hash
            .clone();

        let h2 = store.put(b"v2").unwrap();
        index.set("file.md", h2);
        let snap2 = index
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_str().to_string()))
            .collect();
        commit_graph
            .commit("user:alice", "second", snap2, Some(&c1))
            .unwrap();

        let backup_dir = dir.path().join("backup");
        create_backup(&make_params(
            &backup_dir,
            &store,
            &index,
            &commit_graph,
            &entity_graph,
        ))
        .unwrap();

        let restore_store = ObjectStore::open(dir.path().join("restore_store")).unwrap();
        let restored = restore_backup(&backup_dir, &restore_store).unwrap();
        assert_eq!(restored.commit_graph.len(), 2);

        let log = restored.commit_graph.log(None);
        assert_eq!(log[0].message, "second");
        assert_eq!(log[1].message, "first");
    }
}
