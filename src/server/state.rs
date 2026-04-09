use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};

use crate::core::overlap::Overlap;
use crate::core::primitives::*;
use crate::storage::engine::StorageEngine;

/// Events broadcast over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Connected {
        trunk: Option<Hash>,
        active_workspaces: usize,
    },
    WorkspaceCreated {
        workspace: Workspace,
        overlaps: Vec<Overlap>,
    },
    WorkspaceCommitted {
        workspace_id: String,
        changeset: Changeset,
        overlaps: Vec<Overlap>,
    },
    OverlapDetected {
        overlaps: Vec<Overlap>,
    },
    DecisionNeeded {
        workspace_id: String,
        conflicting_files: Vec<String>,
        trunk_snapshot: Hash,
        workspace_snapshot: Hash,
    },
    TrunkUpdated {
        changeset: Changeset,
    },
    WorkspaceMerged {
        workspace_id: String,
    },
    WorkspaceAbandoned {
        workspace_id: String,
    },
}

/// Shared application state, passed to all axum handlers via State.
#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<Mutex<StorageEngine>>,
    pub events: broadcast::Sender<Event>,
}

impl AppState {
    pub fn new(storage: StorageEngine) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            storage: Arc::new(Mutex::new(storage)),
            events: tx,
        }
    }

    /// Broadcast an event. Silently ignores if no subscribers.
    pub fn broadcast(&self, event: Event) {
        let _ = self.events.send(event);
    }
}
