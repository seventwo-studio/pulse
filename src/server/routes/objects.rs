use std::collections::HashMap;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::core::primitives::Hash;
use crate::server::error::{ApiResult, error_response};
use crate::server::state::AppState;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ObjectResponse {
    r#type: String,
    hash: Hash,
    chunks: Vec<Hash>,
}

#[derive(Serialize)]
pub struct StoreResponse {
    hash: Hash,
    chunks: Vec<Hash>,
    new_chunks: usize,
    reused_chunks: usize,
}

#[derive(Deserialize)]
pub struct BatchStoreRequest {
    files: HashMap<String, String>, // path -> base64 content
}

#[derive(Serialize)]
pub struct BatchStoreResponse {
    blobs: HashMap<String, BlobJson>,
    stats: StatsJson,
}

#[derive(Serialize)]
pub struct BlobJson {
    hash: Hash,
    chunks: Vec<Hash>,
}

#[derive(Serialize)]
pub struct StatsJson {
    new_chunks: usize,
    reused_chunks: usize,
}

#[derive(Deserialize)]
pub struct HaveRequest {
    hashes: Vec<Hash>,
}

#[derive(Serialize)]
pub struct HaveResponse {
    have: Vec<Hash>,
    missing: Vec<Hash>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /objects/:hash
pub async fn get_object(
    State(state): State<AppState>,
    Path(hash_hex): Path<String>,
) -> ApiResult<ObjectResponse> {
    let hash: Hash = hash_hex.parse().map_err(|_| {
        error_response(
            "invalid_hash",
            "hash must be a 64-character hex string",
            StatusCode::BAD_REQUEST,
        )
    })?;

    let storage = state.storage.lock().await;

    let blob = storage.get_blob(&hash).map_err(|_| {
        error_response(
            "object_not_found",
            &format!("no object with hash {hash}"),
            StatusCode::NOT_FOUND,
        )
    })?;

    Ok((
        StatusCode::OK,
        Json(ObjectResponse {
            r#type: "blob".into(),
            hash: blob.hash,
            chunks: blob.chunks.clone(),
        }),
    ))
}

/// POST /objects
pub async fn store(
    State(state): State<AppState>,
    body: Bytes,
) -> ApiResult<StoreResponse> {
    let mut storage = state.storage.lock().await;

    let info = storage.store_file(&body).map_err(|e| {
        error_response(
            "storage_error",
            &format!("failed to store object: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;

    Ok((
        StatusCode::CREATED,
        Json(StoreResponse {
            hash: info.blob.hash,
            chunks: info.blob.chunks,
            new_chunks: info.stats.new_chunks,
            reused_chunks: info.stats.reused_chunks,
        }),
    ))
}

/// POST /objects/batch
pub async fn batch_store(
    State(state): State<AppState>,
    Json(req): Json<BatchStoreRequest>,
) -> ApiResult<BatchStoreResponse> {
    // Decode base64 content for each file.
    let mut decoded: Vec<(String, Vec<u8>)> = Vec::with_capacity(req.files.len());
    for (path, b64) in &req.files {
        let bytes = STANDARD.decode(b64).map_err(|e| {
            error_response(
                "invalid_base64",
                &format!("failed to decode base64 for '{path}': {e}"),
                StatusCode::BAD_REQUEST,
            )
        })?;
        decoded.push((path.clone(), bytes));
    }

    let files: Vec<(&str, &[u8])> = decoded
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();

    let mut storage = state.storage.lock().await;

    let results = storage.store_files(files).map_err(|e| {
        error_response(
            "storage_error",
            &format!("failed to store files: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;

    let mut blobs = HashMap::new();
    let mut total_new = 0usize;
    let mut total_reused = 0usize;

    for (path, info) in results {
        total_new += info.stats.new_chunks;
        total_reused += info.stats.reused_chunks;
        blobs.insert(
            path,
            BlobJson {
                hash: info.blob.hash,
                chunks: info.blob.chunks,
            },
        );
    }

    Ok((
        StatusCode::CREATED,
        Json(BatchStoreResponse {
            blobs,
            stats: StatsJson {
                new_chunks: total_new,
                reused_chunks: total_reused,
            },
        }),
    ))
}

/// POST /objects/have
pub async fn have(
    State(state): State<AppState>,
    Json(req): Json<HaveRequest>,
) -> ApiResult<HaveResponse> {
    let storage = state.storage.lock().await;
    let (have, missing) = storage.have_objects(&req.hashes);

    Ok((
        StatusCode::OK,
        Json(HaveResponse { have, missing }),
    ))
}
