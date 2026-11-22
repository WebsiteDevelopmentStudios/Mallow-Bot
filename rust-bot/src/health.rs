use axum::{routing::get, Json, Router};
use serde::Serialize;
use std::time::Instant;

#[derive(Clone)]
pub struct HealthState { pub started: Instant }

#[derive(Serialize)]
struct HealthResponse { status: &'static str, runtime: &'static str, uptime_seconds: u64 }

async fn health(axum::extract::State(state): axum::extract::State<HealthState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        runtime: "rust-migration",
        uptime_seconds: state.started.elapsed().as_secs(),
    })
}

pub fn router(state: HealthState) -> Router {
    Router::new().route("/health", get(health)).route("/", get(health)).with_state(state)
}
