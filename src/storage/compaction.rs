// Log compaction: reclaim space by copying only referenced chunks to a new log.
//
// Algorithm:
// 1. Walk all snapshots reachable from trunk (changeset parent chain)
// 2. Collect all blob hashes referenced by those snapshots
// 3. For each blob, collect chunk hashes
// 4. The set of referenced chunk hashes = chunks to keep
// 5. Create new chunks.log.compact, copy only referenced chunks
// 6. Rebuild index from new log
// 7. Atomic swap: rename compact → original
//
// The caller must ensure exclusive access (no concurrent writes).

use std::collections::HashSet;
use std::fs;

use crate::core::primitives::Hash;
use crate::storage::engine::StorageEngine;
use crate::storage::index::Index;
use crate::storage::log::AppendLog;

/// Statistics about a compaction run.
#[derive(Debug)]
pub struct CompactionStats {
    pub chunks_before: usize,
    pub chunks_after: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

impl StorageEngine {
    /// Compact the chunks log by copying only referenced chunks.
    ///
    /// Walks the changeset parent chain from trunk, collects every chunk hash
    /// that is still reachable through snapshots and blobs, then rewrites the
    /// chunks log keeping only those chunks. The in-memory pipeline index is
    /// rebuilt to match the new log.
    ///
    /// The caller must ensure no concurrent writes are happening.
    pub fn compact(&mut self) -> Result<CompactionStats, anyhow::Error> {
        // 1. Collect all referenced chunk hashes.
        let referenced_chunks = self.collect_referenced_chunks();

        // 2. Gather pre-compaction stats.
        let chunks_before = self.pipeline().index().len();
        let bytes_before = self.pipeline().log().size();

        // 3. Create a new compact log file next to the original.
        let original_path = self.pipeline().log().path().to_path_buf();
        let compact_path = original_path.with_extension("log.compact");

        // Remove any stale compact file from a previous failed attempt.
        if compact_path.exists() {
            fs::remove_file(&compact_path)?;
        }

        let mut compact_log = AppendLog::open(&compact_path)?;
        let mut compact_index = Index::new();

        // 4. Copy referenced chunks from the old log to the new log.
        //    We iterate the old log, decompress each entry to get its content
        //    hash, and if it's referenced, append the raw (still compressed)
        //    payload to the new log.
        for entry in self.pipeline().log().iter() {
            let (_offset, compressed_payload) = entry?;
            // Decompress to get the content hash (same as Pipeline::open).
            let decompressed = zstd::decode_all(compressed_payload.as_slice())?;
            let hash = Hash::from_bytes(&decompressed);

            if referenced_chunks.contains(&hash) {
                // Skip if we already wrote this chunk (dedup within the log).
                if compact_index.contains(&hash) {
                    continue;
                }

                let new_offset = compact_log.append(&compressed_payload)?;
                compact_index.insert(hash, new_offset, compressed_payload.len() as u32);
            }
        }

        // 5. Gather post-compaction stats.
        let chunks_after = compact_index.len();
        let bytes_after = compact_log.size();

        // 6. Atomic swap: rename compact → original.
        //    Drop the compact log handle before renaming.
        drop(compact_log);

        let backup_path = original_path.with_extension("log.old");
        if backup_path.exists() {
            fs::remove_file(&backup_path)?;
        }

        // Rename original → backup, compact → original.
        fs::rename(&original_path, &backup_path)?;
        fs::rename(&compact_path, &original_path)?;

        // Reopen the log from the new file and rebuild the index.
        let new_log = AppendLog::open(&original_path)?;
        let mut reopened_index = Index::new();
        for entry in new_log.iter() {
            let (offset, compressed) = entry?;
            let decompressed = zstd::decode_all(compressed.as_slice())?;
            let hash = Hash::from_bytes(&decompressed);
            reopened_index.insert(hash, offset, compressed.len() as u32);
        }
        self.pipeline_mut().replace(new_log, reopened_index);

        // Clean up the backup.
        if backup_path.exists() {
            let _ = fs::remove_file(&backup_path);
        }

        Ok(CompactionStats {
            chunks_before,
            chunks_after,
            bytes_before,
            bytes_after,
        })
    }

