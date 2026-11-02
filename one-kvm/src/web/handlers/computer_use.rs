use axum::{
    extract::{ws::WebSocketUpgrade, Query, State},
    response::Response,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::computer_use::{
    ComputerUseConfigResponse, ComputerUseConfigUpdate, ComputerUseSessionSummary,
    ComputerUseStartRequest,
};
use crate::error::Result;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ComputerUseWsQuery {
    client_id: Option<String>,
}

pub async fn computer_use_config(
    State(state): State<Arc<AppState>>,
) -> Json<ComputerUseConfigResponse> {
    Json(state.computer_use.config_response())
}

pub async fn computer_use_update_config(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ComputerUseConfigUpdate>,
) -> Result<Json<ComputerUseConfigResponse>> {
    Ok(Json(state.computer_use.update_config(req).await?))
}

pub async fn computer_use_session(
    State(state): State<Arc<AppState>>,
) -> Json<ComputerUseSessionSummary> {
    Json(state.computer_use.summary().await)
}

pub async fn computer_use_start(
    State(state): State<Arc<AppState>>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
    Json(mut req): Json<ComputerUseStartRequest>,
) -> Result<Json<ComputerUseSessionSummary>> {
    req.client_id = format!("{}:{}", grant.controller().device_id, req.client_id);
    Ok(Json(state.computer_use.start(req, grant).await?))
}

pub async fn computer_use_stop(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ComputerUseSessionSummary>> {
    Ok(Json(state.computer_use.stop().await?))
}

pub async fn computer_use_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(mut query): Query<ComputerUseWsQuery>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
) -> Response {
    query.client_id = query
        .client_id
        .map(|id| format!("{}:{id}", grant.controller().device_id));
    ws.on_upgrade(move |socket| async move {
        tokio::select! {
            biased;
            _ = grant.cancelled() => {},
            _ = state.computer_use.clone().handle_socket(socket, query.client_id) => {},
        }
    })
}
