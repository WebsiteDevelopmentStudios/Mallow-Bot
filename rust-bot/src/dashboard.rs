use anyhow::{Context, Result};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::{collections::HashMap, env, sync::Arc, time::{Duration, Instant}};
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::GuildConfig;

#[derive(Clone)]
pub struct DashboardClient {
    http: Client,
    url: String,
    secret: String,
    cache: Arc<RwLock<HashMap<String, (Instant, GuildConfig)>>>,
    failure_until: Arc<RwLock<HashMap<String, Instant>>>,
}

#[derive(Deserialize)]
struct RuntimeResponse {
    #[serde(default)]
    settings: serde_json::Value,
}

impl DashboardClient {
    pub fn from_env() -> Result<Self> {
        let url = env::var("MALLOW_DASHBOARD_CONFIG_URL")
            .unwrap_or_else(|_| "https://mallowbot.kz.ci/api/runtime-config".into());
        let secret = env::var("MALLOW_DASHBOARD_CONFIG_SECRET").unwrap_or_default();
        let http = Client::builder()
            .connect_timeout(Duration::from_millis(800))
            .timeout(Duration::from_millis(1400))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .context("Could not build dashboard HTTP client")?;
        Ok(Self { http, url, secret, cache: Arc::new(RwLock::new(HashMap::new())), failure_until: Arc::new(RwLock::new(HashMap::new())) })
    }

    pub async fn guild_config(&self, guild_id: u64) -> GuildConfig {
        let key = guild_id.to_string();
        if let Some((at, value)) = self.cache.read().await.get(&key) {
            if at.elapsed() < Duration::from_secs(15) { return value.clone(); }
        }
        if self.failure_until.read().await.get(&key).map(|until| *until > Instant::now()).unwrap_or(false) {
            return self.cached_or_default(&key).await;
        }
        if self.secret.is_empty() { return GuildConfig::default(); }
        let response = self.http.get(&self.url)
            .query(&[("guild_id", key.as_str())])
            .bearer_auth(&self.secret)
            .send().await;
        let response = match response {
            Ok(response) if response.status() == StatusCode::OK => response,
            Ok(response) => {
                debug!(guild_id, status = %response.status(), "Dashboard runtime config request failed; using cached or default settings");
                self.failure_until.write().await.insert(key.clone(), Instant::now() + Duration::from_secs(60));
                return self.cached_or_default(&key).await;
            }
            Err(error) => {
                debug!(guild_id, %error, "Dashboard runtime config request failed; using cached or default settings");
                self.failure_until.write().await.insert(key.clone(), Instant::now() + Duration::from_secs(60));
                return self.cached_or_default(&key).await;
            }
        };
        let payload = match response.json::<RuntimeResponse>().await {
            Ok(payload) => payload,
            Err(error) => {
                debug!(guild_id, %error, "Dashboard returned invalid runtime config; using cached or default settings");
                self.failure_until.write().await.insert(key.clone(), Instant::now() + Duration::from_secs(60));
                return self.cached_or_default(&key).await;
            }
        };
        let settings = payload.settings;
        let mut config = GuildConfig::default();
        config.prefix = settings.get("prefix").and_then(|v| v.as_str()).unwrap_or("-mb").chars().take(8).collect();
        config.greeting = settings.get("greeting").and_then(|v| v.as_str()).unwrap_or(&config.greeting).chars().take(500).collect();
        config.greetings = settings.get("greetings").and_then(|v| v.as_bool()).unwrap_or(true);
        config.mention_only = settings.get("mention_only").and_then(|v| v.as_bool()).unwrap_or(true);
        if let Some(values) = settings.get("disabled_commands").and_then(|v| v.as_array()) {
            config.disabled_commands = values.iter().filter_map(|v| v.as_str()).map(str::to_lowercase).collect();
        }
        for key in ["features", "economy_config", "xp_config", "messages_config", "tickets_config", "swear_count_adjustments"] {
            if let Some(value) = settings.get(key) {
                match key {
                    "features" => config.features = value.clone(),
                    "economy_config" => config.economy_config = value.clone(),
                    "xp_config" => config.xp_config = value.clone(),
                    "messages_config" => config.messages_config = value.clone(),
                    "tickets_config" => config.tickets_config = value.clone(),
                    "swear_count_adjustments" => config.swear_count_adjustments = value.clone(),
                    _ => {}
                }
            }
        }
        if let Some(words) = settings.get("swear_words").and_then(|v| v.as_array()) {
            config.swear_words = words.iter().filter_map(|v| v.as_str()).map(str::to_owned).take(200).collect();
        }
        self.failure_until.write().await.remove(&key);
        self.cache.write().await.insert(key, (Instant::now(), config.clone()));
        config
    }

    async fn cached_or_default(&self, key: &str) -> GuildConfig {
        self.cache.read().await.get(key).map(|(_, value)| value.clone()).unwrap_or_default()
    }
}