    /// Walk the changeset chain from trunk and collect every chunk hash
    /// that is reachable through snapshots → blobs → chunks.
    fn collect_referenced_chunks(&self) -> HashSet<Hash> {
        let mut referenced = HashSet::new();
        let mut blob_hashes: HashSet<Hash> = HashSet::new();

        // Walk trunk's changeset parent chain.
        if let Ok(Some(trunk_id)) = self.get_trunk() {
            let mut current = Some(trunk_id);
            while let Some(cs_id) = current {
                if let Ok(cs) = self.get_changeset(&cs_id) {
                    if let Ok(snapshot) = self.get_snapshot(&cs.snapshot) {
                        for blob_hash in snapshot.files.values() {
                            blob_hashes.insert(*blob_hash);
                        }
                    }
                    current = cs.parent;
                } else {
                    break;
                }
            }
        }

        // Include blobs referenced by workspace changesets.
        for ws in self.list_workspaces(true) {
            for cs_id in &ws.changesets {
                if let Ok(cs) = self.get_changeset(cs_id) {
                    if let Ok(snapshot) = self.get_snapshot(&cs.snapshot) {
                        for blob_hash in snapshot.files.values() {
                            blob_hashes.insert(*blob_hash);
                        }
                    }
                }
            }
        }

        // Resolve blob hashes → chunk hashes.
        for blob_hash in &blob_hashes {
            if let Ok(blob) = self.get_blob(blob_hash) {
                for chunk_hash in &blob.chunks {
                    referenced.insert(*chunk_hash);
                }
            }
        }

        referenced
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use tempfile::tempdir;

    use crate::core::primitives::*;
    use crate::storage::engine::StorageEngine;

    /// Helper: init an engine, store a file, create a snapshot + changeset,
    /// and set trunk. Returns (engine, tempdir, snapshot_id, file content).
    fn setup_with_trunk() -> (StorageEngine, tempfile::TempDir, Hash, Vec<u8>) {
        let dir = tempdir().unwrap();
        let mut engine = StorageEngine::init(dir.path()).unwrap();

        let content = b"fn main() { println!(\"hello pulse compaction test\"); }".to_vec();
        let info = engine.store_file(&content).unwrap();

        let mut files = BTreeMap::new();
        files.insert("src/main.rs".into(), info.blob.hash);
        let snap = Snapshot::new(files);
        let snap_id = engine.store_snapshot(&snap).unwrap();

        let cs = Changeset::new(
            None,
            snap_id,
            Utc::now(),
            Author::human("alice"),
            "initial commit".into(),
            vec!["src/main.rs".into()],
            None,
        );
        let cs_id = engine.store_changeset(&cs).unwrap();
        engine.set_trunk(&cs_id).unwrap();

        (engine, dir, snap_id, content)
    }

    // 1. Compact with no abandoned data: all chunks survive.
    #[test]
    fn compact_no_abandoned_data() {
        let (mut engine, _dir, _snap_id, _content) = setup_with_trunk();

        let stats = engine.compact().unwrap();

        assert!(stats.chunks_before > 0);
        assert_eq!(stats.chunks_after, stats.chunks_before);
        assert_eq!(stats.bytes_before, stats.bytes_after);
    }

    // 2. Compact after storing extra unreferenced data: unreferenced chunks removed.
    #[test]
    fn compact_removes_unreferenced_chunks() {
        let dir = tempdir().unwrap();
        let mut engine = StorageEngine::init(dir.path()).unwrap();

        // Store a file that will be referenced (via trunk).
        let referenced_content = b"this content is referenced by trunk";
        let ref_info = engine.store_file(referenced_content).unwrap();

        let mut files = BTreeMap::new();
        files.insert("keep.txt".into(), ref_info.blob.hash);
        let snap = Snapshot::new(files);
        let snap_id = engine.store_snapshot(&snap).unwrap();

        let cs = Changeset::new(
            None,
            snap_id,
            Utc::now(),
            Author::human("alice"),
            "keep this".into(),
            vec!["keep.txt".into()],
            None,
        );
        let cs_id = engine.store_changeset(&cs).unwrap();
        engine.set_trunk(&cs_id).unwrap();

        // Store another file that is NOT referenced by any snapshot/changeset.
        let orphan_content = b"this content is orphaned and should be removed";
        let orphan_info = engine.store_file(orphan_content).unwrap();
        assert!(orphan_info.stats.new_chunks > 0);

        let referenced_chunk_count = ref_info.blob.chunks.len();

        let stats = engine.compact().unwrap();

        assert!(
            stats.chunks_before > referenced_chunk_count,
            "should have more chunks before compaction than just the referenced ones"
        );
        assert_eq!(stats.chunks_after, referenced_chunk_count);
        assert!(stats.bytes_after < stats.bytes_before);
    }

    // 3. File content still readable after compaction.
    #[test]
    fn content_readable_after_compaction() {
        let (mut engine, _dir, snap_id, content) = setup_with_trunk();

        engine.compact().unwrap();

        let read_back = engine.read_file_by_path(&snap_id, "src/main.rs").unwrap();
        assert_eq!(read_back, content);
    }

    // 4. Compaction with empty repository (no trunk).
    #[test]
    fn compact_empty_repo() {
        let dir = tempdir().unwrap();
        let mut engine = StorageEngine::init(dir.path()).unwrap();

        let stats = engine.compact().unwrap();
        assert_eq!(stats.chunks_before, 0);
        assert_eq!(stats.chunks_after, 0);
        assert_eq!(stats.bytes_before, 0);
        assert_eq!(stats.bytes_after, 0);
    }

    // 5. Compaction preserves data across close/reopen.
    #[test]
    fn compact_survives_reopen() {
        let dir = tempdir().unwrap();
        let snap_id;
        let content = b"persistent after compaction";

        {
            let mut engine = StorageEngine::init(dir.path()).unwrap();
            let info = engine.store_file(content.as_slice()).unwrap();

            let mut files = BTreeMap::new();
            files.insert("data.txt".into(), info.blob.hash);
            let snap = Snapshot::new(files);
            snap_id = engine.store_snapshot(&snap).unwrap();

            let cs = Changeset::new(
                None,
                snap_id,
                Utc::now(),
                Author::human("bob"),
                "add data".into(),
                vec!["data.txt".into()],
                None,
            );
            let cs_id = engine.store_changeset(&cs).unwrap();
            engine.set_trunk(&cs_id).unwrap();

            // Store orphan data
            engine
                .store_file(b"orphan bytes that should be removed")
                .unwrap();

            engine.compact().unwrap();
        }

        // Reopen and verify
        let engine = StorageEngine::open(dir.path()).unwrap();
        let read_back = engine.read_file_by_path(&snap_id, "data.txt").unwrap();
        assert_eq!(read_back, content.as_slice());
    }
}
