// Content pipeline: chunk -> hash -> compress -> store
//
// Glue layer that takes raw file bytes and stores them as deduplicated,
// compressed chunks in the append-only log.

use std::io;

use thiserror::Error;

use crate::core::primitives::{Blob, Hash};

use super::chunker;
use super::index::Index;
use super::log::{AppendLog, LogError};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("log error: {0}")]
    Log(#[from] LogError),
    #[error("compression error: {0}")]
    Compress(#[from] io::Error),
    #[error("object not found: {hash}")]
    NotFound { hash: Hash },
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Stats about a store operation.
#[derive(Debug, Clone)]
pub struct StoreStats {
    pub new_chunks: usize,
    pub reused_chunks: usize,
}

/// Result of storing a single file.
#[derive(Debug, Clone)]
pub struct BlobInfo {
    pub blob: Blob,
    pub stats: StoreStats,
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

pub struct Pipeline {
    log: AppendLog,
    index: Index,
}

impl Pipeline {
    /// Create a new pipeline with the given log and index.
    pub fn new(log: AppendLog, index: Index) -> Self {
        Self { log, index }
    }

    /// Open a pipeline from an existing chunks log, rebuilding the index.
    ///
    /// Unlike `Index::rebuild` (which hashes raw log payloads), this method
    /// decompresses each stored chunk first, so the index keys match the
    /// uncompressed content hashes used by `store_file` / `read_blob`.
    pub fn open(log: AppendLog) -> Result<Self, PipelineError> {
        let mut index = Index::new();

        for entry in log.iter() {
            let (offset, compressed) = entry?;
            let decompressed = zstd::decode_all(compressed.as_slice())?;
            let hash = Hash::from_bytes(&decompressed);
            index.insert(hash, offset, compressed.len() as u32);
        }

        Ok(Self { log, index })
    }

    /// Store file content, returning blob info with dedup stats.
    ///
    /// 1. Chunk the content using the structural chunker.
    /// 2. For each chunk, compute its BLAKE3 hash.
    ///    - If the hash already exists in the index, skip (dedup).
    ///    - Otherwise compress with zstd level 3, append to the log, and
    ///      insert into the index.
    /// 3. Build a `Blob` with the ordered chunk hashes and the overall content hash.
    pub fn store_file(&mut self, content: &[u8]) -> Result<BlobInfo, PipelineError> {
        let raw_chunks = chunker::chunk(content);

        let mut chunk_hashes: Vec<Hash> = Vec::with_capacity(raw_chunks.len());
        let mut new_chunks: usize = 0;
        let mut reused_chunks: usize = 0;

        for chunk_bytes in &raw_chunks {
            let hash = Hash::from_bytes(chunk_bytes);
            chunk_hashes.push(hash);

            if self.index.contains(&hash) {
                reused_chunks += 1;
            } else {
                let compressed = zstd::encode_all(chunk_bytes.as_slice(), 3)?;
                let offset = self.log.append(&compressed)?;
                self.index.insert(hash, offset, compressed.len() as u32);
                new_chunks += 1;
            }
        }

        // The blob hash is the BLAKE3 of the original file content (all chunks
        // concatenated in order, i.e. the original bytes).
        let blob_hash = Hash::from_bytes(content);

        let blob = Blob {
            hash: blob_hash,
            chunks: chunk_hashes,
        };

        Ok(BlobInfo {
            blob,
            stats: StoreStats {
                new_chunks,
                reused_chunks,
            },
        })
    }

    /// Store multiple files. Returns `(path, BlobInfo)` pairs in the same order.
    pub fn store_files(
        &mut self,
        files: Vec<(&str, &[u8])>,
    ) -> Result<Vec<(String, BlobInfo)>, PipelineError> {
        let mut results = Vec::with_capacity(files.len());
        for (path, content) in files {
            let info = self.store_file(content)?;
            results.push((path.to_owned(), info));
        }
        Ok(results)
    }

    /// Read a blob back by reconstructing from its chunks.
    ///
    /// For each chunk hash in the blob, look up the compressed payload in the
    /// log, decompress it, and concatenate all chunks to reproduce the original
    /// file content.
    pub fn read_blob(&self, blob: &Blob) -> Result<Vec<u8>, PipelineError> {
        let mut content = Vec::new();

        for hash in &blob.chunks {
            let (offset, _length) = self
                .index
                .get(hash)
                .ok_or(PipelineError::NotFound { hash: *hash })?;

            let compressed = self.log.read_at(offset)?;
            let decompressed = zstd::decode_all(compressed.as_slice())?;
            content.extend_from_slice(&decompressed);
        }

        Ok(content)
    }

    /// Check which hashes exist in the index.
    ///
    /// Returns `(have, missing)` where `have` contains hashes present in the
    /// index and `missing` contains those that are not.
    pub fn have(&self, hashes: &[Hash]) -> (Vec<Hash>, Vec<Hash>) {
        let mut have = Vec::new();
        let mut missing = Vec::new();

        for hash in hashes {
            if self.index.contains(hash) {
                have.push(*hash);
            } else {
                missing.push(*hash);
            }
        }

        (have, missing)
    }

    /// Borrow the index (for inspection/persistence).
    pub fn index(&self) -> &Index {
        &self.index
    }

    /// Mutable access to the index.
    pub fn index_mut(&mut self) -> &mut Index {
        &mut self.index
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Helper: create a Pipeline backed by a temp directory.
    fn test_pipeline() -> (Pipeline, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("test.log");
        let log = AppendLog::open(&log_path).unwrap();
        let index = Index::new();
        (Pipeline::new(log, index), dir)
    }

    #[test]
    fn roundtrip() {
        let (mut pipeline, _dir) = test_pipeline();

        let content = b"hello world, this is a roundtrip test for the content pipeline";
        let info = pipeline.store_file(content).unwrap();

        let read_back = pipeline.read_blob(&info.blob).unwrap();
        assert_eq!(read_back, content);
    }

    #[test]
    fn dedup_same_file_twice() {
        let (mut pipeline, _dir) = test_pipeline();

        let content = b"deduplicated content that should only be stored once";

        let first = pipeline.store_file(content).unwrap();
        assert!(first.stats.new_chunks > 0);
        assert_eq!(first.stats.reused_chunks, 0);

        let second = pipeline.store_file(content).unwrap();
        assert_eq!(second.stats.new_chunks, 0);
        assert_eq!(second.stats.reused_chunks, first.stats.new_chunks);

        // Both blobs should be identical.
        assert_eq!(first.blob.hash, second.blob.hash);
        assert_eq!(first.blob.chunks, second.blob.chunks);

        // Reading back the second blob should produce the same content.
        let read_back = pipeline.read_blob(&second.blob).unwrap();
        assert_eq!(read_back, content);
    }

    #[test]
    fn partial_dedup() {
        let (mut pipeline, _dir) = test_pipeline();

        // Build a file large enough to produce multiple chunks. Each function
        // body is ~1 KB so 6 functions should give us several structural chunks.
        let mut original = String::new();
        for i in 0..6 {
            if i > 0 {
                original.push('\n');
            }
            original.push_str(&format!("pub fn function_{}() {{\n", i));
            for j in 0..40 {
                original.push_str(&format!("    let x_{} = {};\n", j, j + i * 100));
            }
            original.push_str("}\n");
        }

        let first = pipeline.store_file(original.as_bytes()).unwrap();
        let total_chunks = first.stats.new_chunks;
        assert!(total_chunks > 1, "expected multiple chunks from the original file");

        // Modify only the last function.
        let mut modified = String::new();
        for i in 0..5 {
            if i > 0 {
                modified.push('\n');
            }
            modified.push_str(&format!("pub fn function_{}() {{\n", i));
            for j in 0..40 {
                modified.push_str(&format!("    let x_{} = {};\n", j, j + i * 100));
            }
            modified.push_str("}\n");
        }
        modified.push('\n');
        modified.push_str("pub fn function_5() {\n");
        for j in 0..40 {
            modified.push_str(&format!("    let modified_{} = {};\n", j, j + 9999));
        }
        modified.push_str("}\n");

        let second = pipeline.store_file(modified.as_bytes()).unwrap();

        // Some chunks should be reused, some new.
        assert!(second.stats.reused_chunks > 0, "expected some reused chunks");
        assert!(second.stats.new_chunks > 0, "expected some new chunks");

        // Both files should round-trip correctly.
        let read_original = pipeline.read_blob(&first.blob).unwrap();
        assert_eq!(read_original, original.as_bytes());

        let read_modified = pipeline.read_blob(&second.blob).unwrap();
        assert_eq!(read_modified, modified.as_bytes());
    }

    #[test]
    fn batch_store() {
        let (mut pipeline, _dir) = test_pipeline();

        let files: Vec<(&str, &[u8])> = vec![
            ("src/main.rs", b"fn main() { println!(\"hello\"); }"),
            ("src/lib.rs", b"pub fn add(a: i32, b: i32) -> i32 { a + b }"),
            ("README.md", b"# My Project\n\nA simple project."),
        ];

        let results = pipeline.store_files(files).unwrap();
        assert_eq!(results.len(), 3);

        for (path, info) in &results {
            let read_back = pipeline.read_blob(&info.blob).unwrap();
            match path.as_str() {
                "src/main.rs" => {
                    assert_eq!(read_back, b"fn main() { println!(\"hello\"); }");
                }
                "src/lib.rs" => {
                    assert_eq!(read_back, b"pub fn add(a: i32, b: i32) -> i32 { a + b }");
                }
                "README.md" => {
                    assert_eq!(read_back, b"# My Project\n\nA simple project.");
                }
                _ => panic!("unexpected path: {}", path),
            }
        }
    }

    #[test]
    fn have_check() {
        let (mut pipeline, _dir) = test_pipeline();

        let content = b"some content for the have check test";
        let info = pipeline.store_file(content).unwrap();

        // Collect chunk hashes from the stored blob plus some unknown hashes.
        let known_hashes = info.blob.chunks.clone();
        let unknown1 = Hash::from_bytes(b"definitely not stored");
        let unknown2 = Hash::from_bytes(b"also not stored");

        let mut query: Vec<Hash> = known_hashes.clone();
        query.push(unknown1);
        query.push(unknown2);

        let (have, missing) = pipeline.have(&query);

        assert_eq!(have.len(), known_hashes.len());
        assert_eq!(missing.len(), 2);

        for h in &known_hashes {
            assert!(have.contains(h));
        }
        assert!(missing.contains(&unknown1));
        assert!(missing.contains(&unknown2));
    }

    #[test]
    fn empty_file() {
        let (mut pipeline, _dir) = test_pipeline();

        let info = pipeline.store_file(b"").unwrap();

        // Empty content should produce no chunks.
        assert!(info.blob.chunks.is_empty());
        assert_eq!(info.stats.new_chunks, 0);
        assert_eq!(info.stats.reused_chunks, 0);

        // Reading back should give empty bytes.
        let read_back = pipeline.read_blob(&info.blob).unwrap();
        assert!(read_back.is_empty());
    }

    #[test]
    fn large_file() {
        let (mut pipeline, _dir) = test_pipeline();

        // 100 KB of content with a recognizable pattern.
        let content: Vec<u8> = (0..102_400).map(|i| (i % 251) as u8).collect();

        let info = pipeline.store_file(&content).unwrap();

        // Should have at least one chunk.
        assert!(!info.blob.chunks.is_empty());

        // Round-trip must be identical.
        let read_back = pipeline.read_blob(&info.blob).unwrap();
        assert_eq!(read_back.len(), content.len());
        assert_eq!(read_back, content);
    }
}
