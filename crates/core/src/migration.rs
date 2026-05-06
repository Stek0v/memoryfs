//! Schema migration runner — applies migrations on startup and supports
//! best-effort rollback.
//!
//! Migrations transform workspace data from one schema version to the next.
//! Each migration is a pure function `(data) → Result<data>` that can be
//! tested against golden fixtures.
//!
//! See `04-tasks-dod.md` Phase 7 (task 7.2).

use serde::{Deserialize, Serialize};

use crate::error::{MemoryFsError, Result};
use crate::storage::{InodeIndex, ObjectStore};

/// Current schema version.
pub const CURRENT_VERSION: &str = "memoryfs/v1";

/// A migration step from one version to the next.
pub struct Migration {
    /// Source version (e.g. "memoryfs/v1").
    pub from: &'static str,
    /// Target version (e.g. "memoryfs/v2").
    pub to: &'static str,
    /// Forward migration function.
    pub up: fn(&str) -> Result<String>,
    /// Rollback migration function (best effort).
    pub down: Option<fn(&str) -> Result<String>>,
}

/// Tracks which schema version a workspace is on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaState {
    /// Current schema version.
    pub version: String,
    /// History of applied migrations.
    pub applied: Vec<AppliedMigration>,
}

/// Record of a single applied migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedMigration {
    /// Source version.
    pub from: String,
    /// Target version.
    pub to: String,
    /// When the migration was applied.
    pub applied_at: String,
}

impl SchemaState {
    /// Create initial state at the given version.
    pub fn new(version: &str) -> Self {
        Self {
            version: version.into(),
            applied: Vec::new(),
        }
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("serialize schema state: {e}")))
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json)
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("parse schema state: {e}")))
    }
}

/// Registry of all known migrations.
pub struct MigrationRunner {
    migrations: Vec<Migration>,
}

impl MigrationRunner {
    /// Create a runner with the given migrations.
    pub fn new(migrations: Vec<Migration>) -> Self {
        Self { migrations }
    }

    /// Create a runner with the built-in migration chain.
    pub fn default_chain() -> Self {
        Self::new(vec![Migration {
            from: "memoryfs/v1",
            to: "memoryfs/v2",
            up: migrate_v1_to_v2,
            down: Some(migrate_v2_to_v1),
        }])
    }

    /// Find the migration path from current to target version.
    pub fn plan(&self, from: &str, to: &str) -> Result<Vec<&Migration>> {
        let mut path = Vec::new();
        let mut current = from.to_string();

        while current != to {
            let step = self
                .migrations
                .iter()
                .find(|m| m.from == current)
                .ok_or_else(|| {
                    MemoryFsError::Validation(format!("no migration path from {current} to {to}"))
                })?;
            path.push(step);
            current = step.to.to_string();

            if path.len() > 100 {
                return Err(MemoryFsError::Validation(
                    "migration chain too long (possible cycle)".into(),
                ));
            }
        }

        Ok(path)
    }

    /// Apply all pending migrations to bring workspace to `target_version`.
    /// Returns the number of migrations applied.
    pub fn migrate(
        &self,
        store: &ObjectStore,
        index: &mut InodeIndex,
        state: &mut SchemaState,
        target_version: &str,
    ) -> Result<usize> {
        if state.version == target_version {
            return Ok(0);
        }

        let plan = self.plan(&state.version, target_version)?;
        let mut applied = 0;

        for step in plan {
            apply_migration(store, index, step.up)?;

            state.applied.push(AppliedMigration {
                from: step.from.into(),
                to: step.to.into(),
                applied_at: chrono::Utc::now().to_rfc3339(),
            });
            state.version = step.to.into();
            applied += 1;
        }

        Ok(applied)
    }

    /// Rollback the last applied migration (best effort).
    pub fn rollback(
        &self,
        store: &ObjectStore,
        index: &mut InodeIndex,
        state: &mut SchemaState,
    ) -> Result<()> {
        let last = state
            .applied
            .last()
            .ok_or_else(|| MemoryFsError::Validation("no migrations to rollback".into()))?;

        let step = self
            .migrations
            .iter()
            .find(|m| m.from == last.from && m.to == last.to)
            .ok_or_else(|| {
                MemoryFsError::Validation(format!(
                    "migration {} → {} not found in registry",
                    last.from, last.to
                ))
            })?;

        let down = step.down.ok_or_else(|| {
            MemoryFsError::Validation(format!(
                "migration {} → {} does not support rollback",
                step.from, step.to
            ))
        })?;

        apply_migration(store, index, down)?;
        state.version = last.from.clone();
        state.applied.pop();

        Ok(())
    }
}

/// Apply a migration function to all files in the index.
fn apply_migration(
    store: &ObjectStore,
    index: &mut InodeIndex,
    transform: fn(&str) -> Result<String>,
) -> Result<()> {
    let paths: Vec<String> = index.paths().iter().map(|p| p.to_string()).collect();

    for path in paths {
        let hash = match index.get(&path) {
            Some(h) => h.clone(),
            None => continue,
        };

        let data = store.get(&hash)?;
        let content = String::from_utf8_lossy(&data);
        let transformed = transform(&content)?;

        if transformed != content.as_ref() {
            let new_hash = store.put(transformed.as_bytes())?;
            index.set(path, new_hash);
        }
    }

    Ok(())
}

// ── Built-in migrations ─────────────────────────────────────────────────

fn migrate_v1_to_v2(content: &str) -> Result<String> {
    Ok(content.replace("schema_version: memoryfs/v1", "schema_version: memoryfs/v2"))
}

