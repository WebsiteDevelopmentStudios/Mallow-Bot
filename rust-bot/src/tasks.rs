use reqwest::Client;
use std::{env, time::Duration};
use tokio::time::interval;
use tracing::{info, warn};

pub fn spawn_keepalives() {
    tokio::spawn(mcxboxbroadcast_keepalive());
    tokio::spawn(lavalink_keepalive());
}

async fn mcxboxbroadcast_keepalive() {
    let base = env::var("MCXBROADCAST_URL").unwrap_or_default().trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        info!("MCXboxBroadcast keep-alive is disabled; MCXBROADCAST_URL is not set.");
        return;
    }
    let seconds = env::var("MCXBROADCAST_PING_INTERVAL").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(300).max(60);
    let client = match Client::builder().timeout(Duration::from_secs(10)).build() {
        Ok(client) => client,
        Err(error) => { warn!(%error, "Could not initialize MCXboxBroadcast HTTP client"); return; }
    };
    let url = format!("{base}/health");
    let mut ticker = interval(Duration::from_secs(seconds));
    loop {
        ticker.tick().await;
        match client.get(&url).header("User-Agent", "Mallow-Bot/1.0").send().await {
            Ok(response) if response.status().is_success() => info!("MCXboxBroadcast keep-alive succeeded."),
            Ok(response) => warn!(status = %response.status(), "MCXboxBroadcast keep-alive returned an unsuccessful status"),
            Err(error) => warn!(%error, "MCXboxBroadcast keep-alive request failed"),
        }
    }
}

async fn lavalink_keepalive() {
    let enabled = env::var("ENABLE_LAVALINK").unwrap_or_default().to_lowercase();
    if !matches!(enabled.as_str(), "1" | "true" | "yes" | "on") {
        info!("Lavalink is disabled by ENABLE_LAVALINK.");
        return;
    }
    let base = env::var("LAVALINK_URI").unwrap_or_default().trim().trim_end_matches('/').to_string();
    let password = env::var("LAVALINK_PASSWORD").unwrap_or_default();
    if base.is_empty() || password.is_empty() {
        warn!("Lavalink is enabled but LAVALINK_URI or LAVALINK_PASSWORD is missing.");
        return;
    }
    let client = match Client::builder().timeout(Duration::from_secs(10)).build() {
        Ok(client) => client,
        Err(error) => { warn!(%error, "Could not initialize Lavalink HTTP client"); return; }
    };
    let url = format!("{base}/version");
    let mut ticker = interval(Duration::from_secs(300));
    loop {
        ticker.tick().await;
        match client.get(&url).header("Authorization", &password).header("User-Agent", "Mallow-Bot/1.0").send().await {
            Ok(response) if response.status().is_success() => info!("Lavalink keep-alive succeeded."),
            Ok(response) => warn!(status = %response.status(), "Lavalink keep-alive returned an unsuccessful status"),
            Err(error) => warn!(%error, "Lavalink keep-alive request failed"),
        }
    }
}
