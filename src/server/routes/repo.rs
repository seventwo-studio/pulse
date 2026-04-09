use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use crate::core::primitives::Hash;
use crate::core::trunk::TrunkManager;
use crate::core::workspace::WorkspaceManager;
use crate::server::error::{ApiResult, error_response};
use crate::server::state::AppState;

#[derive(Serialize)]
pub struct InitResponse {
    changeset_id: Hash,
    snapshot_id: Hash,
}

#[derive(Serialize)]
pub struct StatusResponse {
    trunk: Hash,
    active_workspaces: usize,
}

pub async fn init(State(state): State<AppState>) -> ApiResult<InitResponse> {
    let mut storage = state.storage.lock().await;

    // Check if already initialized
    if TrunkManager::head_id(&storage)
        .ok()
        .flatten()
        .is_some()
    {
        return Err(error_response(
            "repo_already_initialized",
            "Repository has already been initialized.",
            StatusCode::CONFLICT,
        ));
    }

    let changeset = TrunkManager::init_repo(&mut storage).map_err(|e| {
        error_response(
            "internal_error",
            &e.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;

    Ok((
        StatusCode::CREATED,
        Json(InitResponse {
            changeset_id: changeset.id,
            snapshot_id: changeset.snapshot,
        }),
    ))
}

pub async fn status(State(state): State<AppState>) -> ApiResult<StatusResponse> {
    let storage = state.storage.lock().await;

    let trunk = TrunkManager::head_id(&storage)
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

    let active_workspaces = WorkspaceManager::list(&storage, false).len();

    Ok((
        StatusCode::OK,
        Json(StatusResponse {
            trunk,
            active_workspaces,
        }),
    ))
}
