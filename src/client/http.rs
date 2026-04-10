use std::collections::HashMap;

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::core::primitives::*;

// ---------------------------------------------------------------------------
// Sync bundle — the unit of exchange between client and server
// ---------------------------------------------------------------------------

/// A bundle of objects exchanged during push/pull.
#[derive(Debug, Serialize, Deserialize)]
pub struct SyncBundle {
    /// Main changeset hash.
    pub main: Hash,
    /// Changesets in topological order (oldest first).
    pub changesets: Vec<Changeset>,
    /// Snapshots referenced by the changesets.
    pub snapshots: Vec<Snapshot>,
    /// Workspaces.
    pub workspaces: Vec<Workspace>,
    /// File content keyed by blob hash (hex) → base64-encoded bytes.
    pub files: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct SyncPushResponse {
    #[allow(dead_code)]
    pub main: Hash,
}

// ---------------------------------------------------------------------------
// Error envelope
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    message: String,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub struct PulseClient {
    base_url: String,
    http: Client,
}

impl PulseClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http: Client::new(),
        }
    }

    async fn check(resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if let Ok(err) = serde_json::from_str::<ErrorBody>(&body) {
            anyhow::bail!("{}: {}", status, err.error.message);
        }
        anyhow::bail!("{}: {}", status, body);
    }

    // -- Sync ----------------------------------------------------------------

    /// Push a bundle of local objects to the remote server.
    pub async fn sync_push(&self, bundle: &SyncBundle) -> anyhow::Result<SyncPushResponse> {
        let resp = self
            .http
            .post(format!("{}/sync/push", self.base_url))
            .json(bundle)
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        Ok(resp.json().await?)
    }

    /// Pull objects from the remote server.
    /// `have_main` is our current main hash (or None if we have nothing).
    pub async fn sync_pull(
        &self,
        have_main: Option<&Hash>,
    ) -> anyhow::Result<SyncBundle> {
        let body = serde_json::json!({
            "have_main": have_main,
        });
        let resp = self
            .http
            .post(format!("{}/sync/pull", self.base_url))
            .json(&body)
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        Ok(resp.json().await?)
    }
}
