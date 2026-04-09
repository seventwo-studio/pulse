use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::core::primitives::*;
use crate::server::error::{ApiResult, error_response};
use crate::server::state::AppState;

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Manifest capturing all metadata needed to recreate a repository.
/// Chunk data is excluded (too large); only metadata objects are included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferManifest {
    pub trunk: Option<Hash>,
    pub blobs: Vec<Blob>,
    pub snapshots: Vec<Snapshot>,
    pub changesets: Vec<Changeset>,
    pub workspaces: Vec<Workspace>,
}

#[derive(Serialize)]
pub struct ExportResponse {
    pub manifest: TransferManifest,
    pub stats: ExportStats,
}

#[derive(Serialize)]
pub struct ExportStats {
    pub blobs: usize,
    pub snapshots: usize,
    pub changesets: usize,
    pub workspaces: usize,
}

/// POST /repo/export
///
/// Export the full repository metadata as a JSON manifest.
/// This can be fed into POST /repo/import on another server to recreate
/// the repository state. Chunk data is not included — only metadata.
pub async fn export(State(state): State<AppState>) -> ApiResult<ExportResponse> {
    let storage = state.storage.lock().await;

    let trunk = storage.get_trunk().map_err(|e| {
        error_response(
            "internal_error",
            &e.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;

    let blobs: Vec<Blob> = storage.list_blobs().cloned().collect();
    let snapshots: Vec<Snapshot> = storage.list_snapshots().cloned().collect();
    let changesets: Vec<Changeset> = storage.list_changesets().cloned().collect();
    let workspaces: Vec<Workspace> = storage
        .list_workspaces(true)
        .into_iter()
        .cloned()
        .collect();

    let stats = ExportStats {
        blobs: blobs.len(),
        snapshots: snapshots.len(),
        changesets: changesets.len(),
        workspaces: workspaces.len(),
    };

    let manifest = TransferManifest {
        trunk,
        blobs,
        snapshots,
        changesets,
        workspaces,
    };

    Ok((StatusCode::OK, Json(ExportResponse { manifest, stats })))
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ImportResponse {
    pub imported: ImportStats,
}

#[derive(Serialize)]
pub struct ImportStats {
    pub blobs: usize,
    pub snapshots: usize,
    pub changesets: usize,
    pub workspaces: usize,
    pub trunk_set: bool,
}

/// POST /repo/import
///
/// Accept a transfer manifest and replay all objects into the local storage
/// engine. This is intended for one-time migration into a fresh repository.
/// If the repository already has a trunk, the import is rejected to prevent
/// accidental overwrites.
pub async fn import(
    State(state): State<AppState>,
    Json(manifest): Json<TransferManifest>,
) -> ApiResult<ImportResponse> {
    let mut storage = state.storage.lock().await;

    // Guard: refuse to import into an already-populated repo
    let existing_trunk = storage.get_trunk().map_err(|e| {
        error_response(
            "internal_error",
            &e.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;

    if existing_trunk.is_some() {
        return Err(error_response(
            "repo_not_empty",
            "Cannot import into a repository that already has a trunk. Use a fresh repo.",
            StatusCode::CONFLICT,
        ));
    }

    // Replay blobs
    let mut blob_count = 0;
    for blob in &manifest.blobs {
        storage.store_blob(blob).map_err(|e| {
            error_response(
                "internal_error",
                &format!("failed to store blob {}: {}", blob.hash, e),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        blob_count += 1;
    }

    // Replay snapshots
    let mut snapshot_count = 0;
    for snapshot in &manifest.snapshots {
        storage.store_snapshot(snapshot).map_err(|e| {
            error_response(
                "internal_error",
                &format!("failed to store snapshot {}: {}", snapshot.id, e),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        snapshot_count += 1;
    }

    // Replay changesets
    let mut changeset_count = 0;
    for changeset in &manifest.changesets {
        storage.store_changeset(changeset).map_err(|e| {
            error_response(
                "internal_error",
                &format!("failed to store changeset {}: {}", changeset.id, e),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        changeset_count += 1;
    }

    // Replay workspaces
    let mut workspace_count = 0;
    for workspace in &manifest.workspaces {
        storage.store_workspace(workspace).map_err(|e| {
            error_response(
                "internal_error",
                &format!("failed to store workspace {}: {}", workspace.id, e),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        workspace_count += 1;
    }

    // Set trunk
    let trunk_set = if let Some(trunk_id) = &manifest.trunk {
        storage.set_trunk(trunk_id).map_err(|e| {
            error_response(
                "internal_error",
                &format!("failed to set trunk: {}", e),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        true
    } else {
        false
    };

    Ok((
        StatusCode::CREATED,
        Json(ImportResponse {
            imported: ImportStats {
                blobs: blob_count,
                snapshots: snapshot_count,
                changesets: changeset_count,
                workspaces: workspace_count,
                trunk_set,
            },
        }),
    ))
}

// ---------------------------------------------------------------------------
// Transfer (source-initiated push)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct TransferRequest {
    pub target_url: String,
}

#[derive(Serialize)]
pub struct TransferResponse {
    pub target_url: String,
    pub exported: ExportStats,
    pub import_result: serde_json::Value,
}

/// POST /repo/transfer
///
/// One-shot push transfer: export this repo's manifest, then POST it to
/// `{target_url}/repo/import`. Returns the combined stats.
pub async fn transfer(
    State(state): State<AppState>,
    Json(body): Json<TransferRequest>,
) -> ApiResult<TransferResponse> {
    let storage = state.storage.lock().await;

    // Build the manifest
    let trunk = storage.get_trunk().map_err(|e| {
        error_response(
            "internal_error",
            &e.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;

    let blobs: Vec<Blob> = storage.list_blobs().cloned().collect();
    let snapshots: Vec<Snapshot> = storage.list_snapshots().cloned().collect();
    let changesets: Vec<Changeset> = storage.list_changesets().cloned().collect();
    let workspaces: Vec<Workspace> = storage
        .list_workspaces(true)
        .into_iter()
        .cloned()
        .collect();

    let stats = ExportStats {
        blobs: blobs.len(),
        snapshots: snapshots.len(),
        changesets: changesets.len(),
        workspaces: workspaces.len(),
    };

    let manifest = TransferManifest {
        trunk,
        blobs,
        snapshots,
        changesets,
        workspaces,
    };

    // Drop the lock before making the outbound HTTP call
    drop(storage);

    // POST the manifest to the target
    let target_import_url = format!("{}/repo/import", body.target_url.trim_end_matches('/'));

    let http_client = reqwest::Client::new();
    let response = http_client
        .post(&target_import_url)
        .json(&manifest)
        .send()
        .await
        .map_err(|e| {
            error_response(
                "transfer_failed",
                &format!("failed to reach target: {}", e),
                StatusCode::BAD_GATEWAY,
            )
        })?;

    let status = response.status();
    let response_body: serde_json::Value = response.json().await.map_err(|e| {
        error_response(
            "transfer_failed",
            &format!("failed to parse target response: {}", e),
            StatusCode::BAD_GATEWAY,
        )
    })?;

    if !status.is_success() {
        let msg = response_body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error from target");
        return Err(error_response(
            "transfer_rejected",
            &format!("target rejected import: {}", msg),
            StatusCode::BAD_GATEWAY,
        ));
    }

    Ok((
        StatusCode::OK,
        Json(TransferResponse {
            target_url: body.target_url,
            exported: stats,
            import_result: response_body,
        }),
    ))
}
