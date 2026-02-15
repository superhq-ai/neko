use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::gateway::Gateway;

pub struct AppState {
    pub gateway: Arc<Gateway>,
    pub api_token: Option<String>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

#[derive(Deserialize)]
pub struct MessageRequest {
    pub text: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Serialize)]
pub struct MessageResponse {
    pub response: String,
    pub session_id: String,
}

#[derive(Serialize)]
struct SessionListEntry {
    session_id: String,
    key: String,
    turn_count: u32,
    input_tokens: u32,
    output_tokens: u32,
    updated_at: String,
    channel: Option<String>,
    display_name: Option<String>,
}

#[derive(Serialize)]
struct SessionListResponse {
    sessions: Vec<SessionListEntry>,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn send_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MessageRequest>,
) -> Result<Json<MessageResponse>, (StatusCode, String)> {
    let (response, session_id) = state
        .gateway
        .handle_http_message(&req.text, req.session_id.as_deref(), None)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(MessageResponse {
        response,
        session_id,
    }))
}

async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Json<SessionListResponse> {
    let metas = state.gateway.session_store.list().await;
    let sessions = metas
        .into_iter()
        .map(|m| SessionListEntry {
            session_id: m.session_id,
            key: m.key,
            turn_count: m.turn_count,
            input_tokens: m.input_tokens,
            output_tokens: m.output_tokens,
            updated_at: m.updated_at.to_rfc3339(),
            channel: m.channel,
            display_name: m.display_name,
        })
        .collect();
    Json(SessionListResponse { sessions })
}

async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .gateway
        .session_store
        .delete(&session_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if let Some(expected) = &state.api_token {
        let auth = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());

        match auth {
            Some(val) if val.starts_with("Bearer ") && &val[7..] == expected.as_str() => {}
            _ => return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response(),
        }
    }
    next.run(request).await
}

pub fn router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/api/v1/message", post(send_message))
        .route("/api/v1/sessions", get(list_sessions))
        .route("/api/v1/sessions/{id}", delete(delete_session))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .with_state(state)
}
