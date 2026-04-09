use serde::{Deserialize, Serialize};

use crate::core::primitives::Workspace;

/// Detected overlap between two workspaces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Overlap {
    pub workspace_a: String,
    pub workspace_b: String,
    /// Pairs of (scope_a, scope_b) that overlap.
    pub overlapping_scopes: Vec<(String, String)>,
    /// Specific file paths both workspaces touch.
    pub overlapping_files: Vec<String>,
}

/// Extract the directory prefix from a scope pattern.
///
/// Strips a trailing `*` to get the prefix. If there is no `*`, the whole
/// string is treated as the prefix.
fn scope_prefix(scope: &str) -> &str {
    scope.strip_suffix('*').unwrap_or(scope)
}

/// Check if two scope patterns overlap using prefix matching.
///
/// Rules:
/// - `"dir/*"` matches any path starting with `"dir/"`.
/// - Two scopes overlap if one prefix starts with the other.
/// - `"src/auth/*"` and `"src/auth/jwt.rs"` overlap.
/// - `"src/*"` and `"src/auth/*"` overlap (parent contains child).
/// - `"src/auth/*"` and `"src/db/*"` do NOT overlap.
pub fn scopes_overlap(a: &str, b: &str) -> bool {
    let pa = scope_prefix(a);
    let pb = scope_prefix(b);
    pa.starts_with(pb) || pb.starts_with(pa)
}

/// Detect scope-level overlaps between a workspace and a list of other active
/// workspaces.
///
/// For every pair of scopes where one workspace's scope overlaps another's,
/// the pair is recorded. Returns one [`Overlap`] per other workspace that has
/// at least one overlapping scope pair.
pub fn detect_scope_overlaps(workspace: &Workspace, others: &[Workspace]) -> Vec<Overlap> {
    let mut results = Vec::new();

    for other in others {
        if other.id == workspace.id {
            continue;
        }

        let mut overlapping_scopes = Vec::new();

        for sa in &workspace.scope {
            for sb in &other.scope {
                if scopes_overlap(sa, sb) {
                    overlapping_scopes.push((sa.clone(), sb.clone()));
                }
            }
        }

        if !overlapping_scopes.is_empty() {
            results.push(Overlap {
                workspace_a: workspace.id.clone(),
                workspace_b: other.id.clone(),
                overlapping_scopes,
                overlapping_files: Vec::new(),
            });
        }
    }

    results
}

