use serde::{Deserialize, Serialize};

use crate::core::primitives::Snapshot;

/// Result of comparing two snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffResult {
    /// Paths present in `b` but not in `a`.
    pub added: Vec<String>,
    /// Paths present in `a` but not in `b`.
    pub removed: Vec<String>,
    /// Paths present in both but with different blob hashes.
    pub modified: Vec<String>,
}

impl DiffResult {
    /// Returns true when the two snapshots are identical.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }

    /// All changed paths (added + removed + modified), useful for merge conflict detection.
    pub fn all_changed(&self) -> Vec<String> {
        let mut paths = Vec::with_capacity(self.added.len() + self.removed.len() + self.modified.len());
        paths.extend_from_slice(&self.added);
        paths.extend_from_slice(&self.removed);
        paths.extend_from_slice(&self.modified);
        paths.sort();
        paths
    }
}

/// Compare snapshot `a` to snapshot `b`.
///
/// Both snapshots use `BTreeMap` internally, so iteration is already sorted by
/// path. We walk both iterators in lockstep for O(n + m) performance.
pub fn diff_snapshots(a: &Snapshot, b: &Snapshot) -> DiffResult {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();

    let mut iter_a = a.files.iter().peekable();
    let mut iter_b = b.files.iter().peekable();

    loop {
        match (iter_a.peek(), iter_b.peek()) {
            (Some((pa, _)), Some((pb, _))) => match pa.cmp(pb) {
                std::cmp::Ordering::Equal => {
                    let (path, hash_a) = iter_a.next().unwrap();
                    let (_, hash_b) = iter_b.next().unwrap();
                    if hash_a != hash_b {
                        modified.push(path.clone());
                    }
                }
                std::cmp::Ordering::Less => {
                    let (path, _) = iter_a.next().unwrap();
                    removed.push(path.clone());
                }
                std::cmp::Ordering::Greater => {
                    let (path, _) = iter_b.next().unwrap();
                    added.push(path.clone());
                }
            },
            (Some(_), None) => {
                let (path, _) = iter_a.next().unwrap();
                removed.push(path.clone());
            }
            (None, Some(_)) => {
                let (path, _) = iter_b.next().unwrap();
                added.push(path.clone());
            }
            (None, None) => break,
        }
    }

    // BTreeMap iteration is sorted, so output is already sorted — but ensure it.
    added.sort();
    removed.sort();
    modified.sort();

    DiffResult {
        added,
        removed,
        modified,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::core::primitives::Hash;

    fn snapshot_from(entries: &[(&str, &[u8])]) -> Snapshot {
        let files: BTreeMap<String, Hash> = entries
            .iter()
            .map(|(path, content)| ((*path).to_string(), Hash::from_bytes(content)))
            .collect();
        Snapshot::new(files)
    }

    // 1. Identical snapshots produce an empty diff.
    #[test]
    fn identical_snapshots() {
        let s = snapshot_from(&[("src/main.rs", b"fn main() {}"), ("README.md", b"# Pulse")]);
        let diff = diff_snapshots(&s, &s);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert!(diff.modified.is_empty());
        assert!(diff.is_empty());
    }

    // 2. Added file: a is empty, b has one file.
    #[test]
    fn added_file() {
        let a = Snapshot::empty();
        let b = snapshot_from(&[("new.txt", b"hello")]);
        let diff = diff_snapshots(&a, &b);
        assert_eq!(diff.added, vec!["new.txt"]);
        assert!(diff.removed.is_empty());
        assert!(diff.modified.is_empty());
    }

    // 3. Removed file: a has one file, b is empty.
    #[test]
    fn removed_file() {
        let a = snapshot_from(&[("old.txt", b"goodbye")]);
        let b = Snapshot::empty();
        let diff = diff_snapshots(&a, &b);
        assert!(diff.added.is_empty());
        assert_eq!(diff.removed, vec!["old.txt"]);
        assert!(diff.modified.is_empty());
    }

    // 4. Modified file: same path, different hash.
    #[test]
    fn modified_file() {
        let a = snapshot_from(&[("lib.rs", b"version 1")]);
        let b = snapshot_from(&[("lib.rs", b"version 2")]);
        let diff = diff_snapshots(&a, &b);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert_eq!(diff.modified, vec!["lib.rs"]);
    }

    // 5. Mixed: add + remove + modify in one diff.
    #[test]
    fn mixed_changes() {
        let a = snapshot_from(&[
            ("keep.rs", b"unchanged"),
            ("modify.rs", b"old content"),
            ("remove.rs", b"will be removed"),
        ]);
        let b = snapshot_from(&[
            ("add.rs", b"brand new"),
            ("keep.rs", b"unchanged"),
            ("modify.rs", b"new content"),
        ]);
        let diff = diff_snapshots(&a, &b);
        assert_eq!(diff.added, vec!["add.rs"]);
        assert_eq!(diff.removed, vec!["remove.rs"]);
        assert_eq!(diff.modified, vec!["modify.rs"]);
    }

    // 6. Both empty snapshots produce an empty diff.
    #[test]
    fn both_empty() {
        let a = Snapshot::empty();
        let b = Snapshot::empty();
        let diff = diff_snapshots(&a, &b);
        assert!(diff.is_empty());
    }

    // 7. is_empty returns true for identical, false for changed.
    #[test]
    fn is_empty_reflects_changes() {
        let a = snapshot_from(&[("a.rs", b"content")]);
        let b = snapshot_from(&[("a.rs", b"content")]);
        assert!(diff_snapshots(&a, &b).is_empty());

        let c = snapshot_from(&[("a.rs", b"different")]);
        assert!(!diff_snapshots(&a, &c).is_empty());
    }

    // 8. all_changed returns sorted union of all three lists.
    #[test]
    fn all_changed_union() {
        let a = snapshot_from(&[
            ("modify.rs", b"old"),
            ("remove.rs", b"gone"),
        ]);
        let b = snapshot_from(&[
            ("add.rs", b"new"),
            ("modify.rs", b"new"),
        ]);
        let diff = diff_snapshots(&a, &b);
        let all = diff.all_changed();
        assert_eq!(all, vec!["add.rs", "modify.rs", "remove.rs"]);
    }
}
