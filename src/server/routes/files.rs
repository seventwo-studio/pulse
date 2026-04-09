use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::core::primitives::Hash;
use crate::core::trunk::TrunkManager;
use crate::server::error::{ErrorBody, error_response};
use crate::server::state::AppState;

#[derive(Deserialize)]
pub struct FileQuery {
    pub snapshot: Option<String>,
}

pub async fn get_file(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorBody>)> {
    let storage = state.storage.lock().await;

    // Determine which snapshot to read from.
    let snapshot_id: Hash = match query.snapshot {
        Some(ref hex) => hex.parse().map_err(|_| {
            error_response(
                "invalid_hash",
                &format!("'{}' is not a valid 64-character hex hash", hex),
                StatusCode::BAD_REQUEST,
            )
        })?,
        None => {
            // Use the trunk head's snapshot.
            let head = TrunkManager::head(&storage)
                .map_err(|e| {
                    error_response(
                        "internal_error",
                        &e.to_string(),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )
                })?
                .ok_or_else(|| {
                    error_response(
                        "repo_not_initialized",
                        "Repository has not been initialized. Call POST /repo/init first.",
                        StatusCode::BAD_REQUEST,
                    )
                })?;
            head.snapshot
        }
    };

    let bytes = storage.read_file_by_path(&snapshot_id, &path).map_err(|e| {
        // Distinguish not-found from other errors.
        let msg = e.to_string();
        if msg.contains("not found") || msg.contains("NotFound") {
            error_response(
                "file_not_found",
                &format!("file '{}' not found in snapshot {}", path, snapshot_id),
                StatusCode::NOT_FOUND,
            )
        } else {
            error_response(
                "internal_error",
                &msg,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        }
    })?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    ))
}
