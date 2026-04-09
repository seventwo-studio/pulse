use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use tokio::sync::broadcast;
use tokio::time::{Duration, interval};

use crate::core::trunk::TrunkManager;
use crate::core::workspace::WorkspaceManager;
use crate::server::state::AppState;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut rx = state.events.subscribe();
    let seq = AtomicU64::new(1);

    // Send connected event with current trunk head and active workspace count.
    {
        let storage = state.storage.lock().await;
        let trunk = TrunkManager::head_id(&storage).ok().flatten();
        let active_workspaces = WorkspaceManager::list(&storage, false).len();

        let connected = serde_json::json!({
            "event": "connected",
            "trunk": trunk,
            "active_workspaces": active_workspaces,
            "seq": seq.fetch_add(1, Ordering::Relaxed),
        });

        if socket
            .send(Message::Text(connected.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }

    let mut ping_interval = interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            // Forward broadcast events as JSON text frames.
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        let mut json = serde_json::to_value(&event).unwrap_or_default();
                        if let Some(obj) = json.as_object_mut() {
                            obj.insert(
                                "seq".to_string(),
                                serde_json::json!(seq.fetch_add(1, Ordering::Relaxed)),
                            );
                        }
                        if socket
                            .send(Message::Text(json.to_string().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Subscriber was too slow — skip missed events and continue.
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            // Keepalive ping every 30 seconds.
            _ = ping_interval.tick() => {
                if socket.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
            }

            // Handle incoming messages (pong, close).
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}
