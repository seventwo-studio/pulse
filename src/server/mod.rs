pub mod error;
pub mod routes;
pub mod state;
pub mod ws;

use std::path::Path;

use axum::Router;
use state::AppState;

use crate::storage::engine::StorageEngine;

/// Build the axum Router with all routes and shared state.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        // Repo
        .route("/repo/init", axum::routing::post(routes::repo::init))
        .route("/repo/status", axum::routing::get(routes::repo::status))
        // Trunk
        .route("/trunk", axum::routing::get(routes::trunk::get_trunk))
        .route("/trunk/log", axum::routing::get(routes::trunk::get_log))
        .route(
            "/trunk/snapshot",
            axum::routing::get(routes::trunk::get_snapshot),
        )
        // Objects
        .route(
            "/objects/batch",
            axum::routing::post(routes::objects::batch_store),
        )
        .route("/objects/have", axum::routing::post(routes::objects::have))
        .route(
            "/objects/{hash}",
            axum::routing::get(routes::objects::get_object),
        )
        .route("/objects", axum::routing::post(routes::objects::store))
        // Workspaces
        .route(
            "/workspaces/{id}/commit",
            axum::routing::post(routes::workspaces::commit),
        )
        .route(
            "/workspaces/{id}/merge",
            axum::routing::post(routes::workspaces::merge),
        )
        .route(
            "/workspaces/{id}",
            axum::routing::get(routes::workspaces::get_workspace)
                .delete(routes::workspaces::abandon),
        )
        .route(
            "/workspaces",
            axum::routing::post(routes::workspaces::create)
                .get(routes::workspaces::list),
        )
        // Diff & Files
        .route(
            "/diff/{a}/{b}",
            axum::routing::get(routes::diff::get_diff),
        )
        .route(
            "/files/*path",
            axum::routing::get(routes::files::get_file),
        )
        // WebSocket
        .route("/ws", axum::routing::get(ws::ws_handler))
        .with_state(state)
}

/// Start the server on the given address.
pub async fn start(addr: &str, root: &Path, init: bool) -> anyhow::Result<()> {
    let storage = if init {
        StorageEngine::init(root)?
    } else {
        StorageEngine::open(root)?
    };

    let state = AppState::new(storage);
    let router = build_router(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Pulse server listening on {addr}");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    tracing::info!("Shutting down...");
}
