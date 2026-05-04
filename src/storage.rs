//! Content-addressable object store backed by the filesystem.
//!
//! Objects are stored as `<root>/objects/<aa>/<bb>/<hex>` where `<hex>` is
//! the full SHA-256 digest. Deduplication is automatic — identical content
//! maps to the same hash and is stored once.
//!
//! An inode index maps workspace-relative paths to their current object hash,
//! enabling O(1) path→content lookup.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{MemoryFsError, Result};

/// SHA-256 hex digest (64 lowercase hex chars).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectHash(String);

impl ObjectHash {
    /// Compute the SHA-256 hash of `data`.
    pub fn of(data: &[u8]) -> Self {
        let digest = Sha256::digest(data);
        Self(hex::encode(digest))
    }

    /// Parse from a hex string, validating format.
    pub fn parse(s: &str) -> Result<Self> {
        if s.len() != 64 || !s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
            return Err(MemoryFsError::Validation(format!(
                "object hash must be 64 lowercase hex chars, got {s:?}"
            )));
        }
        Ok(Self(s.to_string()))
    }

    /// The hex string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Subdirectory path components: `("aa", "bb")` from the first 4 chars.
    fn shard(&self) -> (&str, &str) {
        (&self.0[..2], &self.0[2..4])
    }
}

impl std::fmt::Display for ObjectHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Content-addressable object store on the local filesystem.
///
/// Layout: `<root>/objects/<aa>/<bb>/<full-hex>`
pub struct ObjectStore {
    root: PathBuf,
}

impl ObjectStore {
    /// Open (or create) an object store rooted at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let objects_dir = root.join("objects");
        std::fs::create_dir_all(&objects_dir).map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!("failed to create objects dir: {e}"))
        })?;
        Ok(Self { root })
    }

    /// Store `data` and return its hash. Idempotent — if the object already
    /// exists, the write is skipped (dedup).
    pub fn put(&self, data: &[u8]) -> Result<ObjectHash> {
        let hash = ObjectHash::of(data);
        let path = self.object_path(&hash);

        if path.exists() {
            return Ok(hash);
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MemoryFsError::Internal(anyhow::anyhow!("failed to create shard dir: {e}"))
            })?;
        }

        // Write to temp file then rename for atomicity.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, data)
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("failed to write object: {e}")))?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            MemoryFsError::Internal(anyhow::anyhow!("failed to rename object: {e}"))
        })?;

        Ok(hash)
    }

    /// Retrieve the bytes for `hash`. Returns `NotFound` if absent.
    pub fn get(&self, hash: &ObjectHash) -> Result<Vec<u8>> {
        let path = self.object_path(hash);
        std::fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => MemoryFsError::NotFound(format!("object {hash}")),
            _ => MemoryFsError::Internal(anyhow::anyhow!("failed to read object {hash}: {e}")),
        })
    }

    /// Check whether an object exists without reading it.
    pub fn exists(&self, hash: &ObjectHash) -> bool {
        self.object_path(hash).exists()
    }

    /// Verify that the stored bytes match `hash`. Returns an error if the
    /// object is missing or corrupted.
    pub fn verify(&self, hash: &ObjectHash) -> Result<()> {
        let data = self.get(hash)?;
        let actual = ObjectHash::of(&data);
        if actual != *hash {
            return Err(MemoryFsError::Conflict(format!(
                "object {hash} is corrupted: actual hash is {actual}"
            )));
        }
        Ok(())
    }

    /// Delete an object by hash. No-op if already absent.
    pub fn delete(&self, hash: &ObjectHash) -> Result<()> {
        let path = self.object_path(hash);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(MemoryFsError::Internal(anyhow::anyhow!(
                "failed to delete object {hash}: {e}"
            ))),
        }
    }

    /// Root directory of this store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object_path(&self, hash: &ObjectHash) -> PathBuf {
        let (a, b) = hash.shard();
        self.root
            .join("objects")
            .join(a)
            .join(b)
            .join(hash.as_str())
    }
}

/// In-memory inode index: maps workspace-relative paths to object hashes.
///
/// This is the mutable layer that tracks which file lives at which path.
/// The object store itself is immutable (content-addressed).
pub struct InodeIndex {
    entries: HashMap<String, ObjectHash>,
}

impl InodeIndex {
    /// Create an empty index.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Insert or update a path→hash mapping.
    pub fn set(&mut self, path: impl Into<String>, hash: ObjectHash) {
        self.entries.insert(path.into(), hash);
    }

    /// Look up the hash for a path.
    pub fn get(&self, path: &str) -> Option<&ObjectHash> {
        self.entries.get(path)
    }

    /// Remove a path from the index.
    pub fn remove(&mut self, path: &str) -> Option<ObjectHash> {
        self.entries.remove(path)
    }

