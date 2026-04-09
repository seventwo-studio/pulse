use chrono::{DateTime, Utc};

use crate::core::primitives::{Changeset, Hash, Snapshot};
use crate::storage::engine::{StorageEngine, StorageError};

/// Manages the trunk — the single linear history reference.
///
/// All methods are stateless associated functions operating on a borrowed
/// `StorageEngine`. The server layer is responsible for synchronisation
/// (e.g. wrapping `StorageEngine` in `Arc<Mutex<>>`).
pub struct TrunkManager;

impl TrunkManager {
    /// Get the current trunk head changeset, if any.
    /// Returns `None` if the repo was just initialised and trunk hasn't been set yet.
    pub fn head(storage: &StorageEngine) -> Result<Option<Changeset>, StorageError> {
        match storage.get_trunk()? {
            Some(id) => Ok(Some(storage.get_changeset(&id)?.clone())),
            None => Ok(None),
        }
    }

    /// Get the current trunk head hash, if any.
    pub fn head_id(storage: &StorageEngine) -> Result<Option<Hash>, StorageError> {
        storage.get_trunk()
    }

    /// Advance trunk to a new changeset.
    /// Caller is responsible for holding any necessary locks.
    pub fn advance(storage: &mut StorageEngine, changeset_id: &Hash) -> Result<(), StorageError> {
        // Verify the changeset exists before moving the pointer.
        storage.get_changeset(changeset_id)?;
        storage.set_trunk(changeset_id)
    }

