//! Chaos engineering test suite — simulates crash recovery, corruption,
//! concurrent access, and resource exhaustion scenarios.
//!
//! These tests verify that MemoryFS maintains data integrity under adverse
//! conditions: partial writes, tampered objects, truncated logs, concurrent
//! commits, and large payloads.
//!
//! See `04-tasks-dod.md` Phase 7 (task 7.3).

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::audit::{AuditAction, AuditLog};
    use crate::backup::{create_backup, restore_backup, verify_backup, BackupParams};
    use crate::commit::CommitGraph;
    use crate::event_log::{EventKind, EventLog};
    use crate::graph::EntityGraph;
    use crate::migration::{MigrationRunner, SchemaState};
    use crate::storage::{InodeIndex, ObjectHash, ObjectStore};

    // ── Object store crash recovery ──────────────────────────────────────

    #[test]
    fn corrupted_object_detected_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("objects")).unwrap();

        let hash = store.put(b"original content").unwrap();
        store.verify(&hash).unwrap();

        // ObjectStore uses sharded layout: <root>/objects/<aa>/<bb>/<full-hex>
        let hex = hash.as_str();
        let obj_path = store
            .root()
            .join("objects")
            .join(&hex[..2])
            .join(&hex[2..4])
            .join(hex);
        std::fs::write(&obj_path, b"CORRUPTED DATA").unwrap();

        let err = store.verify(&hash).unwrap_err();
        assert!(err.to_string().contains("corrupted"));
    }

    #[test]
    fn missing_object_returns_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("objects")).unwrap();

        let hash =
            ObjectHash::parse("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
                .unwrap();
        let err = store.get(&hash).unwrap_err();
        assert!(err.to_string().contains("not found") || err.to_string().contains("object"));
    }

    #[test]
    fn delete_and_reput_recovers_from_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("objects")).unwrap();

        let data = b"important data";
        let hash1 = store.put(data).unwrap();

        // Corrupt via sharded path
        let hex = hash1.as_str();
        let obj_path = store
            .root()
            .join("objects")
            .join(&hex[..2])
            .join(&hex[2..4])
            .join(hex);
        std::fs::write(&obj_path, b"BROKEN").unwrap();
        assert!(store.verify(&hash1).is_err());

        // delete + re-put recovers the object
        store.delete(&hash1).unwrap();
        let hash2 = store.put(data).unwrap();
        assert_eq!(hash1.as_str(), hash2.as_str());
        store.verify(&hash2).unwrap();
    }

    #[test]
    fn empty_object_stored_correctly() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("objects")).unwrap();

        let hash = store.put(b"").unwrap();
        let data = store.get(&hash).unwrap();
        assert!(data.is_empty());
        store.verify(&hash).unwrap();
    }

    #[test]
    fn large_object_integrity() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("objects")).unwrap();

        let large = vec![0xABu8; 10 * 1024 * 1024]; // 10 MB
        let hash = store.put(&large).unwrap();
        let restored = store.get(&hash).unwrap();
        assert_eq!(restored.len(), large.len());
        assert_eq!(restored, large);
        store.verify(&hash).unwrap();
    }

    // ── Commit graph crash recovery ──────────────────────────────────────

    #[test]
    fn commit_graph_serialization_roundtrip_under_stress() {
        let mut graph = CommitGraph::new();

        for i in 0..100 {
            let mut snap = BTreeMap::new();
            snap.insert(format!("file_{i}.md"), format!("hash_{i}"));
            let parent = graph.head().map(|c| c.hash.clone());
            graph
                .commit(
                    &format!("user:agent_{}", i % 5),
                    &format!("commit {i}"),
                    snap,
                    parent.as_deref(),
                )
                .unwrap();
        }

        let json = graph.to_json().unwrap();
        let restored = CommitGraph::from_json(&json).unwrap();
        assert_eq!(restored.log(Some(200)).len(), 100);
        assert_eq!(restored.head().unwrap().hash, graph.head().unwrap().hash);
    }

    #[test]
    fn concurrent_commit_conflict_detection() {
        let mut graph = CommitGraph::new();

        let snap1: BTreeMap<String, String> = [("a.md".into(), "h1".into())].into_iter().collect();
        let c1_hash = graph
            .commit("user:alice", "first", snap1, None)
            .unwrap()
            .hash
            .clone();

        let snap2: BTreeMap<String, String> = [("b.md".into(), "h2".into())].into_iter().collect();
        let result = graph.commit("user:bob", "stale parent", snap2.clone(), None);
        assert!(result.is_err());

        let c2_parent = graph
            .commit("user:bob", "correct parent", snap2, Some(&c1_hash))
            .unwrap()
            .parent
            .clone();
        assert_eq!(c2_parent.as_deref(), Some(c1_hash.as_str()));
    }

    #[test]
    fn revert_to_nonexistent_commit_fails() {
        let mut graph = CommitGraph::new();
        let snap: BTreeMap<String, String> = [("a.md".into(), "h1".into())].into_iter().collect();
        graph.commit("user:alice", "first", snap, None).unwrap();

        let result = graph.revert(
            "deadbeef00000000000000000000000000000000000000000000000000000000",
            "user:alice",
        );
        assert!(result.is_err());
    }

    #[test]
    fn commit_graph_empty_json_roundtrip() {
        let graph = CommitGraph::new();
        let json = graph.to_json().unwrap();
        let restored = CommitGraph::from_json(&json).unwrap();
        assert!(restored.is_empty());
        assert!(restored.head().is_none());
    }

    // ── Audit log crash recovery ─────────────────────────────────────────

    #[test]
    fn audit_log_survives_reopen_with_tamper_detection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.ndjson");

        {
            let log = AuditLog::open(&path, true, true).unwrap();
            for i in 0..10 {
                log.record(
                    &format!("user:u{i}"),
                    AuditAction::FileWrite,
                    &format!("file_{i}.md"),
                    None,
                )
                .unwrap();
            }
        }

        {
            let log = AuditLog::open(&path, true, true).unwrap();
            log.verify_chain().unwrap();
            let entries = log.read_all().unwrap();
            assert_eq!(entries.len(), 10);
        }
    }

    #[test]
    fn audit_log_truncated_last_line_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.ndjson");

        {
            let log = AuditLog::open(&path, true, true).unwrap();
            log.record("user:alice", AuditAction::Commit, "a.md", None)
                .unwrap();
            log.record("user:bob", AuditAction::FileWrite, "b.md", None)
                .unwrap();
        }

        let content = std::fs::read_to_string(&path).unwrap();
        // Truncate enough to break the last JSON line
        let truncated = &content[..content.len() - 20];
        std::fs::write(&path, truncated).unwrap();

        // Truncated JSON line should cause a parse error
        let result = crate::audit::read_entries(&path);
        assert!(result.is_err(), "truncated NDJSON should fail to parse");
    }

    #[test]
    fn audit_log_tamper_middle_entry_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.ndjson");

        {
            let log = AuditLog::open(&path, true, true).unwrap();
            log.record("user:a", AuditAction::Commit, "x.md", None)
                .unwrap();
            log.record("user:b", AuditAction::FileWrite, "y.md", None)
                .unwrap();
            log.record("user:c", AuditAction::Revert, "z.md", None)
                .unwrap();
        }

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        let mut entry: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        entry["subject"] = serde_json::json!("user:evil");
        let tampered_line = serde_json::to_string(&entry).unwrap();
        let tampered = format!("{}\n{}\n{}\n", lines[0], tampered_line, lines[2]);
        std::fs::write(&path, tampered).unwrap();

        let entries = crate::audit::read_entries(&path).unwrap();
        let result = crate::audit::verify_hash_chain(&entries);
        assert!(result.is_err());
    }

    // ── Event log crash recovery ─────────────────────────────────────────

    #[test]
    fn event_log_survives_reopen_preserves_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.ndjson");

        {
            let log = EventLog::open(&path).unwrap();
            for i in 0..20 {
                log.append(
                    EventKind::FileStaged,
                    "ws_test",
                    &format!("user:u{i}"),
                    &format!("f{i}"),
                    None,
                )
                .unwrap();
            }
            assert_eq!(log.next_offset(), 20);
        }

        {
            let log = EventLog::open(&path).unwrap();
            assert_eq!(log.next_offset(), 20);
            log.append(EventKind::CommitCreated, "ws_test", "user:x", "c1", None)
                .unwrap();
            assert_eq!(log.next_offset(), 21);
        }
    }

    #[test]
    fn event_log_consumer_offset_persistence() {
        use crate::event_log::ConsumerOffset;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.ndjson");

        let log = EventLog::open(&path).unwrap();
        for i in 0..5 {
            log.append(
                EventKind::FileStaged,
                "ws_test",
                "user:a",
                &format!("f{i}"),
                None,
            )
            .unwrap();
        }

        let offset = ConsumerOffset::open(dir.path().join("indexer.offset")).unwrap();
        assert_eq!(offset.get().unwrap(), 0);

        offset.commit(3).unwrap();
        assert_eq!(offset.get().unwrap(), 3);

        let pending = log.read_from(3).unwrap();
        assert_eq!(pending.len(), 2);
    }

    // ── Backup/restore crash recovery ────────────────────────────────────

    #[test]
    fn backup_with_corrupted_object_detected_by_verify() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("store")).unwrap();
        let mut index = InodeIndex::new();
        let commit_graph = CommitGraph::new();
        let entity_graph = EntityGraph::new();

        let h = store.put(b"valid content").unwrap();
        index.set("test.md", h);

        let backup_dir = dir.path().join("backup");
        create_backup(&BackupParams {
            backup_dir: &backup_dir,
            workspace_id: "ws_test",
            object_store: &store,
            index: &index,
            commit_graph: &commit_graph,
            entity_graph: &entity_graph,
            audit_log_path: None,
            event_log_path: None,
        })
        .unwrap();

        let obj_files: Vec<_> = std::fs::read_dir(backup_dir.join("objects"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!obj_files.is_empty());
        std::fs::write(obj_files[0].path(), b"CORRUPTED").unwrap();

        let result = verify_backup(&backup_dir).unwrap();
        assert!(!result.is_ok());
        assert!(!result.corrupt_objects.is_empty());
    }

    #[test]
    fn backup_missing_manifest_fails_restore() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("empty_backup");
        std::fs::create_dir_all(&backup_dir).unwrap();

        let store = ObjectStore::open(dir.path().join("store")).unwrap();
        let result = restore_backup(&backup_dir, &store);
        assert!(result.is_err());
    }

    #[test]
    fn backup_restore_preserves_all_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("store")).unwrap();
        let mut index = InodeIndex::new();
        let mut commit_graph = CommitGraph::new();
        let entity_graph = EntityGraph::new();

        for i in 0..50 {
            let data = format!("content {i}");
            let h = store.put(data.as_bytes()).unwrap();
            index.set(format!("memories/mem_{i}.md"), h);
        }

        let snap: BTreeMap<String, String> = index
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_str().to_string()))
            .collect();
        commit_graph
            .commit("user:alice", "bulk write", snap, None)
            .unwrap();

        let backup_dir = dir.path().join("backup");
        create_backup(&BackupParams {
            backup_dir: &backup_dir,
            workspace_id: "ws_stress",
            object_store: &store,
            index: &index,
            commit_graph: &commit_graph,
            entity_graph: &entity_graph,
            audit_log_path: None,
            event_log_path: None,
        })
        .unwrap();

        let verify = verify_backup(&backup_dir).unwrap();
        assert!(verify.is_ok());
        assert_eq!(verify.object_count, 50);

        let restore_store = ObjectStore::open(dir.path().join("restore")).unwrap();
        let restored = restore_backup(&backup_dir, &restore_store).unwrap();
        assert_eq!(restored.index.len(), 50);
        assert!(!restored.commit_graph.is_empty());
        assert_eq!(restored.workspace_id, "ws_stress");

        for (path, hash) in index.iter() {
            let original = store.get(hash).unwrap();
            let restored_hash = restored.index.get(path).unwrap();
            let restored_data = restore_store.get(restored_hash).unwrap();
            assert_eq!(original, restored_data, "data mismatch for {path}");
        }
    }

    // ── Migration crash recovery ─────────────────────────────────────────

    #[test]
    fn migration_invalid_target_version_fails_gracefully() {
        let runner = MigrationRunner::default_chain();
        let result = runner.plan("memoryfs/v1", "memoryfs/v99");
        assert!(result.is_err());
    }

    #[test]
    fn migration_rollback_then_redo() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("store")).unwrap();
        let mut index = InodeIndex::new();

        let content = "---\nschema_version: memoryfs/v1\ntitle: Test\n---\nBody";
        let hash = store.put(content.as_bytes()).unwrap();
        index.set("memories/test.md", hash);

        let mut state = SchemaState::new("memoryfs/v1");
        let runner = MigrationRunner::default_chain();

        runner
            .migrate(&store, &mut index, &mut state, "memoryfs/v2")
            .unwrap();
        assert_eq!(state.version, "memoryfs/v2");

        runner.rollback(&store, &mut index, &mut state).unwrap();
        assert_eq!(state.version, "memoryfs/v1");

        runner
            .migrate(&store, &mut index, &mut state, "memoryfs/v2")
            .unwrap();
        assert_eq!(state.version, "memoryfs/v2");
        assert_eq!(state.applied.len(), 1);
    }

    // ── Index consistency ────────────────────────────────────────────────

    #[test]
    fn index_dangling_reference_detected() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("objects")).unwrap();
        let mut index = InodeIndex::new();

        let h = store.put(b"exists").unwrap();
        index.set("good.md", h);

        let fake_hash =
            ObjectHash::parse("0000000000000000000000000000000000000000000000000000000000000000")
                .unwrap();
        index.set("dangling.md", fake_hash);

        assert!(store.get(index.get("good.md").unwrap()).is_ok());
        assert!(store.get(index.get("dangling.md").unwrap()).is_err());
    }

    #[test]
    fn index_overwrite_preserves_old_objects() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("objects")).unwrap();
        let mut index = InodeIndex::new();

        let h1 = store.put(b"version 1").unwrap();
        index.set("file.md", h1.clone());

        let h2 = store.put(b"version 2").unwrap();
        index.set("file.md", h2);

        assert!(store.get(&h1).is_ok());
        let current = store.get(index.get("file.md").unwrap()).unwrap();
        assert_eq!(current, b"version 2");
    }

    // ── Run store resilience ─────────────────────────────────────────────

    #[test]
    fn run_store_many_concurrent_runs() {
        use crate::runs::{
            FinishRunParams, RunStatus, RunStore, StartRunParams, Trigger, TriggerKind,
        };

        let mut store = RunStore::new();
        let mut run_ids = Vec::new();

        for i in 0..100 {
            let run = store.start(StartRunParams {
                agent: format!("agent:worker_{}", i % 10),
                trigger: Trigger {
                    kind: TriggerKind::Test,
                    by: None,
                    trigger_ref: None,
                },
                author: "user:system".into(),
                session_id: None,
                model: None,
                tags: vec![],
            });
            run_ids.push(run.id);
        }

        assert_eq!(store.len(), 100);
        assert_eq!(store.active_count(), 100);

        for (i, id) in run_ids.iter().enumerate() {
            let status = if i % 3 == 0 {
                RunStatus::Failed
            } else {
                RunStatus::Succeeded
            };
            store
                .finish(
                    id,
                    FinishRunParams {
                        status,
                        finished_at: None,
                        artifacts: None,
                        metrics: None,
                        error: if i % 3 == 0 {
                            Some("test error".into())
                        } else {
                            None
                        },
                        proposed_memories: vec![],
                    },
                )
                .unwrap();
        }

        assert_eq!(store.active_count(), 0);
        assert_eq!(store.list_by_agent("agent:worker_0", 100).len(), 10);
    }

    // ── Large payload stress ─────────────────────────────────────────────

    #[test]
    fn commit_with_many_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("objects")).unwrap();
        let mut index = InodeIndex::new();
        let mut graph = CommitGraph::new();

        for i in 0..1000 {
            let data = format!("---\ntitle: Memory {i}\n---\nContent {i}");
            let h = store.put(data.as_bytes()).unwrap();
            index.set(format!("memories/mem_{i:04}.md"), h);
        }

        let snap: BTreeMap<String, String> = index
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_str().to_string()))
            .collect();

        let c_hash = graph
            .commit("user:alice", "bulk load 1000 files", snap, None)
            .unwrap()
            .hash
            .clone();
        assert!(c_hash.len() == 64);
        assert_eq!(graph.log(Some(10)).len(), 1);

        let json = graph.to_json().unwrap();
        let restored = CommitGraph::from_json(&json).unwrap();
        assert_eq!(restored.head().unwrap().hash, c_hash);
    }

    #[test]
    fn unicode_paths_and_content() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("objects")).unwrap();
        let mut index = InodeIndex::new();

        let paths = vec![
            "memories/日本語メモ.md",
            "memories/заметка.md",
            "memories/nota_español.md",
            "memories/émoji_🎯.md",
        ];

        for path in &paths {
            let content = format!("---\ntitle: {path}\n---\nContent for {path}");
            let h = store.put(content.as_bytes()).unwrap();
            index.set(*path, h);
        }

        assert_eq!(index.len(), 4);
        for path in &paths {
            let hash = index.get(path).unwrap();
            let data = store.get(hash).unwrap();
            let text = String::from_utf8(data).unwrap();
            assert!(text.contains(path));
        }
    }

    // ── Entity graph resilience ──────────────────────────────────────────

    #[test]
    fn entity_graph_bulk_operations() {
        let mut graph = EntityGraph::new();

        let mut ids = Vec::new();
        for i in 0..50 {
            let entity = graph
                .create_entity(
                    "ws_test",
                    crate::graph::EntityKind::Tool,
                    &format!("Entity {i}"),
                    vec![format!("entity_{i}")],
                    serde_json::json!({}),
                    vec![],
                )
                .unwrap();
            ids.push(entity.id.clone());
        }

        assert_eq!(graph.entity_count(), 50);

        for i in 0..49 {
            graph
                .link(
                    &ids[i].to_string(),
                    &ids[i + 1].to_string(),
                    crate::graph::Relation::Uses,
                    1.0,
                    None,
                )
                .unwrap();
        }

        let (entities, edges) = graph.neighbors(&ids[25].to_string(), 2, None).unwrap();
        assert!(entities.len() >= 2 || edges.len() >= 2);
    }
}
