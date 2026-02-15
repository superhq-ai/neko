use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::agent::Agent;

pub struct AppState {
    pub agent: Mutex<Agent>,
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
}

#[derive(Serialize)]
pub struct MessageResponse {
    pub response: String,
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
    let mut agent = state.agent.lock().await;
    match agent.run_turn(&req.text).await {
        Ok(response) => Ok(Json(MessageResponse { response })),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
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
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .with_state(state)
}