    /// Walk the parent chain from trunk head backwards, returning changesets
    /// in reverse chronological order.
    ///
    /// - `limit`: maximum number of changesets to return.
    /// - `author`: if `Some`, only include changesets whose `author.id` matches.
    /// - `since`: if `Some`, stop walking once a changeset's timestamp is older.
    pub fn log(
        storage: &StorageEngine,
        limit: usize,
        author: Option<&str>,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<Changeset>, StorageError> {
        let mut result = Vec::new();
        let mut current = match storage.get_trunk()? {
            Some(id) => id,
            None => return Ok(result),
        };

        loop {
            if result.len() >= limit {
                break;
            }

            let changeset = storage.get_changeset(&current)?.clone();

            // Stop walking once we pass the `since` threshold.
            if let Some(since_ts) = since {
                if changeset.timestamp < since_ts {
                    break;
                }
            }

            // Apply author filter — skip but keep walking.
            let include = match author {
                Some(author_filter) => changeset.author.id == author_filter,
                None => true,
            };

            if include {
                result.push(changeset.clone());
            }

            match changeset.parent {
                Some(parent_id) => current = parent_id,
                None => break,
            }
        }

        Ok(result)
    }

    /// Get the current trunk snapshot, if any.
    pub fn snapshot(storage: &StorageEngine) -> Result<Option<Snapshot>, StorageError> {
        match Self::head(storage)? {
            Some(cs) => Ok(Some(storage.get_snapshot(&cs.snapshot)?.clone())),
            None => Ok(None),
        }
    }

    /// Initialise the repository: create an empty snapshot and a root changeset,
    /// then set the trunk pointer.
    pub fn init_repo(storage: &mut StorageEngine) -> Result<Changeset, StorageError> {
        let snapshot = Snapshot::empty();
        storage.store_snapshot(&snapshot)?;
        let changeset = Changeset::root(snapshot.id);
        storage.store_changeset(&changeset)?;
        storage.set_trunk(&changeset.id)?;
        Ok(changeset)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use tempfile::tempdir;

    use crate::core::primitives::{Author, AuthorKind, Changeset, Hash, Snapshot};
    use crate::storage::engine::StorageEngine;

    use super::TrunkManager;

    /// Helper: init an engine in a temp dir.
    fn init_engine() -> (StorageEngine, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let engine = StorageEngine::init(dir.path()).unwrap();
        (engine, dir)
    }

    // 1. Fresh repo: head() returns None before init
    #[test]
    fn head_returns_none_on_fresh_repo() {
        let (engine, _dir) = init_engine();
        let head = TrunkManager::head(&engine).unwrap();
        assert!(head.is_none());
    }

    // 2. After init: init_repo(), head() returns root changeset with empty snapshot
    #[test]
    fn head_returns_root_after_init() {
        let (mut engine, _dir) = init_engine();
        let root = TrunkManager::init_repo(&mut engine).unwrap();

        let head = TrunkManager::head(&engine).unwrap().unwrap();
        assert_eq!(head.id, root.id);
        assert!(head.parent.is_none());

        // The snapshot referenced by the root changeset should be empty.
        let snap = engine.get_snapshot(&head.snapshot).unwrap();
        assert!(snap.files.is_empty());
    }

    // 3. Advance: create a second changeset, advance, verify head changed
    #[test]
    fn advance_updates_head() {
        let (mut engine, _dir) = init_engine();
        let root = TrunkManager::init_repo(&mut engine).unwrap();

        let snap = Snapshot::empty();
        engine.store_snapshot(&snap).unwrap();
        let cs = Changeset::new(
            Some(root.id),
            snap.id,
            Utc::now(),
            Author::human("alice"),
            "second commit".into(),
            vec![],
            None,
        );
        engine.store_changeset(&cs).unwrap();
        TrunkManager::advance(&mut engine, &cs.id).unwrap();

        let head = TrunkManager::head(&engine).unwrap().unwrap();
        assert_eq!(head.id, cs.id);
        assert_eq!(head.parent, Some(root.id));
    }

    // 4. Log walk: chain of 5 changesets, verify log returns them in reverse order
    #[test]
    fn log_returns_reverse_chronological_order() {
        let (mut engine, _dir) = init_engine();
        let root = TrunkManager::init_repo(&mut engine).unwrap();

        let mut prev_id = root.id;
        let mut ids = vec![root.id];

        for i in 1..5 {
            let snap = Snapshot::empty();
            engine.store_snapshot(&snap).unwrap();
            let cs = Changeset::new(
                Some(prev_id),
                snap.id,
                Utc::now(),
                Author::human("alice"),
                format!("commit {i}"),
                vec![],
                None,
            );
            engine.store_changeset(&cs).unwrap();
            TrunkManager::advance(&mut engine, &cs.id).unwrap();
            prev_id = cs.id;
            ids.push(cs.id);
        }

        let log = TrunkManager::log(&engine, 100, None, None).unwrap();
        assert_eq!(log.len(), 5);

        // Most recent first.
        ids.reverse();
        let log_ids: Vec<Hash> = log.iter().map(|cs| cs.id).collect();
        assert_eq!(log_ids, ids);
    }

    // 5. Log author filter: mixed authors, filter returns only matching
    #[test]
    fn log_filters_by_author() {
        let (mut engine, _dir) = init_engine();
        let root = TrunkManager::init_repo(&mut engine).unwrap();

        let authors = ["alice", "bob", "alice", "bob"];
        let mut prev_id = root.id;

        for (i, author) in authors.iter().enumerate() {
            let snap = Snapshot::empty();
            engine.store_snapshot(&snap).unwrap();
            let cs = Changeset::new(
                Some(prev_id),
                snap.id,
                Utc::now(),
                Author::human(*author),
                format!("commit {}", i + 1),
                vec![],
                None,
            );
            engine.store_changeset(&cs).unwrap();
            TrunkManager::advance(&mut engine, &cs.id).unwrap();
            prev_id = cs.id;
        }

        let alice_log = TrunkManager::log(&engine, 100, Some("alice"), None).unwrap();
        assert_eq!(alice_log.len(), 2);
        for cs in &alice_log {
            assert_eq!(cs.author.id, "alice");
        }

        let bob_log = TrunkManager::log(&engine, 100, Some("bob"), None).unwrap();
        assert_eq!(bob_log.len(), 2);
        for cs in &bob_log {
            assert_eq!(cs.author.id, "bob");
        }
    }

    // 6. Log limit: 5 changesets, limit=2 returns only 2
    #[test]
    fn log_respects_limit() {
        let (mut engine, _dir) = init_engine();
        let root = TrunkManager::init_repo(&mut engine).unwrap();

        let mut prev_id = root.id;
        for i in 1..5 {
            let snap = Snapshot::empty();
            engine.store_snapshot(&snap).unwrap();
            let cs = Changeset::new(
                Some(prev_id),
                snap.id,
                Utc::now(),
                Author::human("alice"),
                format!("commit {i}"),
                vec![],
                None,
            );
            engine.store_changeset(&cs).unwrap();
            TrunkManager::advance(&mut engine, &cs.id).unwrap();
            prev_id = cs.id;
        }

        let log = TrunkManager::log(&engine, 2, None, None).unwrap();
        assert_eq!(log.len(), 2);
    }

    // 7. Log since filter: changesets with timestamps, since filters correctly
    #[test]
    fn log_filters_by_since() {
        let (mut engine, _dir) = init_engine();
        let root = TrunkManager::init_repo(&mut engine).unwrap();

        let now = Utc::now();
        let mut prev_id = root.id;
        let mut recent_count = 0;

        // Create 4 changesets: 2 old (30 days ago), 2 recent (now).
        for i in 0..4 {
            let ts = if i < 2 {
                now - Duration::days(30)
            } else {
                recent_count += 1;
                now
            };
            let snap = Snapshot::empty();
            engine.store_snapshot(&snap).unwrap();
            let cs = Changeset::new(
                Some(prev_id),
                snap.id,
                ts,
                Author::human("alice"),
                format!("commit {i}"),
                vec![],
                None,
            );
            engine.store_changeset(&cs).unwrap();
            TrunkManager::advance(&mut engine, &cs.id).unwrap();
            prev_id = cs.id;
        }

        // Since 7 days ago — should only return the 2 recent ones.
        let since = now - Duration::days(7);
        let log = TrunkManager::log(&engine, 100, None, Some(since)).unwrap();
        assert_eq!(log.len(), recent_count);
        for cs in &log {
            assert!(cs.timestamp >= since);
        }
    }

    // 8. Snapshot: after init, snapshot() returns empty snapshot
    #[test]
    fn snapshot_returns_empty_after_init() {
        let (mut engine, _dir) = init_engine();
        TrunkManager::init_repo(&mut engine).unwrap();

        let snap = TrunkManager::snapshot(&engine).unwrap().unwrap();
        assert!(snap.files.is_empty());
        assert_eq!(snap.id, Snapshot::empty().id);
    }

    // 9. init_repo produces valid changeset: parent=None, author=system, correct message
    #[test]
    fn init_repo_produces_valid_changeset() {
        let (mut engine, _dir) = init_engine();
        let root = TrunkManager::init_repo(&mut engine).unwrap();

        assert!(root.parent.is_none());
        assert_eq!(root.author.kind, AuthorKind::System);
        assert_eq!(root.author.id, "pulse");
        assert_eq!(root.message, "Repository initialized");
        assert!(root.files_changed.is_empty());
        assert!(root.metadata.is_none());
    }
}