fn migrate_v2_to_v1(content: &str) -> Result<String> {
    Ok(content.replace("schema_version: memoryfs/v2", "schema_version: memoryfs/v1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> (tempfile::TempDir, ObjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("store")).unwrap();
        (dir, store)
    }

    #[test]
    fn schema_state_roundtrip() {
        let state = SchemaState::new("memoryfs/v1");
        let json = state.to_json().unwrap();
        let restored = SchemaState::from_json(&json).unwrap();
        assert_eq!(restored.version, "memoryfs/v1");
        assert!(restored.applied.is_empty());
    }

    #[test]
    fn plan_direct_path() {
        let runner = MigrationRunner::default_chain();
        let plan = runner.plan("memoryfs/v1", "memoryfs/v2").unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].from, "memoryfs/v1");
        assert_eq!(plan[0].to, "memoryfs/v2");
    }

    #[test]
    fn plan_no_path() {
        let runner = MigrationRunner::default_chain();
        let result = runner.plan("memoryfs/v1", "memoryfs/v99");
        assert!(result.is_err());
    }

    #[test]
    fn plan_already_at_target() {
        let runner = MigrationRunner::default_chain();
        let plan = runner.plan("memoryfs/v2", "memoryfs/v2").unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn migrate_v1_to_v2_transforms_content() {
        let (_dir, store) = test_store();
        let mut index = InodeIndex::new();

        let content = "---\nschema_version: memoryfs/v1\ntitle: Test\n---\nBody";
        let hash = store.put(content.as_bytes()).unwrap();
        index.set("memories/test.md", hash);

        let mut state = SchemaState::new("memoryfs/v1");
        let runner = MigrationRunner::default_chain();
        let applied = runner
            .migrate(&store, &mut index, &mut state, "memoryfs/v2")
            .unwrap();

        assert_eq!(applied, 1);
        assert_eq!(state.version, "memoryfs/v2");
        assert_eq!(state.applied.len(), 1);

        let new_hash = index.get("memories/test.md").unwrap();
        let new_data = store.get(new_hash).unwrap();
        let new_content = String::from_utf8(new_data).unwrap();
        assert!(new_content.contains("schema_version: memoryfs/v2"));
        assert!(!new_content.contains("schema_version: memoryfs/v1"));
    }

    #[test]
    fn migrate_noop_when_already_current() {
        let (_dir, store) = test_store();
        let mut index = InodeIndex::new();
        let mut state = SchemaState::new("memoryfs/v2");

        let runner = MigrationRunner::default_chain();
        let applied = runner
            .migrate(&store, &mut index, &mut state, "memoryfs/v2")
            .unwrap();

        assert_eq!(applied, 0);
    }

    #[test]
    fn rollback_v2_to_v1() {
        let (_dir, store) = test_store();
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
        assert!(state.applied.is_empty());

        let hash = index.get("memories/test.md").unwrap();
        let data = store.get(hash).unwrap();
        let rolled_back = String::from_utf8(data).unwrap();
        assert!(rolled_back.contains("schema_version: memoryfs/v1"));
    }

    #[test]
    fn rollback_with_no_history_fails() {
        let (_dir, store) = test_store();
        let mut index = InodeIndex::new();
        let mut state = SchemaState::new("memoryfs/v1");
        let runner = MigrationRunner::default_chain();

        let result = runner.rollback(&store, &mut index, &mut state);
        assert!(result.is_err());
    }

    #[test]
    fn migrate_preserves_non_versioned_files() {
        let (_dir, store) = test_store();
        let mut index = InodeIndex::new();

        let versioned = "---\nschema_version: memoryfs/v1\n---\nV1 content";
        let plain = "Just a plain file without schema_version";

        let h1 = store.put(versioned.as_bytes()).unwrap();
        let h2 = store.put(plain.as_bytes()).unwrap();
        index.set("memories/versioned.md", h1);
        index.set("docs/plain.md", h2.clone());

        let mut state = SchemaState::new("memoryfs/v1");
        let runner = MigrationRunner::default_chain();
        runner
            .migrate(&store, &mut index, &mut state, "memoryfs/v2")
            .unwrap();

        let plain_hash = index.get("docs/plain.md").unwrap();
        assert_eq!(plain_hash.as_str(), h2.as_str());
    }

    #[test]
    fn multi_step_migration() {
        let runner = MigrationRunner::new(vec![
            Migration {
                from: "memoryfs/v1",
                to: "memoryfs/v2",
                up: |c| Ok(c.replace("v1", "v2")),
                down: Some(|c| Ok(c.replace("v2", "v1"))),
            },
            Migration {
                from: "memoryfs/v2",
                to: "memoryfs/v3",
                up: |c| Ok(c.replace("v2", "v3")),
                down: Some(|c| Ok(c.replace("v3", "v2"))),
            },
        ]);

        let plan = runner.plan("memoryfs/v1", "memoryfs/v3").unwrap();
        assert_eq!(plan.len(), 2);

        let (_dir, store) = test_store();
        let mut index = InodeIndex::new();
        let h = store.put(b"version: v1").unwrap();
        index.set("test.md", h);

        let mut state = SchemaState::new("memoryfs/v1");
        let applied = runner
            .migrate(&store, &mut index, &mut state, "memoryfs/v3")
            .unwrap();
        assert_eq!(applied, 2);
        assert_eq!(state.version, "memoryfs/v3");

        let hash = index.get("test.md").unwrap();
        let data = store.get(hash).unwrap();
        assert_eq!(String::from_utf8(data).unwrap(), "version: v3");
    }
}
