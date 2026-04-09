use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use crate::core::diff::{DiffResult, diff_snapshots};
use crate::core::primitives::{Hash, Snapshot};
use crate::server::error::{ApiResult, error_response};
use crate::server::state::AppState;
use crate::storage::engine::StorageEngine;

/// Resolve a hash to a snapshot: try as changeset first (use its snapshot),
/// then fall back to treating the hash as a snapshot id directly.
fn resolve_snapshot<'a>(storage: &'a StorageEngine, hash: &Hash) -> Result<&'a Snapshot, (StatusCode, Json<crate::server::error::ErrorBody>)> {
    // Try as changeset first — if it is one, follow its snapshot pointer.
    if let Ok(cs) = storage.get_changeset(hash) {
        return storage.get_snapshot(&cs.snapshot).map_err(|e| {
            error_response(
                "internal_error",
                &format!("changeset {} references missing snapshot: {}", hash, e),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        });
    }

    // Try as a snapshot directly.
    storage.get_snapshot(hash).map_err(|_| {
        error_response(
            "not_found",
            &format!("hash {} is neither a known changeset nor snapshot", hash),
            StatusCode::NOT_FOUND,
        )
    })
}

pub async fn get_diff(
    State(state): State<AppState>,
    Path((a, b)): Path<(String, String)>,
) -> ApiResult<DiffResult> {
    let hash_a: Hash = a.parse().map_err(|_| {
        error_response(
            "invalid_hash",
            &format!("'{}' is not a valid 64-character hex hash", a),
            StatusCode::BAD_REQUEST,
        )
    })?;
    let hash_b: Hash = b.parse().map_err(|_| {
        error_response(
            "invalid_hash",
            &format!("'{}' is not a valid 64-character hex hash", b),
            StatusCode::BAD_REQUEST,
        )
    })?;

    let storage = state.storage.lock().await;

    let snap_a = resolve_snapshot(&storage, &hash_a)?;
    let snap_b = resolve_snapshot(&storage, &hash_b)?;

    let diff = diff_snapshots(snap_a, snap_b);

    Ok((StatusCode::OK, Json(diff)))
}