#[allow(dead_code)]
/// Detect file-level overlaps given pre-computed changed-file lists.
///
/// `workspace_changed_files` is the list of files changed by the target
/// workspace. `other_workspaces` is a slice of `(workspace_id, changed_files)`
/// for every other workspace to check against.
///
/// Returns one [`Overlap`] per other workspace that shares at least one
/// changed file.
pub fn detect_file_overlaps(
    workspace_id: &str,
    workspace_changed_files: &[String],
    other_workspaces: &[(String, Vec<String>)],
) -> Vec<Overlap> {
    let mut results = Vec::new();

    for (other_id, other_files) in other_workspaces {
        if other_id == workspace_id {
            continue;
        }

        let mut overlapping_files: Vec<String> = workspace_changed_files
            .iter()
            .filter(|f| other_files.contains(f))
            .cloned()
            .collect();

        overlapping_files.sort();
        overlapping_files.dedup();

        if !overlapping_files.is_empty() {
            results.push(Overlap {
                workspace_a: workspace_id.to_string(),
                workspace_b: other_id.clone(),
                overlapping_scopes: Vec::new(),
                overlapping_files,
            });
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::primitives::{Author, Hash, Workspace};

    fn make_workspace(id: &str, scopes: Vec<&str>) -> Workspace {
        let mut ws = Workspace::new(
            Hash::from_bytes(b"test"),
            "test workspace".into(),
            scopes.into_iter().map(String::from).collect(),
            Author::human("test"),
        );
        ws.id = id.to_string();
        ws
    }

    // 1. Disjoint scopes do not overlap.
    #[test]
    fn disjoint_scopes_no_overlap() {
        assert!(!scopes_overlap("src/auth/*", "src/db/*"));
    }

    // 2. A glob scope overlaps a specific file within it.
    #[test]
    fn scope_overlaps_file_within() {
        assert!(scopes_overlap("src/auth/*", "src/auth/jwt.rs"));
    }

    // 3. Parent scope overlaps child scope.
    #[test]
    fn parent_child_scopes_overlap() {
        assert!(scopes_overlap("src/*", "src/auth/*"));
    }

    // 4. Identical scopes overlap.
    #[test]
    fn identical_scopes_overlap() {
        assert!(scopes_overlap("src/auth/*", "src/auth/*"));
    }

    // 5. File overlap: two workspaces both change the same file.
    #[test]
    fn file_overlap_detected() {
        let overlaps = detect_file_overlaps(
            "ws-aaaa",
            &["src/auth/jwt.rs".into(), "src/main.rs".into()],
            &[(
                "ws-bbbb".into(),
                vec!["src/auth/jwt.rs".into(), "src/db/pool.rs".into()],
            )],
        );

        assert_eq!(overlaps.len(), 1);
        assert_eq!(overlaps[0].workspace_a, "ws-aaaa");
        assert_eq!(overlaps[0].workspace_b, "ws-bbbb");
        assert_eq!(overlaps[0].overlapping_files, vec!["src/auth/jwt.rs"]);
        assert!(overlaps[0].overlapping_scopes.is_empty());
    }

    // 6. No other workspaces returns empty vec.
    #[test]
    fn no_others_returns_empty() {
        let ws = make_workspace("ws-aaaa", vec!["src/auth/*"]);
        let overlaps = detect_scope_overlaps(&ws, &[]);
        assert!(overlaps.is_empty());
    }

    // 7. Workspace overlaps with two others produces two Overlap entries.
    #[test]
    fn multiple_overlaps() {
        let ws = make_workspace("ws-aaaa", vec!["src/*"]);
        let others = vec![
            make_workspace("ws-bbbb", vec!["src/auth/*"]),
            make_workspace("ws-cccc", vec!["src/db/*"]),
        ];

        let overlaps = detect_scope_overlaps(&ws, &others);
        assert_eq!(overlaps.len(), 2);

        assert_eq!(overlaps[0].workspace_b, "ws-bbbb");
        assert_eq!(
            overlaps[0].overlapping_scopes,
            vec![("src/*".to_string(), "src/auth/*".to_string())]
        );

        assert_eq!(overlaps[1].workspace_b, "ws-cccc");
        assert_eq!(
            overlaps[1].overlapping_scopes,
            vec![("src/*".to_string(), "src/db/*".to_string())]
        );
    }

    // 8. Mixed: scope overlap with one workspace, file overlap with another.
    #[test]
    fn mixed_scope_and_file_overlaps() {
        // Scope overlap: ws-aaaa scopes overlap with ws-bbbb scopes.
        let ws = make_workspace("ws-aaaa", vec!["src/auth/*"]);
        let scope_other = make_workspace("ws-bbbb", vec!["src/auth/jwt.rs"]);

        let scope_overlaps = detect_scope_overlaps(&ws, &[scope_other]);
        assert_eq!(scope_overlaps.len(), 1);
        assert_eq!(scope_overlaps[0].workspace_b, "ws-bbbb");
        assert!(!scope_overlaps[0].overlapping_scopes.is_empty());
        assert!(scope_overlaps[0].overlapping_files.is_empty());

        // File overlap: ws-aaaa changed files overlap with ws-cccc changed files.
        let file_overlaps = detect_file_overlaps(
            "ws-aaaa",
            &["README.md".into()],
            &[("ws-cccc".into(), vec!["README.md".into()])],
        );
        assert_eq!(file_overlaps.len(), 1);
        assert_eq!(file_overlaps[0].workspace_b, "ws-cccc");
        assert!(file_overlaps[0].overlapping_scopes.is_empty());
        assert_eq!(file_overlaps[0].overlapping_files, vec!["README.md"]);
    }
}