    /// List all paths in the index, sorted.
    pub fn paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self.entries.keys().map(|s| s.as_str()).collect();
        paths.sort();
        paths
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over all (path, hash) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ObjectHash)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl Default for InodeIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_store() -> (tempfile::TempDir, ObjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn put_get_roundtrip() {
        let (_dir, store) = temp_store();
        let data = b"hello, world!";
        let hash = store.put(data).unwrap();
        let got = store.get(&hash).unwrap();
        assert_eq!(got, data);
    }

    #[test]
    fn put_is_idempotent() {
        let (_dir, store) = temp_store();
        let data = b"duplicate content";
        let h1 = store.put(data).unwrap();
        let h2 = store.put(data).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn dedup_same_content() {
        let (_dir, store) = temp_store();
        let data = b"same bytes";
        let h1 = store.put(data).unwrap();
        let h2 = store.put(data).unwrap();
        assert_eq!(h1, h2);
        // Only one file on disk
        let path = store.object_path(&h1);
        assert!(path.exists());
    }

    #[test]
    fn different_content_different_hash() {
        let (_dir, store) = temp_store();
        let h1 = store.put(b"aaa").unwrap();
        let h2 = store.put(b"bbb").unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn get_missing_returns_not_found() {
        let (_dir, store) = temp_store();
        let hash = ObjectHash::of(b"does not exist");
        let err = store.get(&hash).unwrap_err();
        assert_eq!(err.api_code(), "NOT_FOUND");
    }

    #[test]
    fn verify_intact_object() {
        let (_dir, store) = temp_store();
        let hash = store.put(b"verify me").unwrap();
        store.verify(&hash).unwrap();
    }

    #[test]
    fn verify_corrupted_object() {
        let (_dir, store) = temp_store();
        let hash = store.put(b"original").unwrap();
        // Corrupt the file
        let path = store.object_path(&hash);
        fs::write(&path, b"corrupted!").unwrap();
        let err = store.verify(&hash).unwrap_err();
        assert_eq!(err.api_code(), "CONFLICT");
    }

    #[test]
    fn delete_existing_object() {
        let (_dir, store) = temp_store();
        let hash = store.put(b"delete me").unwrap();
        assert!(store.exists(&hash));
        store.delete(&hash).unwrap();
        assert!(!store.exists(&hash));
    }

    #[test]
    fn delete_missing_is_noop() {
        let (_dir, store) = temp_store();
        let hash = ObjectHash::of(b"never stored");
        store.delete(&hash).unwrap(); // no error
    }

    #[test]
    fn exists_check() {
        let (_dir, store) = temp_store();
        let hash = store.put(b"exists check").unwrap();
        assert!(store.exists(&hash));
        let missing = ObjectHash::of(b"nope");
        assert!(!store.exists(&missing));
    }

    #[test]
    fn object_hash_parse_valid() {
        let hex = "a".repeat(64);
        ObjectHash::parse(&hex).unwrap();
    }

    #[test]
    fn object_hash_parse_invalid_length() {
        assert!(ObjectHash::parse("abc").is_err());
    }

    #[test]
    fn object_hash_parse_invalid_chars() {
        let hex = "G".repeat(64);
        assert!(ObjectHash::parse(&hex).is_err());
    }

    // -- InodeIndex tests --

    #[test]
    fn inode_set_get() {
        let mut idx = InodeIndex::new();
        let hash = ObjectHash::of(b"file content");
        idx.set("memory/user/prefs.md", hash.clone());
        assert_eq!(idx.get("memory/user/prefs.md"), Some(&hash));
        assert_eq!(idx.get("nonexistent"), None);
    }

    #[test]
    fn inode_remove() {
        let mut idx = InodeIndex::new();
        let hash = ObjectHash::of(b"data");
        idx.set("a.md", hash);
        assert_eq!(idx.len(), 1);
        idx.remove("a.md");
        assert_eq!(idx.len(), 0);
        assert!(idx.is_empty());
    }

    #[test]
    fn inode_paths_sorted() {
        let mut idx = InodeIndex::new();
        idx.set("z.md", ObjectHash::of(b"z"));
        idx.set("a.md", ObjectHash::of(b"a"));
        idx.set("m.md", ObjectHash::of(b"m"));
        assert_eq!(idx.paths(), vec!["a.md", "m.md", "z.md"]);
    }

    #[test]
    fn inode_overwrite() {
        let mut idx = InodeIndex::new();
        let h1 = ObjectHash::of(b"v1");
        let h2 = ObjectHash::of(b"v2");
        idx.set("file.md", h1);
        idx.set("file.md", h2.clone());
        assert_eq!(idx.get("file.md"), Some(&h2));
        assert_eq!(idx.len(), 1);
    }
}
