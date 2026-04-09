// Workspace manager — stateless helper for workspace lifecycle operations.
//
// All methods operate on `&StorageEngine` (reads) or `&mut StorageEngine` (writes).
// The server layer is responsible for per-workspace locking to serialize commits.

use crate::core::primitives::*;
use crate::storage::engine::{StorageEngine, StorageError};
use crate::storage::pipeline::StoreStats;

/// Result of a commit operation.
#[derive(Debug)]
pub struct CommitResult {
    pub changeset: Changeset,
    pub stats: StoreStats,
}

pub struct WorkspaceManager;

impl WorkspaceManager {
    /// Create a new workspace branching from the given trunk head.
    pub fn create(
        storage: &mut StorageEngine,
        intent: String,
        scope: Vec<String>,
        author: Author,
        trunk_head: &Hash,
    ) -> Result<Workspace, StorageError> {
        let ws = Workspace::new(*trunk_head, intent, scope, author);
        storage.store_workspace(&ws)?;
        Ok(ws)
    }

    /// Commit files to a workspace.
    ///
    /// 1. Verify workspace exists and is Active.
    /// 2. Determine parent changeset: last in workspace.changesets, or workspace.base if first commit.
    /// 3. Get parent snapshot.
    /// 4. Store all files via storage engine to get blob hashes.
    /// 5. Build new snapshot: start from parent snapshot, update changed paths with new blob hashes.
    /// 6. Store snapshot.
    /// 7. Create changeset with parent, snapshot, author, message, files_changed.
    /// 8. Store changeset.
    /// 9. Update workspace: push changeset id to workspace.changesets, store updated workspace.
    /// 10. Return CommitResult.
    pub fn commit(
        storage: &mut StorageEngine,
        workspace_id: &str,
        files: Vec<(String, Vec<u8>)>,
        message: String,
        author: Author,
    ) -> Result<CommitResult, StorageError> {
        // Get workspace, verify Active
        let mut ws = storage.get_workspace(workspace_id)?.clone();
        if ws.status != WorkspaceStatus::Active {
            return Err(StorageError::NotFound(format!(
                "workspace {} is not active",
                workspace_id
            )));
        }

        // Find parent changeset id: last commit in workspace, or the base changeset
        let parent_id = ws.changesets.last().copied().unwrap_or(ws.base);

        // Get parent snapshot
        let parent_cs = storage.get_changeset(&parent_id)?;
        let parent_snapshot_id = parent_cs.snapshot;
        let parent_snapshot = storage.get_snapshot(&parent_snapshot_id)?.clone();

        // Store files, collect blob hashes and stats
        let file_refs: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_slice()))
            .collect();
        let stored = storage.store_files(file_refs)?;

        let mut total_new = 0usize;
        let mut total_reused = 0usize;

        // Build new snapshot: start from parent, update changed paths
        let mut new_files = parent_snapshot.files.clone();
        let mut files_changed = Vec::new();

        for (path, blob_info) in &stored {
            new_files.insert(path.clone(), blob_info.blob.hash);
            files_changed.push(path.clone());
            total_new += blob_info.stats.new_chunks;
            total_reused += blob_info.stats.reused_chunks;
        }

        let snapshot = Snapshot::new(new_files);
        storage.store_snapshot(&snapshot)?;

        // Create changeset
        let changeset = Changeset::new(
            Some(parent_id),
            snapshot.id,
            chrono::Utc::now(),
            author,
            message,
            files_changed,
            None,
        );
        storage.store_changeset(&changeset)?;

        // Update workspace
        ws.changesets.push(changeset.id);
        storage.store_workspace(&ws)?;

        Ok(CommitResult {
            changeset,
            stats: StoreStats {
                new_chunks: total_new,
                reused_chunks: total_reused,
            },
        })
    }

    /// Abandon a workspace. Sets its status to Abandoned.
    pub fn abandon(
        storage: &mut StorageEngine,
        workspace_id: &str,
    ) -> Result<Workspace, StorageError> {
        let mut ws = storage.get_workspace(workspace_id)?.clone();
        if ws.status != WorkspaceStatus::Active {
            return Err(StorageError::NotFound(format!(
                "workspace {} is not active",
                workspace_id
            )));
        }
        ws.status = WorkspaceStatus::Abandoned;
        storage.store_workspace(&ws)?;
        Ok(ws)
    }

    /// Get a workspace by ID.
    pub fn get(storage: &StorageEngine, workspace_id: &str) -> Result<Workspace, StorageError> {
        Ok(storage.get_workspace(workspace_id)?.clone())
    }

    /// List workspaces. If `all` is false, only Active workspaces are returned.
    pub fn list(storage: &StorageEngine, all: bool) -> Vec<Workspace> {
        storage.list_workspaces(all).into_iter().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::storage::engine::StorageEngine;

    /// Initialize a storage engine with a valid repo state (empty snapshot + root changeset + trunk).
    fn setup() -> (StorageEngine, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let mut storage = StorageEngine::init(dir.path()).unwrap();

        // Init repo: empty snapshot + root changeset + trunk
        let snapshot = Snapshot::empty();
        storage.store_snapshot(&snapshot).unwrap();
        let root = Changeset::root(snapshot.id);
        storage.store_changeset(&root).unwrap();
        storage.set_trunk(&root.id).unwrap();

        (storage, dir)
    }

    /// Helper: get the current trunk head hash from the engine.
    fn trunk_head(storage: &StorageEngine) -> Hash {
        storage.get_trunk().unwrap().unwrap()
    }

    // 1. Create workspace: verify id format (ws-XXXX), base matches trunk head, status Active
    #[test]
    fn create_workspace() {
        let (mut storage, _dir) = setup();
        let head = trunk_head(&storage);

        let ws = WorkspaceManager::create(
            &mut storage,
            "add logging".into(),
            vec!["src/**".into()],
            Author::human("alice"),
            &head,
        )
        .unwrap();

        assert!(ws.id.starts_with("ws-"));
        assert_eq!(ws.id.len(), 7);
        assert!(ws.id[3..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(ws.base, head);
        assert_eq!(ws.status, WorkspaceStatus::Active);
        assert!(ws.changesets.is_empty());
        assert_eq!(ws.intent, "add logging");
        assert_eq!(ws.scope, vec!["src/**".to_string()]);

        // Should be retrievable
        let fetched = WorkspaceManager::get(&storage, &ws.id).unwrap();
        assert_eq!(fetched, ws);
    }

    // 2. Commit to workspace: store files, verify changeset created with correct parent (= workspace base for first commit)
    #[test]
    fn commit_to_workspace() {
        let (mut storage, _dir) = setup();
        let head = trunk_head(&storage);

        let ws = WorkspaceManager::create(
            &mut storage,
            "initial feature".into(),
            vec![],
            Author::human("alice"),
            &head,
        )
        .unwrap();

        let files = vec![
            ("src/main.rs".to_string(), b"fn main() {}".to_vec()),
            ("README.md".to_string(), b"# Hello".to_vec()),
        ];

        let result = WorkspaceManager::commit(
            &mut storage,
            &ws.id,
            files,
            "add initial files".into(),
            Author::human("alice"),
        )
        .unwrap();

        // Parent should be the workspace base (root changeset)
        assert_eq!(result.changeset.parent, Some(head));
        assert_eq!(result.changeset.message, "add initial files");
        assert_eq!(result.changeset.files_changed.len(), 2);
        assert!(result.changeset.files_changed.contains(&"src/main.rs".to_string()));
        assert!(result.changeset.files_changed.contains(&"README.md".to_string()));

        // Workspace should now have one changeset
        let updated_ws = WorkspaceManager::get(&storage, &ws.id).unwrap();
        assert_eq!(updated_ws.changesets.len(), 1);
        assert_eq!(updated_ws.changesets[0], result.changeset.id);
    }

    // 3. Second commit: parent is first commit's changeset id, snapshot includes files from both commits
    #[test]
    fn second_commit_chains_parent() {
        let (mut storage, _dir) = setup();
        let head = trunk_head(&storage);

        let ws = WorkspaceManager::create(
            &mut storage,
            "multi-commit feature".into(),
            vec![],
            Author::human("alice"),
            &head,
        )
        .unwrap();

        // First commit
        let first = WorkspaceManager::commit(
            &mut storage,
            &ws.id,
            vec![("src/a.rs".to_string(), b"file a".to_vec())],
            "add a".into(),
            Author::human("alice"),
        )
        .unwrap();

        // Second commit
        let second = WorkspaceManager::commit(
            &mut storage,
            &ws.id,
            vec![("src/b.rs".to_string(), b"file b".to_vec())],
            "add b".into(),
            Author::human("alice"),
        )
        .unwrap();

        // Second commit's parent should be the first commit
        assert_eq!(second.changeset.parent, Some(first.changeset.id));

        // Snapshot from the second commit should contain both files
        let snap = storage.get_snapshot(&second.changeset.snapshot).unwrap();
        assert!(snap.files.contains_key("src/a.rs"));
        assert!(snap.files.contains_key("src/b.rs"));

        // Workspace should have two changesets in order
        let updated_ws = WorkspaceManager::get(&storage, &ws.id).unwrap();
        assert_eq!(updated_ws.changesets.len(), 2);
        assert_eq!(updated_ws.changesets[0], first.changeset.id);
        assert_eq!(updated_ws.changesets[1], second.changeset.id);
    }

    // 4. Commit to non-existent workspace: returns error
    #[test]
    fn commit_to_nonexistent_workspace() {
        let (mut storage, _dir) = setup();

        let result = WorkspaceManager::commit(
            &mut storage,
            "ws-nope",
            vec![("file.rs".to_string(), b"data".to_vec())],
            "should fail".into(),
            Author::human("alice"),
        );

        assert!(result.is_err());
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }

    // 5. Commit to abandoned workspace: returns error
    #[test]
    fn commit_to_abandoned_workspace() {
        let (mut storage, _dir) = setup();
        let head = trunk_head(&storage);

        let ws = WorkspaceManager::create(
            &mut storage,
            "doomed workspace".into(),
            vec![],
            Author::human("alice"),
            &head,
        )
        .unwrap();

        WorkspaceManager::abandon(&mut storage, &ws.id).unwrap();

        let result = WorkspaceManager::commit(
            &mut storage,
            &ws.id,
            vec![("file.rs".to_string(), b"data".to_vec())],
            "should fail".into(),
            Author::human("alice"),
        );

        assert!(result.is_err());
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }

    // 6. Abandon: status changes to Abandoned, subsequent commit fails
    #[test]
    fn abandon_workspace() {
        let (mut storage, _dir) = setup();
        let head = trunk_head(&storage);

        let ws = WorkspaceManager::create(
            &mut storage,
            "to be abandoned".into(),
            vec![],
            Author::human("alice"),
            &head,
        )
        .unwrap();

        assert_eq!(ws.status, WorkspaceStatus::Active);

        let abandoned = WorkspaceManager::abandon(&mut storage, &ws.id).unwrap();
        assert_eq!(abandoned.status, WorkspaceStatus::Abandoned);

        // Fetching should show Abandoned
        let fetched = WorkspaceManager::get(&storage, &ws.id).unwrap();
        assert_eq!(fetched.status, WorkspaceStatus::Abandoned);

        // Committing should fail
        let result = WorkspaceManager::commit(
            &mut storage,
            &ws.id,
            vec![("file.rs".to_string(), b"data".to_vec())],
            "nope".into(),
            Author::human("alice"),
        );
        assert!(matches!(result, Err(StorageError::NotFound(_))));

        // Abandoning again should also fail (not active)
        let result = WorkspaceManager::abandon(&mut storage, &ws.id);
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }

    // 7. List filtering: create 3 workspaces, abandon 1, list(false) returns 2, list(true) returns 3
    #[test]
    fn list_filtering() {
        let (mut storage, _dir) = setup();
        let head = trunk_head(&storage);

        let ws1 = WorkspaceManager::create(
            &mut storage,
            "feature A".into(),
            vec![],
            Author::human("alice"),
            &head,
        )
        .unwrap();

        let ws2 = WorkspaceManager::create(
            &mut storage,
            "feature B".into(),
            vec![],
            Author::human("bob"),
            &head,
        )
        .unwrap();

        let _ws3 = WorkspaceManager::create(
            &mut storage,
            "feature C".into(),
            vec![],
            Author::human("carol"),
            &head,
        )
        .unwrap();

        // Abandon ws2
        WorkspaceManager::abandon(&mut storage, &ws2.id).unwrap();

        // Active only should return 2
        let active = WorkspaceManager::list(&storage, false);
        assert_eq!(active.len(), 2);
        assert!(active.iter().all(|ws| ws.status == WorkspaceStatus::Active));
        assert!(active.iter().any(|ws| ws.id == ws1.id));
        assert!(!active.iter().any(|ws| ws.id == ws2.id));

        // All should return 3
        let all = WorkspaceManager::list(&storage, true);
        assert_eq!(all.len(), 3);
    }

    // 8. File content roundtrip: commit files, read them back via the workspace's latest snapshot
    #[test]
    fn file_content_roundtrip() {
        let (mut storage, _dir) = setup();
        let head = trunk_head(&storage);

        let ws = WorkspaceManager::create(
            &mut storage,
            "content test".into(),
            vec![],
            Author::human("alice"),
            &head,
        )
        .unwrap();

        let main_content = b"fn main() { println!(\"hello pulse\"); }";
        let lib_content = b"pub fn add(a: i32, b: i32) -> i32 { a + b }";

        let result = WorkspaceManager::commit(
            &mut storage,
            &ws.id,
            vec![
                ("src/main.rs".to_string(), main_content.to_vec()),
                ("src/lib.rs".to_string(), lib_content.to_vec()),
            ],
            "add source files".into(),
            Author::human("alice"),
        )
        .unwrap();

        // Read files back via the changeset's snapshot
        let snapshot_id = result.changeset.snapshot;

        let read_main = storage.read_file_by_path(&snapshot_id, "src/main.rs").unwrap();
        assert_eq!(read_main, main_content);

        let read_lib = storage.read_file_by_path(&snapshot_id, "src/lib.rs").unwrap();
        assert_eq!(read_lib, lib_content);

        // Verify snapshot has exactly these two files (empty snapshot had none)
        let snap = storage.get_snapshot(&snapshot_id).unwrap();
        assert_eq!(snap.files.len(), 2);
    }
}
