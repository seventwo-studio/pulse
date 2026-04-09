// In-memory hash index over append-only log
//
// Maps BLAKE3 content hashes to (offset, length) pairs in the log file,
// enabling O(1) lookup of any stored payload by its content hash.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;

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

    /// Persist the index to a binary file.
    ///
    /// Format: `[8-byte entry count LE][entries...][32-byte BLAKE3 checksum]`
    /// Each entry: `[32-byte hash][8-byte offset LE][4-byte length LE]` = 44 bytes.
    pub fn persist(&self, path: &Path) -> Result<(), LogError> {
        let mut buf: Vec<u8> =
            Vec::with_capacity(8 + self.entries.len() * 44 + 32);

        // Entry count
        buf.extend_from_slice(&(self.entries.len() as u64).to_le_bytes());

        // Entries (deterministic order: sort by hash bytes)
        let mut sorted: Vec<_> = self.entries.iter().collect();
        sorted.sort_by_key(|(h, _)| h.0);

        for (hash, (offset, length)) in &sorted {
            buf.extend_from_slice(hash.as_bytes());
            buf.extend_from_slice(&offset.to_le_bytes());
            buf.extend_from_slice(&length.to_le_bytes());
        }

        // BLAKE3 checksum of everything written so far
        let checksum = blake3::hash(&buf);
        buf.extend_from_slice(checksum.as_bytes());

        let mut file = fs::File::create(path)?;
        file.write_all(&buf)?;
        file.sync_all()?;

        Ok(())
    }

    /// Load index from a binary file. Returns `None` if the file is missing or corrupt.
    pub fn load(path: &Path) -> Option<Self> {
        let data = fs::read(path).ok()?;

        // Minimum size: 8 (count) + 32 (checksum) = 40 bytes
        if data.len() < 40 {
            return None;
        }

        let (payload, stored_checksum) = data.split_at(data.len() - 32);
        let expected = blake3::hash(payload);
        if expected.as_bytes() != stored_checksum {
            return None;
        }

        let count = u64::from_le_bytes(payload[..8].try_into().ok()?) as usize;

        // Validate expected size: 8 + count * 44
        if payload.len() != 8 + count * 44 {
            return None;
        }

        let mut index = Self::new();
        let entries_data = &payload[8..];

        for i in 0..count {
            let base = i * 44;
            let hash_bytes: [u8; 32] = entries_data[base..base + 32].try_into().ok()?;
            let offset = u64::from_le_bytes(
                entries_data[base + 32..base + 40].try_into().ok()?,
            );
            let length = u32::from_le_bytes(
                entries_data[base + 40..base + 44].try_into().ok()?,
            );
            index.insert(Hash::from_slice(&hash_bytes), offset, length);
        }

        Some(index)
    }

    /// Scan new entries from a log starting at the given byte offset.
    ///
    /// Used for catch-up after loading a persisted index: only processes
    /// entries appended after the index was last persisted.
    pub fn catch_up(&mut self, log: &AppendLog, from_offset: u64) -> Result<(), LogError> {
        for entry in log.iter_from(from_offset) {
            let (offset, payload) = entry?;
            let hash = Hash::from_bytes(&payload);
            self.insert(hash, offset, payload.len() as u32);
        }
        Ok(())
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
    fn persist_load_roundtrip() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("test.log");
        let idx_path = dir.path().join("test.idx");

        let payloads: Vec<&[u8]> = vec![b"alpha", b"bravo", b"charlie"];

        let mut log = AppendLog::open(&log_path).unwrap();
        for payload in &payloads {
            log.append(payload).unwrap();
        }

        let original = Index::rebuild(&log).unwrap();
        original.persist(&idx_path).unwrap();

        let loaded = Index::load(&idx_path).expect("should load persisted index");
        assert_eq!(loaded.len(), original.len());

        for payload in &payloads {
            let hash = Hash::from_bytes(payload);
            assert_eq!(loaded.get(&hash), original.get(&hash));
        }
    }

    #[test]
    fn load_corrupt_returns_none() {
        let dir = tempdir().unwrap();
        let idx_path = dir.path().join("corrupt.idx");

        // Write some garbage
        std::fs::write(&idx_path, b"this is not a valid index file").unwrap();
        assert!(Index::load(&idx_path).is_none());
    }

    #[test]
    fn load_corrupt_checksum_returns_none() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("test.log");
        let idx_path = dir.path().join("test.idx");

        let mut log = AppendLog::open(&log_path).unwrap();
        log.append(b"data").unwrap();

        let index = Index::rebuild(&log).unwrap();
        index.persist(&idx_path).unwrap();

        // Corrupt a byte in the middle of the file
        let mut data = std::fs::read(&idx_path).unwrap();
        data[10] ^= 0xFF;
        std::fs::write(&idx_path, &data).unwrap();

        assert!(Index::load(&idx_path).is_none());
    }

    #[test]
    fn catch_up_after_persist() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("test.log");
        let idx_path = dir.path().join("test.idx");

        let mut log = AppendLog::open(&log_path).unwrap();
        log.append(b"first").unwrap();
        log.append(b"second").unwrap();

        let index = Index::rebuild(&log).unwrap();
        let saved_size = log.size();
        index.persist(&idx_path).unwrap();

        // Append more entries after persisting
        log.append(b"third").unwrap();
        log.append(b"fourth").unwrap();

        // Load and catch up
        let mut loaded = Index::load(&idx_path).expect("should load");
        assert_eq!(loaded.len(), 2);

        loaded.catch_up(&log, saved_size).unwrap();
        assert_eq!(loaded.len(), 4);

        // Verify all entries are present
        for payload in &[b"first" as &[u8], b"second", b"third", b"fourth"] {
            let hash = Hash::from_bytes(payload);
            assert!(loaded.contains(&hash), "missing hash for {:?}", payload);
        }
    }

    #[test]
    fn load_missing_file_returns_none() {
        let dir = tempdir().unwrap();
        let idx_path = dir.path().join("nonexistent.idx");
        assert!(Index::load(&idx_path).is_none());
    }

    #[test]
    fn persist_load_empty_index() {
        let dir = tempdir().unwrap();
        let idx_path = dir.path().join("empty.idx");

        let index = Index::new();
        index.persist(&idx_path).unwrap();

        let loaded = Index::load(&idx_path).expect("should load empty index");
        assert!(loaded.is_empty());
        assert_eq!(loaded.len(), 0);
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
