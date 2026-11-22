use reqwest::Client;
use serde::Deserialize;
use std::{collections::HashMap, env, sync::Arc, time::{Duration, Instant}};
use tokio::sync::RwLock;
use tracing::warn;

#[derive(Clone)]
pub struct PremiumClient {
    http: Client,
    url: String,
    cache: Arc<RwLock<HashMap<u64, (Instant, bool)>>>,
}

#[derive(Deserialize)]
struct PremiumStatus { #[serde(default)] banned: bool, #[serde(default)] active: bool, expires_at: Option<String> }

impl PremiumClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let url = env::var("PREMIUM_CHECK_URL")
            .unwrap_or_else(|_| "https://mallowbot.kz.ci/api/premium-check".into());
        let http = Client::builder()
            .connect_timeout(Duration::from_millis(800))
            .timeout(Duration::from_millis(1400))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()?;
        Ok(Self { http, url, cache: Arc::new(RwLock::new(HashMap::new())) })
    }

    pub async fn status(&self, user_id: u64) -> (bool, bool, Option<String>) {
        match self.http.get(&self.url).query(&[("discord_id", user_id)]).send().await {
            Ok(response) if response.status().is_success() => match response.json::<PremiumStatus>().await {
                Ok(status) => (status.banned, status.active, status.expires_at),
                Err(error) => { warn!(%error, "Premium status response was invalid"); (false, false, None) }
            },
            Ok(response) => { warn!(status = %response.status(), "Premium status request failed"); (false, false, None) }
            Err(error) => { warn!(%error, "Premium status request unavailable"); (false, false, None) }
        }
    }

    pub async fn is_banned(&self, user_id: u64) -> bool {
        if let Some((at, banned)) = self.cache.read().await.get(&user_id) {
            if at.elapsed() < Duration::from_secs(15) { return *banned; }
        }
        match self.http.get(&self.url).query(&[("discord_id", user_id)]).send().await {
            Ok(response) if response.status().is_success() => match response.json::<PremiumStatus>().await {
                Ok(status) => {
                    self.cache.write().await.insert(user_id, (Instant::now(), status.banned));
                    status.banned
                }
                Err(error) => { warn!(%error, "Premium status response was invalid; allowing request"); false }
            },
            Ok(response) => { warn!(status = %response.status(), "Premium status check failed; allowing request"); false }
            Err(error) => { warn!(%error, "Premium status check unavailable; allowing request"); false }
        }
    }
}
