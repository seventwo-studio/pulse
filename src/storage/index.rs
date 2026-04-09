// In-memory hash index over append-only log
//
// Maps BLAKE3 content hashes to (offset, length) pairs in the log file,
// enabling O(1) lookup of any stored payload by its content hash.

use std::collections::HashMap;

use crate::core::primitives::Hash;

use super::log::{AppendLog, LogError};

/// In-memory index mapping content hashes to their position in an append-only log.
///
/// Each entry stores the byte offset where the log frame starts and the payload
/// length, which is enough to read the entry back via `AppendLog::read_at`.
pub struct Index {
    entries: HashMap<Hash, (u64, u32)>,
}

impl Index {
    /// Create an empty index.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Insert a mapping. If the hash already exists, overwrites (idempotent).
    pub fn insert(&mut self, hash: Hash, offset: u64, length: u32) {
        self.entries.insert(hash, (offset, length));
    }

    /// Look up a hash. Returns (offset, length) if found.
    pub fn get(&self, hash: &Hash) -> Option<(u64, u32)> {
        self.entries.get(hash).copied()
    }

    /// Check if a hash exists in the index.
    pub fn contains(&self, hash: &Hash) -> bool {
        self.entries.contains_key(hash)
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Rebuild index from an append-only log.
    ///
    /// Iterates every valid entry in the log, computes its BLAKE3 hash, and
    /// inserts a mapping from that hash to `(offset, payload_length)`.
    pub fn rebuild(log: &AppendLog) -> Result<Self, LogError> {
        let mut index = Self::new();

        for entry in log.iter() {
            let (offset, payload) = entry?;
            let hash = Hash::from_bytes(&payload);
            index.insert(hash, offset, payload.len() as u32);
        }

        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn rebuild_from_log() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");

        let payloads: Vec<&[u8]> = vec![
            b"alpha",
            b"bravo",
            b"charlie",
            b"delta",
            b"echo",
        ];

        let mut log = AppendLog::open(&path).unwrap();
        let mut expected: Vec<(Hash, u64, u32)> = Vec::new();

        for payload in &payloads {
            let offset = log.append(payload).unwrap();
            let hash = Hash::from_bytes(payload);
            expected.push((hash, offset, payload.len() as u32));
        }

        let index = Index::rebuild(&log).unwrap();

        assert_eq!(index.len(), 5);

        for (hash, offset, length) in &expected {
            let (got_offset, got_length) = index.get(hash).expect("hash should be in index");
            assert_eq!(got_offset, *offset);
            assert_eq!(got_length, *length);
        }
    }

    #[test]
    fn dedup_idempotent_overwrite() {
        let mut index = Index::new();
        let hash = Hash::from_bytes(b"same content");

        index.insert(hash, 100, 50);
        assert_eq!(index.get(&hash), Some((100, 50)));

        // Second insert with different offset overwrites
        index.insert(hash, 200, 60);
        assert_eq!(index.get(&hash), Some((200, 60)));

        // Only one entry in the map
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn missing_key_returns_none() {
        let index = Index::new();
        let unknown = Hash::from_bytes(b"not stored");
        assert_eq!(index.get(&unknown), None);
    }

    #[test]
    fn rebuild_empty_log() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.log");

        let log = AppendLog::open(&path).unwrap();
        let index = Index::rebuild(&log).unwrap();

        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn contains_known_and_unknown() {
        let mut index = Index::new();
        let known = Hash::from_bytes(b"known");
        let unknown = Hash::from_bytes(b"unknown");

        index.insert(known, 0, 5);

        assert!(index.contains(&known));
        assert!(!index.contains(&unknown));
    }

    #[test]
    fn len_and_is_empty() {
        let mut index = Index::new();
        assert_eq!(index.len(), 0);
        assert!(index.is_empty());

        index.insert(Hash::from_bytes(b"one"), 0, 3);
        assert_eq!(index.len(), 1);
        assert!(!index.is_empty());

        index.insert(Hash::from_bytes(b"two"), 10, 3);
        assert_eq!(index.len(), 2);
        assert!(!index.is_empty());

        // Re-inserting same hash doesn't increase count
        index.insert(Hash::from_bytes(b"one"), 20, 3);
        assert_eq!(index.len(), 2);
    }
}
