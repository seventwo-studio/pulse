use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::core::primitives::{Changeset, Snapshot};
use crate::core::trunk::TrunkManager;
use crate::server::error::{ApiResult, error_response};
use crate::server::state::AppState;

/// GET /trunk — return the current trunk head changeset.
pub async fn get_trunk(State(state): State<AppState>) -> ApiResult<Changeset> {
    let storage = state.storage.lock().await;

    let changeset = TrunkManager::head(&storage)
        .map_err(|e| {
            error_response(
                "storage_error",
                &e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?
        .ok_or_else(|| {
            error_response(
                "repo_not_initialized",
                "Repository has not been initialized. POST /repo/init first.",
                StatusCode::BAD_REQUEST,
            )
        })?;

    Ok((StatusCode::OK, Json(changeset)))
}

#[derive(Deserialize)]
pub struct LogQuery {
    author: Option<String>,
    since: Option<String>,
    limit: Option<usize>,
}

/// GET /trunk/log — return trunk history with optional filters.
pub async fn get_log(
    State(state): State<AppState>,
    Query(query): Query<LogQuery>,
) -> ApiResult<Vec<Changeset>> {
    let limit = query.limit.unwrap_or(50).min(1000);

    let since: Option<DateTime<Utc>> = match &query.since {
        Some(s) => {
            let dt = DateTime::parse_from_rfc3339(s).map_err(|_| {
                error_response(
                    "invalid_since",
                    "The 'since' parameter must be a valid RFC 3339 / ISO 8601 timestamp.",
                    StatusCode::BAD_REQUEST,
                )
            })?;
            Some(dt.with_timezone(&Utc))
        }
        None => None,
    };

    let storage = state.storage.lock().await;

    let changesets =
        TrunkManager::log(&storage, limit, query.author.as_deref(), since).map_err(|e| {
            error_response(
                "storage_error",
                &e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

    Ok((StatusCode::OK, Json(changesets)))
}

/// GET /trunk/snapshot — return the current trunk snapshot.
pub async fn get_snapshot(State(state): State<AppState>) -> ApiResult<Snapshot> {
    let storage = state.storage.lock().await;

    let snapshot = TrunkManager::snapshot(&storage)
        .map_err(|e| {
            error_response(
                "storage_error",
                &e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?
        .ok_or_else(|| {
            error_response(
                "repo_not_initialized",
                "Repository has not been initialized. POST /repo/init first.",
                StatusCode::BAD_REQUEST,
            )
        })?;

    Ok((StatusCode::OK, Json(snapshot)))
}
