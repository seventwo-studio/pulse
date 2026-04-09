use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::core::merge::{MergeEngine, MergeResult};
use crate::core::overlap::{detect_file_overlaps, detect_scope_overlaps, Overlap};
use crate::core::primitives::*;
use crate::core::trunk::TrunkManager;
use crate::core::workspace::WorkspaceManager;
use crate::server::error::{ApiResult, error_response};
use crate::server::state::{AppState, Event};

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateRequest {
    intent: String,
    scope: Vec<String>,
    author: Author,
}

#[derive(Serialize)]
pub struct CreateResponse {
    workspace: Workspace,
    overlaps: Vec<Overlap>,
}

#[derive(Deserialize)]
pub struct ListQuery {
    all: Option<bool>,
}

#[derive(Deserialize)]
pub struct CommitRequest {
    files: HashMap<String, String>, // path -> base64 content
    message: String,
    author: Author,
}

#[derive(Serialize)]
pub struct CommitStats {
    new_chunks: usize,
    reused_chunks: usize,
}

#[derive(Serialize)]
pub struct CommitResponse {
    changeset: Changeset,
    stats: CommitStats,
}

#[derive(Serialize)]
pub struct MergeSuccessResponse {
    changeset: Changeset,
}

#[derive(Serialize)]
pub struct MergeConflictResponse {
    error: MergeConflictError,
    conflicting_files: Vec<String>,
    trunk_snapshot: Hash,
    workspace_snapshot: Hash,
}

#[derive(Serialize)]
pub struct MergeConflictError {
    code: String,
    message: String,
    status: u16,
}

