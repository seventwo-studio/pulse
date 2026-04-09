// High-level storage engine managing all logs, indices, and typed domain objects.
//
// On-disk layout:
//   .pulse/
//     data/
//       chunks.log        — managed by Pipeline (chunk data)
//     meta/
//       blobs.log         — serialized Blob JSON per entry
//       changesets.log    — serialized Changeset JSON per entry
//       snapshots.log     — serialized Snapshot JSON per entry
//       workspaces.log    — serialized Workspace JSON per entry (full state per event)
//       trunk             — current trunk changeset ID as hex string

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::core::primitives::*;

use super::index::Index;
use super::log::AppendLog;
use super::pipeline::{BlobInfo, Pipeline, PipelineError};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("log error: {0}")]
    Log(#[from] super::log::LogError),
    #[error("pipeline error: {0}")]
    Pipeline(#[from] PipelineError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not initialized: {0}")]
    NotInitialized(PathBuf),
    #[error("already initialized: {0}")]
    AlreadyInitialized(PathBuf),
    #[error("not found: {0}")]
    NotFound(String),
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

pub struct StorageEngine {
    root: PathBuf,
    pipeline: Pipeline,
    blobs_log: AppendLog,
    changesets_log: AppendLog,
    snapshots_log: AppendLog,
    workspaces_log: AppendLog,
    // In-memory caches rebuilt on open
    blobs: HashMap<Hash, Blob>,
    changesets: HashMap<Hash, Changeset>,
    snapshots: HashMap<Hash, Snapshot>,
    workspaces: HashMap<String, Workspace>,
}

impl StorageEngine {
    // -- Paths -------------------------------------------------------------

    fn pulse_dir(root: &Path) -> PathBuf {
        root.join(".pulse")
    }

    fn data_dir(root: &Path) -> PathBuf {
        Self::pulse_dir(root).join("data")
    }

    fn meta_dir(root: &Path) -> PathBuf {
        Self::pulse_dir(root).join("meta")
    }

    fn chunks_log_path(root: &Path) -> PathBuf {
        Self::data_dir(root).join("chunks.log")
    }

    fn blobs_log_path(root: &Path) -> PathBuf {
        Self::meta_dir(root).join("blobs.log")
    }

    fn changesets_log_path(root: &Path) -> PathBuf {
        Self::meta_dir(root).join("changesets.log")
    }

    fn snapshots_log_path(root: &Path) -> PathBuf {
        Self::meta_dir(root).join("snapshots.log")
    }

    fn workspaces_log_path(root: &Path) -> PathBuf {
        Self::meta_dir(root).join("workspaces.log")
    }

    fn trunk_path(root: &Path) -> PathBuf {
        Self::meta_dir(root).join("trunk")
    }

    // -- Init / Open -------------------------------------------------------

    /// Create a new `.pulse/` directory and initialize all logs.
    /// Fails if `.pulse/` already exists.
    pub fn init(root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let root = root.as_ref().to_path_buf();
        let pulse = Self::pulse_dir(&root);

        if pulse.exists() {
            return Err(StorageError::AlreadyInitialized(pulse));
        }

        // Create directory structure
        fs::create_dir_all(Self::data_dir(&root))?;
        fs::create_dir_all(Self::meta_dir(&root))?;

        // Open (create) all logs — this also writes the empty file
        let chunks_log = AppendLog::open(Self::chunks_log_path(&root))?;
        let index = Index::new();
        let pipeline = Pipeline::new(chunks_log, index);

        let blobs_log = AppendLog::open(Self::blobs_log_path(&root))?;
        let changesets_log = AppendLog::open(Self::changesets_log_path(&root))?;
        let snapshots_log = AppendLog::open(Self::snapshots_log_path(&root))?;
        let workspaces_log = AppendLog::open(Self::workspaces_log_path(&root))?;

        Ok(Self {
            root,
            pipeline,
            blobs_log,
            changesets_log,
            snapshots_log,
            workspaces_log,
            blobs: HashMap::new(),
            changesets: HashMap::new(),
            snapshots: HashMap::new(),
            workspaces: HashMap::new(),
        })
    }

    /// Open an existing `.pulse/` directory. Scans all logs and rebuilds
    /// in-memory caches. Runs crash recovery on each log via `AppendLog::open`.
    /// Fails if `.pulse/` doesn't exist.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let root = root.as_ref().to_path_buf();
        let pulse = Self::pulse_dir(&root);

        if !pulse.exists() {
            return Err(StorageError::NotInitialized(pulse));
        }

        // Open chunk pipeline — decompresses every chunk to rebuild the
        // uncompressed-hash index correctly.
        let chunks_log = AppendLog::open(Self::chunks_log_path(&root))?;
        let pipeline = Pipeline::open(chunks_log)?;

        // Open meta logs (recovery happens inside AppendLog::open)
        let blobs_log = AppendLog::open(Self::blobs_log_path(&root))?;
        let changesets_log = AppendLog::open(Self::changesets_log_path(&root))?;
        let snapshots_log = AppendLog::open(Self::snapshots_log_path(&root))?;
        let workspaces_log = AppendLog::open(Self::workspaces_log_path(&root))?;

        // Rebuild in-memory caches from logs
        let mut blobs = HashMap::new();
        for entry in blobs_log.iter() {
            let (_offset, payload) = entry?;
            let blob: Blob = serde_json::from_slice(&payload)?;
            blobs.insert(blob.hash, blob);
        }

        let mut changesets = HashMap::new();
        for entry in changesets_log.iter() {
            let (_offset, payload) = entry?;
            let cs: Changeset = serde_json::from_slice(&payload)?;
            changesets.insert(cs.id, cs);
        }

        let mut snapshots = HashMap::new();
        for entry in snapshots_log.iter() {
            let (_offset, payload) = entry?;
            let snap: Snapshot = serde_json::from_slice(&payload)?;
            snapshots.insert(snap.id, snap);
        }

        let mut workspaces = HashMap::new();
        for entry in workspaces_log.iter() {
            let (_offset, payload) = entry?;
            let ws: Workspace = serde_json::from_slice(&payload)?;
            workspaces.insert(ws.id.clone(), ws);
        }

        Ok(Self {
            root,
            pipeline,
            blobs_log,
            changesets_log,
            snapshots_log,
            workspaces_log,
            blobs,
            changesets,
            snapshots,
            workspaces,
        })
    }

    // -- File storage (delegates to Pipeline) ------------------------------

    /// Store raw file content, returning chunk/blob information.
    /// Also persists the resulting `Blob` for later lookup by hash.
    pub fn store_file(&mut self, content: &[u8]) -> Result<BlobInfo, StorageError> {
        let info = self.pipeline.store_file(content)?;
        self.persist_blob(&info.blob)?;
        Ok(info)
    }

    /// Store multiple files. Returns `(path, BlobInfo)` pairs in the same order.
    /// Persists each resulting `Blob`.
    pub fn store_files(
        &mut self,
        files: Vec<(&str, &[u8])>,
    ) -> Result<Vec<(String, BlobInfo)>, StorageError> {
        let results = self.pipeline.store_files(files)?;
        for (_path, info) in &results {
            self.persist_blob(&info.blob)?;
        }
        Ok(results)
    }

    /// Read a blob back by reconstructing from its chunks.
    pub fn read_blob(&self, blob: &Blob) -> Result<Vec<u8>, StorageError> {
        Ok(self.pipeline.read_blob(blob)?)
    }

    /// Read file content by looking up a path in a snapshot, then
    /// resolving the blob hash to a full `Blob` with chunk list,
    /// and finally reading chunks from the pipeline.
    pub fn read_file_by_path(
        &self,
        snapshot_id: &Hash,
        path: &str,
    ) -> Result<Vec<u8>, StorageError> {
        let snapshot = self.get_snapshot(snapshot_id)?;
        let blob_hash = snapshot
            .files
            .get(path)
            .ok_or_else(|| StorageError::NotFound(format!("file '{}' in snapshot {}", path, snapshot_id)))?;
        let blob = self.get_blob(blob_hash)?;
        Ok(self.pipeline.read_blob(blob)?)
    }

    // -- Blobs -------------------------------------------------------------

    /// Persist a blob to the blobs log and in-memory cache.
    /// Skips writing if the blob is already known (idempotent).
    fn persist_blob(&mut self, blob: &Blob) -> Result<(), StorageError> {
        if self.blobs.contains_key(&blob.hash) {
            return Ok(());
        }
        let json = serde_json::to_vec(blob)?;
        self.blobs_log.append(&json)?;
        self.blobs.insert(blob.hash, blob.clone());
        Ok(())
    }

    /// Explicitly store a blob (e.g. received from a remote).
    pub fn store_blob(&mut self, blob: &Blob) -> Result<(), StorageError> {
        self.persist_blob(blob)
    }

    /// Iterate over all stored blobs.
    pub fn list_blobs(&self) -> impl Iterator<Item = &Blob> {
        self.blobs.values()
    }

    /// Look up a blob by its content hash.
    pub fn get_blob(&self, hash: &Hash) -> Result<&Blob, StorageError> {
        self.blobs
            .get(hash)
            .ok_or_else(|| StorageError::NotFound(format!("blob {}", hash)))
    }

    // -- Snapshots ---------------------------------------------------------

    /// Persist a snapshot to the log and in-memory cache. Returns the snapshot id.
    pub fn store_snapshot(&mut self, snapshot: &Snapshot) -> Result<Hash, StorageError> {
        let json = serde_json::to_vec(snapshot)?;
        self.snapshots_log.append(&json)?;
        let id = snapshot.id;
        self.snapshots.insert(id, snapshot.clone());
        Ok(id)
    }

    /// Iterate over all stored snapshots.
    pub fn list_snapshots(&self) -> impl Iterator<Item = &Snapshot> {
        self.snapshots.values()
    }

    /// Look up a snapshot by id.
    pub fn get_snapshot(&self, id: &Hash) -> Result<&Snapshot, StorageError> {
        self.snapshots
            .get(id)
            .ok_or_else(|| StorageError::NotFound(format!("snapshot {}", id)))
    }

    // -- Changesets --------------------------------------------------------

    /// Persist a changeset to the log and in-memory cache. Returns the changeset id.
    pub fn store_changeset(&mut self, changeset: &Changeset) -> Result<Hash, StorageError> {
        let json = serde_json::to_vec(changeset)?;
        self.changesets_log.append(&json)?;
        let id = changeset.id;
        self.changesets.insert(id, changeset.clone());
        Ok(id)
    }

    /// Iterate over all stored changesets.
    pub fn list_changesets(&self) -> impl Iterator<Item = &Changeset> {
        self.changesets.values()
    }

    /// Look up a changeset by id.
    pub fn get_changeset(&self, id: &Hash) -> Result<&Changeset, StorageError> {
        self.changesets
            .get(id)
            .ok_or_else(|| StorageError::NotFound(format!("changeset {}", id)))
    }

    // -- Workspaces --------------------------------------------------------

    /// Persist a workspace state to the log and upsert in-memory cache.
    /// The workspaces log is event-sourced: every write appends the full
    /// workspace state. On open, the last entry per workspace id wins.
    pub fn store_workspace(&mut self, workspace: &Workspace) -> Result<(), StorageError> {
        let json = serde_json::to_vec(workspace)?;
        self.workspaces_log.append(&json)?;
        self.workspaces
            .insert(workspace.id.clone(), workspace.clone());
        Ok(())
    }

    /// Look up a workspace by id.
    pub fn get_workspace(&self, id: &str) -> Result<&Workspace, StorageError> {
        self.workspaces
            .get(id)
            .ok_or_else(|| StorageError::NotFound(format!("workspace {}", id)))
    }

    /// List workspaces. If `all` is false, only active workspaces are returned.
    pub fn list_workspaces(&self, all: bool) -> Vec<&Workspace> {
        self.workspaces
            .values()
            .filter(|ws| all || ws.status == WorkspaceStatus::Active)
            .collect()
    }

    // -- Trunk -------------------------------------------------------------

    /// Read the current trunk changeset id from `.pulse/meta/trunk`.
    /// Returns `None` if the file doesn't exist yet.
    pub fn get_trunk(&self) -> Result<Option<Hash>, StorageError> {
        let path = Self::trunk_path(&self.root);
        if !path.exists() {
            return Ok(None);
        }
        let hex = fs::read_to_string(&path)?;
        let hex = hex.trim();
        if hex.is_empty() {
            return Ok(None);
        }
        let hash: Hash = hex
            .parse()
            .map_err(|e| StorageError::NotFound(format!("invalid trunk hash: {}", e)))?;
        Ok(Some(hash))
    }

    /// Atomically set the trunk changeset id by writing to a tmp file and renaming.
    pub fn set_trunk(&self, changeset_id: &Hash) -> Result<(), StorageError> {
        let path = Self::trunk_path(&self.root);
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, changeset_id.to_string())?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    // -- Pipeline access (for compaction) ------------------------------------

    /// Borrow the pipeline (used by compaction).
    pub(crate) fn pipeline(&self) -> &super::pipeline::Pipeline {
        &self.pipeline
    }

    /// Mutably borrow the pipeline (used by compaction).
    pub(crate) fn pipeline_mut(&mut self) -> &mut super::pipeline::Pipeline {
        &mut self.pipeline
    }

    // -- Object queries ----------------------------------------------------

    /// Check which content hashes exist in the chunk index.
    /// Returns `(have, missing)`.
    pub fn have_objects(&self, hashes: &[Hash]) -> (Vec<Hash>, Vec<Hash>) {
        self.pipeline.have(hashes)
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

    use super::*;

    /// Helper: init an engine in a temp dir and return (engine, dir).
    fn init_engine() -> (StorageEngine, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let engine = StorageEngine::init(dir.path()).unwrap();
        (engine, dir)
    }

    // 1. Init + open roundtrip
    #[test]
    fn init_and_reopen() {
        let dir = tempdir().unwrap();

        // Init creates .pulse/
        {
            let _engine = StorageEngine::init(dir.path()).unwrap();
        }

        // Reopen should succeed
        let engine = StorageEngine::open(dir.path()).unwrap();
        assert!(engine.get_trunk().unwrap().is_none());
    }

    // 2. Store and retrieve changeset
    #[test]
    fn store_and_get_changeset() {
        let (mut engine, _dir) = init_engine();

        let cs = Changeset::new(
            None,
            Hash::from_bytes(b"snap"),
            Utc::now(),
            Author::human("alice"),
            "initial commit".into(),
            vec!["README.md".into()],
            None,
        );

        let id = engine.store_changeset(&cs).unwrap();
        assert_eq!(id, cs.id);

        let retrieved = engine.get_changeset(&id).unwrap();
        assert_eq!(retrieved, &cs);
    }

    // 3. Store and retrieve snapshot
    #[test]
    fn store_and_get_snapshot() {
        let (mut engine, _dir) = init_engine();

        let mut files = BTreeMap::new();
        files.insert("src/main.rs".into(), Hash::from_bytes(b"main content"));
        files.insert("src/lib.rs".into(), Hash::from_bytes(b"lib content"));
        let snap = Snapshot::new(files.clone());

        let id = engine.store_snapshot(&snap).unwrap();
        assert_eq!(id, snap.id);

        let retrieved = engine.get_snapshot(&id).unwrap();
        assert_eq!(retrieved.files.len(), 2);
        assert_eq!(retrieved.files, files);
    }

    // 4. Full pipeline: store file -> snapshot -> changeset -> read back
    #[test]
    fn full_pipeline_roundtrip() {
        let (mut engine, _dir) = init_engine();

        let content = b"fn main() { println!(\"hello pulse\"); }";

        // Store file content
        let info = engine.store_file(content).unwrap();
        assert!(!info.blob.chunks.is_empty());

        // Build a snapshot referencing this blob
        let mut files = BTreeMap::new();
        files.insert("src/main.rs".into(), info.blob.hash);
        let snap = Snapshot::new(files);
        let snap_id = engine.store_snapshot(&snap).unwrap();

        // Build a changeset pointing to the snapshot
        let cs = Changeset::new(
            None,
            snap_id,
            Utc::now(),
            Author::human("alice"),
            "add main.rs".into(),
            vec!["src/main.rs".into()],
            None,
        );
        engine.store_changeset(&cs).unwrap();

        // Read file back via read_file_by_path
        let read_back = engine.read_file_by_path(&snap_id, "src/main.rs").unwrap();
        assert_eq!(read_back, content);
    }

    // 5. Trunk: set, drop, reopen, get
    #[test]
    fn trunk_persists_across_reopen() {
        let dir = tempdir().unwrap();
        let hash = Hash::from_bytes(b"trunk changeset");

        {
            let engine = StorageEngine::init(dir.path()).unwrap();
            assert!(engine.get_trunk().unwrap().is_none());
            engine.set_trunk(&hash).unwrap();
            assert_eq!(engine.get_trunk().unwrap(), Some(hash));
        }

        // Reopen and verify trunk survived
        let engine = StorageEngine::open(dir.path()).unwrap();
        assert_eq!(engine.get_trunk().unwrap(), Some(hash));
    }

    // 6. Workspaces: store, list, update, get returns latest
    #[test]
    fn workspace_store_list_update() {
        let (mut engine, _dir) = init_engine();

        let mut ws = Workspace::new(
            Hash::ZERO,
            "add logging".into(),
            vec!["src/**".into()],
            Author::human("bob"),
        );
        let ws_id = ws.id.clone();

        engine.store_workspace(&ws).unwrap();

        // list active
        let active = engine.list_workspaces(false);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, ws_id);

        // Update: add a changeset
        let cs_hash = Hash::from_bytes(b"some changeset");
        ws.changesets.push(cs_hash);
        engine.store_workspace(&ws).unwrap();

        // get should return the updated version
        let retrieved = engine.get_workspace(&ws_id).unwrap();
        assert_eq!(retrieved.changesets.len(), 1);
        assert_eq!(retrieved.changesets[0], cs_hash);

        // Mark merged
        ws.status = WorkspaceStatus::Merged;
        engine.store_workspace(&ws).unwrap();

        // list active should be empty now
        let active = engine.list_workspaces(false);
        assert!(active.is_empty());

        // list all should still show it
        let all = engine.list_workspaces(true);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, WorkspaceStatus::Merged);
    }

    // 7. Already initialized
    #[test]
    fn init_already_initialized() {
        let dir = tempdir().unwrap();
        StorageEngine::init(dir.path()).unwrap();
        let result = StorageEngine::init(dir.path());
        assert!(matches!(result, Err(StorageError::AlreadyInitialized(_))));
    }

    // 8. Not initialized
    #[test]
    fn open_not_initialized() {
        let dir = tempdir().unwrap();
        let result = StorageEngine::open(dir.path());
        assert!(matches!(result, Err(StorageError::NotInitialized(_))));
    }

    // 9. Data survives full close/reopen cycle with multiple object types
    #[test]
    fn full_persistence_across_reopen() {
        let dir = tempdir().unwrap();

        let content = b"persistent file content for the roundtrip test";
        let snap_id;
        let cs_id;
        let ws_id;

        {
            let mut engine = StorageEngine::init(dir.path()).unwrap();

            // Store file
            let info = engine.store_file(content).unwrap();

            // Store snapshot
            let mut files = BTreeMap::new();
            files.insert("data.txt".into(), info.blob.hash);
            let snap = Snapshot::new(files);
            snap_id = engine.store_snapshot(&snap).unwrap();

            // Store changeset
            let cs = Changeset::new(
                None,
                snap_id,
                Utc::now(),
                Author::human("alice"),
                "add data.txt".into(),
                vec!["data.txt".into()],
                None,
            );
            cs_id = engine.store_changeset(&cs).unwrap();

            // Store workspace
            let mut ws = Workspace::new(
                Hash::ZERO,
                "feature work".into(),
                vec![],
                Author::agent("claude", None),
            );
            ws.changesets.push(cs_id);
            ws_id = ws.id.clone();
            engine.store_workspace(&ws).unwrap();

            // Set trunk
            engine.set_trunk(&cs_id).unwrap();
        }

        // Reopen and verify everything
        let engine = StorageEngine::open(dir.path()).unwrap();

        let read_back = engine.read_file_by_path(&snap_id, "data.txt").unwrap();
        assert_eq!(read_back, content);

        let cs = engine.get_changeset(&cs_id).unwrap();
        assert_eq!(cs.message, "add data.txt");

        let snap = engine.get_snapshot(&snap_id).unwrap();
        assert!(snap.files.contains_key("data.txt"));

        let ws = engine.get_workspace(&ws_id).unwrap();
        assert_eq!(ws.changesets.len(), 1);

        assert_eq!(engine.get_trunk().unwrap(), Some(cs_id));
    }

    // 10. Not found errors
    #[test]
    fn not_found_errors() {
        let (engine, _dir) = init_engine();
        let missing = Hash::from_bytes(b"does not exist");

        assert!(matches!(
            engine.get_changeset(&missing),
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            engine.get_snapshot(&missing),
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            engine.get_blob(&missing),
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            engine.get_workspace("ws-nope"),
            Err(StorageError::NotFound(_))
        ));
    }

    // 11. read_file_by_path with missing path returns NotFound
    #[test]
    fn read_file_by_path_missing_path() {
        let (mut engine, _dir) = init_engine();

        let snap = Snapshot::empty();
        let snap_id = engine.store_snapshot(&snap).unwrap();

        let result = engine.read_file_by_path(&snap_id, "nonexistent.rs");
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }

    // 12. have_objects delegates correctly
    #[test]
    fn have_objects_check() {
        let (mut engine, _dir) = init_engine();

        let info = engine.store_file(b"some content").unwrap();
        let unknown = Hash::from_bytes(b"unknown chunk");

        let mut query: Vec<Hash> = info.blob.chunks.clone();
        query.push(unknown);

        let (have, missing) = engine.have_objects(&query);
        assert_eq!(have.len(), info.blob.chunks.len());
        assert_eq!(missing.len(), 1);
        assert!(missing.contains(&unknown));
    }

    // 13. Blob dedup: storing same file twice only writes blob once
    #[test]
    fn blob_dedup() {
        let (mut engine, _dir) = init_engine();

        let content = b"identical content stored twice";
        let info1 = engine.store_file(content).unwrap();
        let info2 = engine.store_file(content).unwrap();

        assert_eq!(info1.blob.hash, info2.blob.hash);

        // Second store should have reused all chunks
        assert_eq!(info2.stats.new_chunks, 0);
        assert_eq!(info2.stats.reused_chunks, info1.stats.new_chunks);
    }

    // 14. Multiple workspaces with mixed statuses
    #[test]
    fn multiple_workspaces_filtering() {
        let (mut engine, _dir) = init_engine();

        let ws1 = Workspace::new(
            Hash::ZERO,
            "feature A".into(),
            vec![],
            Author::human("alice"),
        );
        let mut ws2 = Workspace::new(
            Hash::ZERO,
            "feature B".into(),
            vec![],
            Author::human("bob"),
        );
        ws2.status = WorkspaceStatus::Abandoned;

        let ws3 = Workspace::new(
            Hash::ZERO,
            "feature C".into(),
            vec![],
            Author::human("carol"),
        );

        engine.store_workspace(&ws1).unwrap();
        engine.store_workspace(&ws2).unwrap();
        engine.store_workspace(&ws3).unwrap();

        // Active only
        let active = engine.list_workspaces(false);
        assert_eq!(active.len(), 2);

        // All
        let all = engine.list_workspaces(true);
        assert_eq!(all.len(), 3);
    }
}
