use axum::{routing::get, Json, Router};
use serde::Serialize;
use std::{sync::{atomic::{AtomicBool, Ordering}, Arc}, time::Instant};

#[derive(Clone)]
pub struct HealthState {
    pub started: Instant,
    pub ready: Arc<AtomicBool>,
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
    bot_ready: bool,
    architecture: &'static str,
    runtime: &'static str,
    uptime_seconds: u64,
}

async fn health(axum::extract::State(state): axum::extract::State<HealthState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        bot_ready: state.ready.load(Ordering::Relaxed),
        architecture: "cloudflare-worker-render-wake",
        runtime: "rust",
        uptime_seconds: state.started.elapsed().as_secs(),
    })
}

pub fn router(state: HealthState) -> Router {
    Router::new().route("/health", get(health)).route("/", get(health)).with_state(state)
}
