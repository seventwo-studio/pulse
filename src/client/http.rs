use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::Client;
use serde::Deserialize;

use crate::core::diff::DiffResult;
use crate::core::primitives::*;

// ---------------------------------------------------------------------------
// Response types (mirror what the server returns)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct InitResponse {
    pub changeset_id: Hash,
    pub snapshot_id: Hash,
}

#[derive(Deserialize)]
pub struct StatusResponse {
    pub trunk: Hash,
    pub active_workspaces: usize,
}

#[derive(Deserialize)]
pub struct StoreResponse {
    pub hash: Hash,
    pub chunks: Vec<Hash>,
    pub new_chunks: usize,
    pub reused_chunks: usize,
}

#[derive(Deserialize)]
pub struct CreateWorkspaceResponse {
    pub workspace: Workspace,
    pub overlaps: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct CommitResponse {
    pub changeset: Changeset,
    pub stats: CommitStats,
}

#[derive(Deserialize)]
pub struct CommitStats {
    pub new_chunks: usize,
    pub reused_chunks: usize,
}

#[derive(Deserialize)]
pub struct AbandonResponse {
    pub workspace: Workspace,
}

/// Standard error envelope returned by the server.
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

    // -- helpers -------------------------------------------------------------

    /// Check a response for error status and return a descriptive anyhow error.
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

    // -- Repo ----------------------------------------------------------------

    /// POST /repo/init
    pub async fn init_repo(&self) -> anyhow::Result<InitResponse> {
        let resp = self
            .http
            .post(format!("{}/repo/init", self.base_url))
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        Ok(resp.json().await?)
    }

    /// GET /repo/status
    pub async fn status(&self) -> anyhow::Result<StatusResponse> {
        let resp = self
            .http
            .get(format!("{}/repo/status", self.base_url))
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        Ok(resp.json().await?)
    }

    // -- Trunk ---------------------------------------------------------------

    /// GET /trunk
    pub async fn trunk(&self) -> anyhow::Result<Changeset> {
        let resp = self
            .http
            .get(format!("{}/trunk", self.base_url))
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        Ok(resp.json().await?)
    }

    /// GET /trunk/log
    pub async fn trunk_log(
        &self,
        limit: Option<usize>,
        author: Option<&str>,
    ) -> anyhow::Result<Vec<Changeset>> {
        let mut url = format!("{}/trunk/log", self.base_url);
        let mut params = Vec::new();
        if let Some(l) = limit {
            params.push(format!("limit={l}"));
        }
        if let Some(a) = author {
            params.push(format!("author={a}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let resp = self.http.get(&url).send().await?;
        let resp = Self::check(resp).await?;
        Ok(resp.json().await?)
    }

    /// GET /trunk/snapshot
    pub async fn trunk_snapshot(&self) -> anyhow::Result<Snapshot> {
        let resp = self
            .http
            .get(format!("{}/trunk/snapshot", self.base_url))
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        Ok(resp.json().await?)
    }

    // -- Workspaces ----------------------------------------------------------

    /// POST /workspaces
    pub async fn create_workspace(
        &self,
        intent: &str,
        scope: &[String],
        author: &Author,
    ) -> anyhow::Result<CreateWorkspaceResponse> {
        let body = serde_json::json!({
            "intent": intent,
            "scope": scope,
            "author": author,
        });
        let resp = self
            .http
            .post(format!("{}/workspaces", self.base_url))
            .json(&body)
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        Ok(resp.json().await?)
    }

    /// GET /workspaces
    pub async fn list_workspaces(&self, all: bool) -> anyhow::Result<Vec<Workspace>> {
        let url = if all {
            format!("{}/workspaces?all=true", self.base_url)
        } else {
            format!("{}/workspaces", self.base_url)
        };
        let resp = self.http.get(&url).send().await?;
        let resp = Self::check(resp).await?;
        Ok(resp.json().await?)
    }

    /// GET /workspaces/:id
    pub async fn get_workspace(&self, id: &str) -> anyhow::Result<Workspace> {
        let resp = self
            .http
            .get(format!("{}/workspaces/{}", self.base_url, id))
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        Ok(resp.json().await?)
    }

    /// POST /workspaces/:id/commit
    ///
    /// File contents are base64-encoded before sending.
    pub async fn commit(
        &self,
        workspace_id: &str,
        files: HashMap<String, Vec<u8>>,
        message: &str,
        author: &Author,
    ) -> anyhow::Result<CommitResponse> {
        let encoded: HashMap<String, String> = files
            .into_iter()
            .map(|(path, bytes)| (path, STANDARD.encode(bytes)))
            .collect();

        let body = serde_json::json!({
            "files": encoded,
            "message": message,
            "author": author,
        });

        let resp = self
            .http
            .post(format!(
                "{}/workspaces/{}/commit",
                self.base_url, workspace_id
            ))
            .json(&body)
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        Ok(resp.json().await?)
    }

    /// POST /workspaces/:id/merge
    ///
    /// Returns the raw JSON value since the response shape varies between
    /// success and conflict.
    pub async fn merge(&self, workspace_id: &str) -> anyhow::Result<serde_json::Value> {
        let resp = self
            .http
            .post(format!(
                "{}/workspaces/{}/merge",
                self.base_url, workspace_id
            ))
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if status.is_success() {
            Ok(body)
        } else if status == reqwest::StatusCode::CONFLICT {
            // Return the conflict body as-is so the caller can inspect it.
            Ok(body)
        } else {
            // Other error
            let msg = body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("{}: {}", status, msg);
        }
    }

    /// DELETE /workspaces/:id
    pub async fn abandon_workspace(&self, id: &str) -> anyhow::Result<Workspace> {
        let resp = self
            .http
            .delete(format!("{}/workspaces/{}", self.base_url, id))
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        let body: AbandonResponse = resp.json().await?;
        Ok(body.workspace)
    }

    // -- Diff ----------------------------------------------------------------

    /// GET /diff/:a/:b
    pub async fn diff(&self, a: &str, b: &str) -> anyhow::Result<DiffResult> {
        let resp = self
            .http
            .get(format!("{}/diff/{}/{}", self.base_url, a, b))
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        Ok(resp.json().await?)
    }

    // -- Files ---------------------------------------------------------------

    /// GET /files/:path
    ///
    /// Returns the raw file bytes.
    pub async fn get_file(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        let resp = self
            .http
            .get(format!("{}/files/{}", self.base_url, path))
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        Ok(resp.bytes().await?.to_vec())
    }
}