#[derive(Serialize)]
pub struct AbandonResponse {
    workspace: Workspace,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /workspaces
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateRequest>,
) -> ApiResult<CreateResponse> {
    let mut storage = state.storage.lock().await;

    let trunk_head = TrunkManager::head_id(&storage)
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

    let workspace =
        WorkspaceManager::create(&mut storage, body.intent, body.scope, body.author, &trunk_head)
            .map_err(|e| {
                error_response(
                    "internal_error",
                    &e.to_string(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            })?;

    // Detect scope overlaps against other active workspaces
    let active = WorkspaceManager::list(&storage, false);
    let overlaps = detect_scope_overlaps(&workspace, &active);

    state.broadcast(Event::WorkspaceCreated {
        workspace: workspace.clone(),
        overlaps: overlaps.clone(),
    });

    Ok((
        StatusCode::CREATED,
        Json(CreateResponse {
            workspace,
            overlaps,
        }),
    ))
}

/// GET /workspaces
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Vec<Workspace>> {
    let storage = state.storage.lock().await;
    let all = query.all.unwrap_or(false);
    let workspaces = WorkspaceManager::list(&storage, all);
    Ok((StatusCode::OK, Json(workspaces)))
}

/// GET /workspaces/:id
pub async fn get_workspace(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Workspace> {
    let storage = state.storage.lock().await;

    let workspace = WorkspaceManager::get(&storage, &id).map_err(|_| {
        error_response(
            "workspace_not_found",
            &format!("Workspace {id} not found."),
            StatusCode::NOT_FOUND,
        )
    })?;

    Ok((StatusCode::OK, Json(workspace)))
}

/// POST /workspaces/:id/commit
pub async fn commit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CommitRequest>,
) -> ApiResult<CommitResponse> {
    // Decode base64 files
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(body.files.len());
    for (path, b64) in &body.files {
        let bytes = STANDARD.decode(b64).map_err(|e| {
            error_response(
                "invalid_base64",
                &format!("Failed to decode base64 for {path}: {e}"),
                StatusCode::BAD_REQUEST,
            )
        })?;
        files.push((path.clone(), bytes));
    }

    let mut storage = state.storage.lock().await;

    let result =
        WorkspaceManager::commit(&mut storage, &id, files, body.message, body.author).map_err(
            |e| {
                let msg = e.to_string();
                if msg.contains("not found") || msg.contains("not active") {
                    error_response("workspace_not_found", &msg, StatusCode::NOT_FOUND)
                } else {
                    error_response("internal_error", &msg, StatusCode::INTERNAL_SERVER_ERROR)
                }
            },
        )?;

    // Detect file-level overlaps against other active workspaces
    let committed_files = &result.changeset.files_changed;
    let active = WorkspaceManager::list(&storage, false);

    let other_workspaces: Vec<(String, Vec<String>)> = active
        .iter()
        .filter(|ws| ws.id != id)
        .map(|ws| {
            let mut changed: Vec<String> = Vec::new();
            for cs_id in &ws.changesets {
                if let Ok(cs) = storage.get_changeset(cs_id) {
                    for f in &cs.files_changed {
                        changed.push(f.clone());
                    }
                }
            }
            changed.sort();
            changed.dedup();
            (ws.id.clone(), changed)
        })
        .collect();

    let overlaps = detect_file_overlaps(&id, committed_files, &other_workspaces);

    // Broadcast commit event (includes overlaps)
    state.broadcast(Event::WorkspaceCommitted {
        workspace_id: id.clone(),
        changeset: result.changeset.clone(),
        overlaps: overlaps.clone(),
    });

    // If overlaps detected, also broadcast a separate overlap event
    if !overlaps.is_empty() {
        state.broadcast(Event::OverlapDetected {
            overlaps: overlaps.clone(),
        });
    }

    Ok((
        StatusCode::CREATED,
        Json(CommitResponse {
            changeset: result.changeset,
            stats: CommitStats {
                new_chunks: result.stats.new_chunks,
                reused_chunks: result.stats.reused_chunks,
            },
        }),
    ))
}

/// POST /workspaces/:id/merge
pub async fn merge(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let mut storage = state.storage.lock().await;

    let result = MergeEngine::merge(&mut storage, &id).map_err(|e| {
        let msg = e.to_string();
        let (code, status) = if msg.contains("not found") || msg.contains("not active") {
            ("workspace_not_found", StatusCode::NOT_FOUND)
        } else {
            ("internal_error", StatusCode::INTERNAL_SERVER_ERROR)
        };
        (
            status,
            Json(serde_json::json!({
                "error": { "code": code, "message": msg, "status": status.as_u16() }
            })),
        )
    })?;

    match result {
        MergeResult::Success { changeset } => {
            state.broadcast(Event::TrunkUpdated {
                changeset: changeset.clone(),
            });
            state.broadcast(Event::WorkspaceMerged {
                workspace_id: id.clone(),
            });

            Ok((
                StatusCode::OK,
                Json(serde_json::json!({ "changeset": changeset })),
            ))
        }
        MergeResult::Conflict {
            conflicting_files,
            trunk_snapshot,
            workspace_snapshot,
        } => {
            state.broadcast(Event::DecisionNeeded {
                workspace_id: id.clone(),
                conflicting_files: conflicting_files.clone(),
                trunk_snapshot,
                workspace_snapshot,
            });

            Err((
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": {
                        "code": "merge_conflict",
                        "message": format!("Merge conflict in workspace {id}"),
                        "status": 409
                    },
                    "conflicting_files": conflicting_files,
                    "trunk_snapshot": trunk_snapshot,
                    "workspace_snapshot": workspace_snapshot,
                })),
            ))
        }
    }
}

/// DELETE /workspaces/:id
pub async fn abandon(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<AbandonResponse> {
    let mut storage = state.storage.lock().await;

    let workspace = WorkspaceManager::abandon(&mut storage, &id).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("not found") || msg.contains("not active") {
            error_response("workspace_not_found", &msg, StatusCode::NOT_FOUND)
        } else {
            error_response("internal_error", &msg, StatusCode::INTERNAL_SERVER_ERROR)
        }
    })?;

    state.broadcast(Event::WorkspaceAbandoned {
        workspace_id: id.clone(),
    });

    Ok((StatusCode::OK, Json(AbandonResponse { workspace })))
}
